//! ActivityPanel - generic UI shell for in-progress activity displays.
//!
//! Provides a header bar (title + status badge + optional close button) and
//! a scrollable body. Renders either a connection error, "Connecting..." empty
//! state, or the `children` content. No SSE logic - purely presentational.

use leptos::prelude::*;

/// ActivityPanel - a presentational shell for activity/progress displays.
///
/// Renders:
/// - Header bar with title, status badge, and optional close button
/// - Scrollable body showing: connection error (if any), empty state, or children
#[component]
pub fn ActivityPanel(
    /// Panel title displayed in the header bar.
    title: String,
    /// Current status - drives the status badge text (running/succeeded/failed/other).
    status: RwSignal<String>,
    /// Connection error. `Some(msg)` shows error in red, `None` shows normal content.
    connection_error: RwSignal<Option<String>>,
    /// Called when user clicks the close button. If `None`, no close button is shown.
    #[prop(default = None)]
    on_close: Option<Callback<()>>,
    /// Child content rendered in the scrollable body when no error is present.
    children: Children,
) -> impl IntoView {
    let on_close_handler = move |_| {
        if let Some(cb) = &on_close {
            cb.run(());
        }
    };

    view! {
        <div class="activity-panel">
            <div class="activity-panel__header">
                <div class="activity-panel__title-group">
                    <span class="activity-panel__title">{title}</span>
                    <span class="activity-panel__status">
                        {move || {
                            let s = status.get();
                            match s.as_str() {
                                "running" => "● Running",
                                "succeeded" => "✓ Succeeded",
                                "failed" => "✗ Failed",
                                _ => "● Unknown",
                            }
                        }}
                    </span>
                </div>
                {move || {
                    if on_close.is_some() {
                        view! {
                            <button
                                type="button"
                                class="activity-panel__close"
                                on:click=on_close_handler
                            >
                                "×"
                            </button>
                        }
                        .into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                }}
            </div>

            <div class="activity-panel__body">
                {move || {
                    connection_error.get().map(|err| {
                        view! { <div class="activity-panel__error">{err}</div> }.into_any()
                    })
                }}
                {children()}
            </div>
        </div>
    }
}
