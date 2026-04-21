use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use leptos::prelude::*;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct LogPayload {
    line: String,
}

#[derive(Debug, Clone, Deserialize)]
struct StatusPayload {
    status: String,
    #[serde(default)]
    error: Option<String>,
}

/// Stream update events via SSE, matching the pattern from `job_log_panel.rs`.
pub async fn stream_update_events(
    update_status: RwSignal<String>,
    update_in_progress: RwSignal<bool>,
    update_available: RwSignal<bool>,
    current_version: RwSignal<String>,
    latest_version: RwSignal<String>,
) {
    let mut es = match EventSource::new("/koji/v1/self-update/events") {
        Ok(es) => es,
        Err(e) => {
            update_status.set(format!("Failed to open event stream: {:?}", e));
            update_in_progress.set(false);
            return;
        }
    };

    let mut log_stream = match es.subscribe("log") {
        Ok(s) => s,
        Err(e) => {
            update_status.set(format!("Failed to subscribe to log events: {:?}", e));
            es.close();
            update_in_progress.set(false);
            return;
        }
    };

    let mut status_stream = match es.subscribe("status") {
        Ok(s) => s,
        Err(e) => {
            update_status.set(format!("Failed to subscribe to status events: {:?}", e));
            es.close();
            update_in_progress.set(false);
            return;
        }
    };

    let mut update_succeeded = false;

    loop {
        let next_log = log_stream.next();
        let next_status = status_stream.next();
        futures_util::pin_mut!(next_log, next_status);

        match futures_util::future::select(next_log, next_status).await {
            futures_util::future::Either::Left((Some(Ok((_, msg))), _)) => {
                let data = msg.data().as_string().unwrap_or_default();
                if let Ok(payload) = serde_json::from_str::<LogPayload>(&data) {
                    update_status.set(payload.line);
                }
            }
            futures_util::future::Either::Right((Some(Ok((_, msg))), _)) => {
                let data = msg.data().as_string().unwrap_or_default();
                if let Ok(payload) = serde_json::from_str::<StatusPayload>(&data) {
                    match payload.status.as_str() {
                        "succeeded" => {
                            update_status.set("Updated! Restarting...".to_string());
                            update_succeeded = true;
                            break;
                        }
                        "failed" => {
                            let err_msg = payload.error.unwrap_or_else(|| "Unknown error".into());
                            update_status.set(format!("Update failed: {}", err_msg));
                            update_in_progress.set(false);
                            break;
                        }
                        "restarting" => {
                            update_status.set("Restarting Koji...".to_string());
                            update_succeeded = true;
                            break;
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                // Stream ended unexpectedly
                break;
            }
        }
    }

    es.close();

    // If update succeeded, poll for server restart
    if update_succeeded {
        poll_for_restart(
            update_status,
            update_in_progress,
            update_available,
            current_version,
            latest_version,
        )
        .await;
    }
}

/// Poll `/koji/v1/self-update/check` every 2 seconds until the server
/// responds with a new version, or give up after 5 attempts.
pub async fn poll_for_restart(
    update_status: RwSignal<String>,
    update_in_progress: RwSignal<bool>,
    update_available: RwSignal<bool>,
    current_version: RwSignal<String>,
    latest_version: RwSignal<String>,
) {
    let old_version = current_version.get_untracked();
    let max_attempts = 5;

    for attempt in 0..max_attempts {
        gloo_timers::future::TimeoutFuture::new(2_000).await;

        update_status.set(format!(
            "Waiting for server to restart... ({}/{})",
            attempt + 1,
            max_attempts
        ));

        if let Ok(resp) = gloo_net::http::Request::get("/koji/v1/self-update/check")
            .send()
            .await
        {
            if let Ok(data) = resp.json::<serde_json::Value>().await {
                if let Some(new_ver) = data["current_version"].as_str() {
                    if new_ver != old_version {
                        update_status.set(format!("Updated to v{}!", new_ver));
                        current_version.set(new_ver.to_string());
                        latest_version.set(String::new());
                        update_available.set(false);
                        update_in_progress.set(false);
                        return;
                    }
                }
            }
        }
    }

    // Gave up — tell user to refresh manually
    update_status.set("Server is restarting. Please refresh the page.".to_string());
    update_in_progress.set(false);
}
