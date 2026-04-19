use leptos::prelude::*;
use serde::Deserialize;
use std::sync::LazyLock;

pub use crate::utils::format_size;

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadQueueItemDto {
    pub job_id: String,
    #[expect(dead_code)]
    pub repo_id: String,
    pub filename: String,
    #[expect(dead_code)]
    pub display_name: Option<String>,
    pub status: String,
    pub bytes_downloaded: i64,
    pub total_bytes: Option<i64>,
    #[expect(dead_code)]
    pub error_message: Option<String>,
    #[expect(dead_code)]
    pub started_at: Option<String>,
    #[expect(dead_code)]
    pub completed_at: Option<String>,
    #[expect(dead_code)]
    pub queued_at: String,
    #[expect(dead_code)]
    pub kind: String,
}

impl DownloadQueueItemDto {
    /// Compute progress percentage from bytes. The API doesn't send this —
    /// it's computed client-side to save bandwidth.
    pub fn progress_percent(&self) -> f64 {
        match self.total_bytes {
            Some(total) if total > 0 => {
                (self.bytes_downloaded as f64 / total as f64) * 100.0
            }
            _ => 0.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadsActiveResponse {
    pub items: Vec<DownloadQueueItemDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadsHistoryResponse {
    pub items: Vec<DownloadQueueItemDto>,
    pub total: i64,
}

/// Shared reactive signals for downloads state (used by SSE handler in lib.rs).
pub static ACTIVE_DOWNLOADS: LazyLock<RwSignal<Vec<DownloadQueueItemDto>>> =
    LazyLock::new(|| RwSignal::new(Vec::new()));
pub static HISTORY_ITEMS: LazyLock<RwSignal<Vec<DownloadQueueItemDto>>> =
    LazyLock::new(|| RwSignal::new(Vec::new()));
pub static HISTORY_TOTAL: LazyLock<RwSignal<i64>> = LazyLock::new(|| RwSignal::new(0));
pub static HISTORY_PAGE: LazyLock<RwSignal<i64>> = LazyLock::new(|| RwSignal::new(0));
pub static HISTORY_LIMIT: LazyLock<RwSignal<i64>> = LazyLock::new(|| RwSignal::new(50));

#[component]
pub fn Downloads() -> impl IntoView {
    let active_tab = RwSignal::new("active".to_string()); // "active" | "history"
    let history_page = RwSignal::new(0i64);
    let history_limit = RwSignal::new(50i64);

    // Sync local history signals with shared signals so the SSE handler
    // knows which page to refresh on terminal events.
    Effect::new(move |_| {
        HISTORY_PAGE.set(history_page.get());
    });
    Effect::new(move |_| {
        HISTORY_LIMIT.set(history_limit.get());
    });

    // Initial fetch of active downloads
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(resp) = gloo_net::http::Request::get("/api/downloads/active")
            .send()
            .await
        {
            if let Ok(data) = resp.json::<DownloadsActiveResponse>().await {
                ACTIVE_DOWNLOADS.set(data.items);
            }
        }
    });

    // Load history (initial + whenever page changes)
    let load_history = move || {
        let limit = history_limit.get();
        let page = history_page.get();
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get(&format!(
                "/api/downloads/history?limit={}&offset={}",
                limit,
                page * limit
            ))
            .send()
            .await
            {
                if let Ok(data) = resp.json::<DownloadsHistoryResponse>().await {
                    HISTORY_ITEMS.set(data.items);
                    HISTORY_TOTAL.set(data.total);
                }
            }
        });
    };
    // Load on mount
    load_history();
    // Re-load whenever page or limit changes
    Effect::new(move |_| {
        load_history();
    });

