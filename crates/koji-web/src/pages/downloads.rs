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
    pub progress_percent: f64,
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

    // Load history
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(resp) = gloo_net::http::Request::get(&format!(
            "/api/downloads/history?limit={}&offset={}",
            history_limit.get(),
            history_page.get() * history_limit.get()
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
                        style=format!("width: {}%", item.progress_percent)
                    />
                    <span class="progress-bar__label">
                        {format!("{}%", item.progress_percent as u32)}
                    </span>
                </div>
                <span class="download-item__bytes">
                    {format_size(item.bytes_downloaded as u64)} / {format_size(item.total_bytes.map(|t| t as u64).unwrap_or(0))}
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
