use leptos::prelude::*;
use serde::Deserialize;

/// A global toast store accessible from any component.
#[derive(Clone)]
pub struct ToastStore {
    toasts: RwSignal<Vec<Toast>>,
}

impl Default for ToastStore {
    fn default() -> Self {
        Self {
            toasts: RwSignal::new(vec![]),
        }
    }
}

impl ToastStore {
    pub fn global() -> Self {
        Self::default()
    }

    pub fn add(&self, toast: Toast) {
        let id = toast.id.clone();
        let duration_secs = toast.duration_secs;
        self.toasts.update(|toasts| {
            toasts.push(toast);
            if toasts.len() > 5 {
                toasts.remove(0); // Remove oldest when exceeding max
            }
        });
        // Auto-dismiss after duration_secs
        let store_clone = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(duration_secs as u32 * 1000).await;
            store_clone.remove(&id);
        });
    }

    pub fn remove(&self, id: &str) {
        self.toasts.update(|toasts| {
            toasts.retain(|t| t.id != id);
        });
    }

    #[expect(dead_code)]
    pub fn clear(&self) {
        self.toasts.set(vec![]);
    }

    pub fn toasts(&self) -> Vec<Toast> {
        self.toasts.get()
    }

    /// Convert a DownloadEvent to a Toast (some events don't produce toasts).
    pub fn from_download_event(event: &DownloadEvent) -> Option<Toast> {
        match event.event.as_str() {
            "Started" => Some(Toast {
                id: format!("toast-{}", uuid::Uuid::new_v4()),
                severity: ToastSeverity::Info,
                title: event.filename.clone().unwrap_or_default(),
                message: "Downloading...".to_string(),
                duration_secs: 5,
                action_label: None,
                on_action: None,
            }),
            "Completed" => {
                let duration = if let Some(ms) = event.duration_ms {
                    format!("{}s", ms / 1000)
                } else {
                    "unknown time".to_string()
                };
                let size = format_size(event.size_bytes.unwrap_or(0));
                Some(Toast {
                    id: format!("toast-{}", uuid::Uuid::new_v4()),
                    severity: ToastSeverity::Success,
                    title: event.filename.clone().unwrap_or_default(),
                    message: format!("{size} in {duration}"),
                    duration_secs: 8,
                    action_label: Some("View".to_string()),
                    on_action: None,
                })
            }
            "Failed" => Some(Toast {
                id: format!("toast-{}", uuid::Uuid::new_v4()),
                severity: ToastSeverity::Error,
                title: event.filename.clone().unwrap_or_default(),
                message: event
                    .error
                    .clone()
                    .unwrap_or_else(|| "Unknown error".to_string()),
                duration_secs: 10,
                action_label: None,
                on_action: None,
            }),
            "Cancelled" => Some(Toast {
                id: format!("toast-{}", uuid::Uuid::new_v4()),
                severity: ToastSeverity::Warning,
                title: event.filename.clone().unwrap_or_default(),
                message: "Download cancelled".to_string(),
                duration_secs: 5,
                action_label: None,
                on_action: None,
            }),
            _ => None, // Progress, Verifying, Queued don't produce toasts
        }
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: String,
    pub severity: ToastSeverity,
    pub title: String,
    pub message: String,
    pub duration_secs: u64,
    #[expect(dead_code)]
    pub action_label: Option<String>,
    #[expect(dead_code)]
    pub on_action: Option<Callback<()>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToastSeverity {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Deserialize)]
pub struct DownloadEvent {
    pub event: String,
    #[expect(dead_code)]
    pub job_id: String,
    // These fields vary by event type
    pub filename: Option<String>,
    #[expect(dead_code)]
    pub repo_id: Option<String>,
    #[expect(dead_code)]
    pub bytes_downloaded: Option<u64>,
    #[expect(dead_code)]
    pub total_bytes: Option<u64>,
    pub size_bytes: Option<u64>,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Render the toast container (top-right notification area).
#[component]
pub fn ToastContainer(store: ToastStore) -> impl IntoView {
    view! {
        <div class="toast-container">
            {move || {
                store
                    .toasts()
                    .into_iter()
                    .map(|toast| {
                        let store_clone = store.clone();
                        let toast_id = toast.id.clone();
                        view! {
                            <ToastCard
                                toast=toast.clone()
                                on_remove=move || {
                                    store_clone.remove(&toast_id);
                                }
                            />
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>()
            }}
        </div>
    }
}

#[component]
pub fn ToastCard(toast: Toast, on_remove: impl Fn() + Send + 'static) -> impl IntoView {
    let severity_class = move || match toast.severity {
        ToastSeverity::Info => "toast--info",
        ToastSeverity::Success => "toast--success",
        ToastSeverity::Warning => "toast--warning",
        ToastSeverity::Error => "toast--error",
    };

    view! {
        <div class=format!("toast {}", severity_class())>
            <div class="toast__content">
                <span class="toast__title">{toast.title}</span>
                <span class="toast__message">{toast.message}</span>
            </div>
            <button
                class="toast__close"
                on:click=move |_| {
                    on_remove();
                }
            >
                "✕"
            </button>
        </div>
    }
}
