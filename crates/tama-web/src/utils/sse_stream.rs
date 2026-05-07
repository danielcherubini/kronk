//! Server-Sent Events (SSE) stream utility with consumer-driven reconnection.
//!
//! This module provides [`SseConnection`] — a handle to an SSE connection with
//! automatic reconnection support. The consumer owns the reconnection loop via
//! [`SseConnection::connect_once`], which attempts to open the EventSource with
//! exponential backoff on failure.
//!
//! # Example
//!
//! ```rust,ignore
//! use tama_web::utils::sse_stream::{SseReconnectConfig, SseConnection};
//!
//! let cancelled = RwSignal::new(false);
//! let config = SseReconnectConfig::default();
//! let mut conn = SseConnection::create(
//!     "/tama/v1/downloads/events".to_string(),
//!     cancelled,
//!     Some(config),
//! );
//!
//! // Consumer-driven reconnection loop
//! loop {
//!     if cancelled.get() { break; }
//!     if conn.is_reconnecting().get() {
//!         gloo_timers::future::TimeoutFuture::new(1000).await;
//!     }
//!     if conn.connect_once().await.is_ok() {
//!         let stream = conn.subscribe("Started")?;
//!         // consume stream...
//!         break;
//!     }
//! }
//! ```

use futures_util::{
    future::{AbortHandle, AbortRegistration, Abortable},
    Stream,
};
use gloo_net::eventsource::futures::{EventSource, EventSourceSubscription};
use leptos::prelude::{GetUntracked, RwSignal, Set};

/// Configuration for SSE reconnection behavior.
#[derive(Debug, Clone)]
pub struct SseReconnectConfig {
    /// Initial delay between reconnection attempts (in milliseconds).
    pub initial_delay_ms: u32,
    /// Maximum delay between reconnection attempts (in milliseconds).
    pub max_delay_ms: u32,
    /// Maximum number of connection attempts. `None` means infinite.
    pub max_attempts: Option<u32>,
}

impl Default for SseReconnectConfig {
    fn default() -> Self {
        Self {
            initial_delay_ms: 1_000,
            max_delay_ms: 30_000,
            max_attempts: None, // infinite
        }
    }
}

/// Error type for SSE operations.
#[derive(Debug, Clone)]
pub enum SseError {
    /// Failed to establish the SSE connection.
    ConnectionFailed(String),
    /// Failed to subscribe to an event channel.
    SubscribeFailed(String),
    /// The connection was closed by the consumer.
    Closed,
}

impl std::fmt::Display for SseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SseError::ConnectionFailed(msg) => write!(f, "SSE connection failed: {}", msg),
            SseError::SubscribeFailed(msg) => write!(f, "SSE subscribe failed: {}", msg),
            SseError::Closed => write!(f, "SSE connection closed"),
        }
    }
}

impl std::error::Error for SseError {}

/// A single SSE event received from a channel.
pub struct SseEvent {
    /// The event type (e.g., "Started", "Progress", "message").
    pub event_type: String,
    /// The event data payload.
    pub data: String,
}

/// A stream of SSE events for a single channel.
pub struct SseStream {
    inner: EventSourceSubscription,
}