    view! {
        <div class="page downloads-page">
            <h1 class="page__title">"Downloads Center"</h1>

            // Tab navigation
            <div class="downloads-tabs">
                <button
                    class=move || format!("tab-btn {}", if active_tab.get() == "active" { "active" } else { "" })
                    on:click=move |_| active_tab.set("active".to_string())
                >
                    "Active"
                </button>
                <button
                    class=move || format!("tab-btn {}", if active_tab.get() == "history" { "active" } else { "" })
                    on:click=move |_| active_tab.set("history".to_string())
                >
                    "History"
                </button>
            </div>

            // Active tab content
            {move || {
                if active_tab.get() == "active" {
                    view! {
                        <div class="downloads-active">
                            {move || {
                                let items = ACTIVE_DOWNLOADS.get();
                                render_active_list(items)
                            }}
                        </div>
                    }
                    .into_any()
                } else {
                    view! {
                        <div class="downloads-history">
                            {move || {
                                let items = HISTORY_ITEMS.get();
                                render_history_list(items)
                            }}
                            // Pagination controls
                            {move || {
                                let total = HISTORY_TOTAL.get();
                                let limit = history_limit.get();
                                let page = history_page.get();
                                if total > 0 {
                                    let total_pages = ((total as f64) / (limit as f64)).ceil() as i64;
                                    view! {
                                        <div class="pagination">
                                            <button
                                                class="pagination__btn"
                                                disabled=page == 0
                                                on:click=move |_| history_page.update(|p| *p = p.saturating_sub(1))
                                            >
                                                "← Prev"
                                            </button>
                                            <span class="pagination__info">
                                                {format!("Page {} of {}", page + 1, total_pages)}
                                            </span>
                                            <button
                                                class="pagination__btn"
                                                disabled=page >= total_pages - 1
                                                on:click=move |_| history_page.update(|p| *p = p.saturating_add(1))
                                            >
                                                "Next →"
                                            </button>
                                        </div>
                                    }
                                    .into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}
                        </div>
                    }
                    .into_any()
                }
            }}
        </div>
    }
}

fn render_active_list(items: Vec<DownloadQueueItemDto>) -> AnyView {
    if items.is_empty() {
        view! { <p class="empty-state">"No active downloads"</p> }.into_any()
    } else {
        items
            .into_iter()
            .map(render_download_item)
            .collect::<Vec<_>>()
            .into_any()
    }
}

fn render_history_list(items: Vec<DownloadQueueItemDto>) -> AnyView {
    if items.is_empty() {
        view! { <p class="empty-state">"No download history"</p> }.into_any()
    } else {
        items
            .into_iter()
            .map(render_history_item)
            .collect::<Vec<_>>()
            .into_any()
    }
}

fn render_download_item(item: DownloadQueueItemDto) -> impl IntoView {
    let status_str = item.status.clone();
    let status_label = match status_str.as_str() {
        "running" => "Downloading".to_string(),
        "verifying" => "Verifying".to_string(),
        "queued" => "Queued".to_string(),
        _ => status_str.clone(),
    };
    let prog = item.progress_percent();
    let prog_u32 = prog as u32;
    let bytes_down = item.bytes_downloaded as u64;
    let bytes_total = item.total_bytes.map(|t| t as u64).unwrap_or(0);

    view! {
        <div class="download-item">
            <div class="download-item__info">
                <span class="download-item__filename">{item.filename}</span>
                <span
                    class="download-item__status"
                    class:download-item__status--running=item.status == "running"
                    class:download-item__status--verifying=item.status == "verifying"
                >
                    {status_label}
                </span>
            </div>
            <div class="download-item__progress">
                <div class="progress-bar">
                    <div
                        class="progress-bar__fill"
                        style=format!("width: {}%", prog)
                    />
                    <span class="progress-bar__label">
                        {format!("{}%", prog_u32)}
                    </span>
                </div>
                <span class="download-item__bytes">
                    {format_size(bytes_down)} / {format_size(bytes_total)}
                </span>
            </div>
            <button
                class="download-item__cancel"
                on:click=move |_| {
                    let job_id = item.job_id.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        cancel_download(&job_id).await;
                    });
                }
            >
                "Cancel"
            </button>
        </div>
    }
}

fn render_history_item(item: DownloadQueueItemDto) -> impl IntoView {
    let status_str = item.status.clone();
    let status_label = match status_str.as_str() {
        "completed" => "Completed".to_string(),
        "failed" => "Failed".to_string(),
        "cancelled" => "Cancelled".to_string(),
        _ => status_str.clone(),
    };

    let status_class = match item.status.as_str() {
        "completed" => "download-item__status--completed",
        "failed" => "download-item__status--failed",
        "cancelled" => "download-item__status--cancelled",
        _ => "",
    };

    view! {
        <div class="download-item download-item--history">
            <div class="download-item__info">
                <span class="download-item__filename">{item.filename}</span>
                <span class=format!("download-item__status {}", status_class)>
                    {status_label}
                </span>
            </div>
            <div class="download-item__meta">
                <span>{format_size(item.bytes_downloaded as u64)}</span>
                <span>{item.status}</span>
            </div>
        </div>
    }
}

pub async fn cancel_download(job_id: &str) {
    let url = format!("/api/downloads/{}/cancel", job_id);
    if let Ok(resp) = gloo_net::http::Request::post(&url).send().await {
        if resp.status() >= 200 && resp.status() < 300 {
            // Refresh active list
            if let Ok(resp2) = gloo_net::http::Request::get("/api/downloads/active")
                .send()
                .await
            {
                if let Ok(data) = resp2.json::<DownloadsActiveResponse>().await {
                    ACTIVE_DOWNLOADS.set(data.items);
                }
            }
        }
    }
}