impl Stream for SseStream {
    type Item = Result<SseEvent, SseError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        match Stream::poll_next(std::pin::pin!(&mut self.inner), cx) {
            Poll::Ready(Some(Ok((event_type, msg)))) => {
                let data = msg.data().as_string().unwrap_or_default();
                Poll::Ready(Some(Ok(SseEvent { event_type, data })))
            }
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Some(Err(SseError::ConnectionFailed(e.to_string()))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Handle to an SSE connection with automatic reconnection support.
///
/// The `SseConnection` does NOT own a reconnection loop. Instead it provides
/// [`SseConnection::connect_once`] — an async method that attempts to open the
/// EventSource with exponential backoff on failure. The consumer owns the
/// reconnection loop.
pub struct SseConnection {
    url: String,
    config: SseReconnectConfig,
    cancelled: RwSignal<bool>,
    is_reconnecting: RwSignal<bool>,
    last_error: RwSignal<Option<String>>,
    /// Uses RefCell instead of Cell because AbortHandle is not Copy.
    abort_handle: std::cell::RefCell<AbortHandle>,
    abort_registration: std::cell::Cell<Option<AbortRegistration>>,
    event_source: std::cell::RefCell<Option<EventSource>>,
    attempt_count: std::cell::Cell<u32>,
    delay_ms: std::cell::Cell<u32>,
}

impl SseConnection {
    /// Attempt to connect the EventSource once, with internal retry on failure.
    ///
    /// On the first call, this immediately attempts to connect. On subsequent
    /// calls (reconnection), it waits with exponential backoff before trying
    /// again.
    ///
    /// - If `cancelled` is `true`, returns `Err(SseError::Closed)`.
    /// - If `max_attempts` is reached, returns `Err(SseError::ConnectionFailed(...))`.
    /// - On success, resets backoff state and returns `Ok(())`.
    pub async fn connect_once(&self) -> Result<(), SseError> {
        // Check cancellation before attempting.
        if self.cancelled.get_untracked() {
            return Err(SseError::Closed);
        }

        let initial_delay = self.config.initial_delay_ms;
        let max_delay = self.config.max_delay_ms;
        let max_attempts = self.config.max_attempts;

        loop {
            let attempt = self.attempt_count.get();

            // If this is a reconnection attempt, wait with backoff.
            if attempt > 0 {
                self.is_reconnecting.set(true);
                let delay = self.delay_ms.get();

                // Create an abortable timeout so that `close()` can cancel the wait.
                // Take the old registration (dropping it), create a fresh pair,
                // store the new registration in the cell, and use it with Abortable.
                let old_reg = self.abort_registration.take();
                drop(old_reg);
                let (new_handle, new_reg) = AbortHandle::new_pair();
                *self.abort_handle.borrow_mut() = new_handle;

                match Abortable::new(gloo_timers::future::TimeoutFuture::new(delay), new_reg).await
                {
                    Ok(()) => {
                        // Timeout completed — proceed to connect.
                    }
                    Err(_) => {
                        // Aborted — check if we should stop.
                        if self.cancelled.get_untracked() {
                            return Err(SseError::Closed);
                        }
                        // Otherwise, the abort was for a new connection attempt.
                        // Continue the loop to try the new attempt.
                        continue;
                    }
                }
            }

            // Attempt to create the EventSource.
            match EventSource::new(&self.url) {
                Ok(es) => {
                    // Successful connection — reset state.
                    *self.event_source.borrow_mut() = Some(es);
                    self.is_reconnecting.set(false);
                    self.last_error.set(None);
                    self.delay_ms.set(initial_delay);
                    self.attempt_count.set(0);
                    return Ok(());
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    self.last_error.set(Some(err_msg.clone()));
                    self.attempt_count.set(attempt + 1);

                    // Check max attempts.
                    if let Some(max) = max_attempts {
                        if attempt + 1 >= max {
                            return Err(SseError::ConnectionFailed(err_msg));
                        }
                    }

                    // Exponential backoff for next attempt.
                    let current_delay = self.delay_ms.get();
                    self.delay_ms.set((current_delay * 2).min(max_delay));

                    continue;
                }
            }
        }
    }

    /// Subscribe to a named event channel.
    ///
    /// Returns an [`SseStream`] that yields [`SseEvent`]s for the given channel.
    ///
    /// # Errors
    ///
    /// Returns `Err(SseError::ConnectionFailed("not connected"))` if
    /// [`connect_once`](Self::connect_once) has not been called successfully.
    pub fn subscribe(&self, channel: &str) -> Result<SseStream, SseError> {
        let mut es_borrow = self.event_source.borrow_mut();
        let event_source = es_borrow
            .as_mut()
            .ok_or_else(|| SseError::ConnectionFailed("not connected".to_string()))?;

        let subscription = event_source
            .subscribe(channel)
            .map_err(|e| SseError::SubscribeFailed(e.to_string()))?;

        Ok(SseStream {
            inner: subscription,
        })
    }

    /// Returns a reactive signal indicating whether the connection is currently
    /// reconnecting (waiting with backoff).
    pub fn is_reconnecting(&self) -> RwSignal<bool> {
        self.is_reconnecting
    }

    /// Returns a reactive signal containing the last error message, if any.
    pub fn last_error(&self) -> RwSignal<Option<String>> {
        self.last_error
    }

    /// Close the connection and stop all reconnection attempts.
    ///
    /// This sets the cancelled flag, aborts any in-flight wait, and closes the
    /// underlying EventSource.
    pub fn close(&self) {
        self.cancelled.set(true);
        {
            let handle = self.abort_handle.borrow();
            handle.abort();
        }
        if let Some(es) = self.event_source.borrow_mut().take() {
            es.close();
        }
    }
}

impl Drop for SseConnection {
    fn drop(&mut self) {
        self.close();
    }
}

/// Create a new [`SseConnection`] handle.
///
/// This does NOT open any connection — it simply creates the handle with the
/// given configuration. Call [`SseConnection::connect_once`] to start the
/// connection.
///
/// # Arguments
///
/// * `url` — The SSE endpoint URL (e.g., `/tama/v1/downloads/events`).
/// * `cancelled` — A signal that, when `true`, causes `connect_once` to return
///   `Err(SseError::Closed)`.
/// * `config` — Optional reconnection configuration. Uses defaults if `None`.
pub fn create(
    url: String,
    cancelled: RwSignal<bool>,
    config: Option<SseReconnectConfig>,
) -> SseConnection {
    let config = config.unwrap_or_default();
    let initial_delay = config.initial_delay_ms;
    let is_reconnecting = RwSignal::new(false);
    let last_error = RwSignal::new(None);

    let (abort_handle, abort_registration) = AbortHandle::new_pair();

    SseConnection {
        url,
        config,
        cancelled,
        is_reconnecting,
        last_error,
        abort_handle: std::cell::RefCell::new(abort_handle),
        abort_registration: std::cell::Cell::new(Some(abort_registration)),
        event_source: std::cell::RefCell::new(None),
        attempt_count: std::cell::Cell::new(0),
        delay_ms: std::cell::Cell::new(initial_delay),
    }
}
