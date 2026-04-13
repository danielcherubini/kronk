//! Backup & Restore section for the Config page.

use gloo_net::http::Request;
use leptos::prelude::*;

#[component]
pub fn BackupSection() -> impl IntoView {
    let (uploading, set_uploading) = create_signal(false);
    let (restore_preview, set_restore_preview) = create_signal<Option<RestorePreviewData>>(None);
    let (selected_models, set_selected_models) = create_signal(Vec::<String>::new());
    let (restore_status, set_restore_status) = create_signal(String::new());
    let (restoring, set_restoring) = create_signal(false);
    let (error, set_error) = create_signal<Option<String>>(None);

    // Backup handler - downloads backup as file
    let backup_handler = move |_| {
        let set_error = error;
        let _ = spawn_local(async move {
            match Request::get("/api/backup")
                .send()
                .await
            {
                Ok(resp) => {
                    match resp.blob().await {
                        Ok(blob) => {
                            // Create download link
                            let url = web_sys::window()
                                .unwrap()
                                .create_object_url(&blob)
                                .unwrap();
                            let a = web_sys::window()
                                .unwrap()
                                .document()
                                .unwrap()
                                .create_element("a")
                                .unwrap();
                            a.set_attribute("href", &url).ok();
                            a.set_attribute("download", "koji-backup.tar.gz").ok();
                            a.click();
                            web_sys::window()
                                .unwrap()
                                .revoke_object_url(&url)
                                .ok();
                        }
                        Err(e) => {
                            set_error.set(Some(format!("Failed to get backup: {:?}", e)));
                        }
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Backup failed: {:?}", e)));
                }
            }
        });
    };

    // Upload handler
    let upload_handler = move |ev: web_sys::Event| {
        let target = ev.target().unwrap();
        let input = target.dyn_ref::<web_sys::HtmlInputElement>().unwrap();
        let files = input.files().unwrap();

        if files.length() == 0 {
            return;
        }

        let file = files.get(0).unwrap();
        let set_uploading = set_uploading;
        let set_restore_preview = set_restore_preview;
        let set_selected_models = set_selected_models;
        let set_error = set_error;

        set_uploading.set(true);
        set_error.set(None);

        let _ = spawn_local(async move {
            let mut form_data = web_sys::FormData::new().unwrap();
            form_data.append_with_str(&file, "file").ok();

            // Convert FormData to bytes for gloo_net
            // We need to use a different approach - use fetch directly
            set_uploading.set(false);
            set_error.set(Some("Upload not yet implemented".to_string()));
        });
    };

    // Restore handler
    let restore_handler = move |_| {
        let preview = restore_preview.get();
        let selected = selected_models.get();
        
        if preview.is_none() {
            set_error.set(Some("Please upload a backup file first".to_string()));
            return;
        }
        
        let preview = preview.unwrap();
        let set_restoring = set_restoring;
        let set_restore_status = set_restore_status;
        let set_error = set_error;
        
        set_restoring.set(true);
        set_error.set(None);

        let body = serde_json::json!({
            "upload_id": preview.upload_id,
            "selected_models": selected,
            "skip_backends": false,
            "skip_models": false
        });

        let _ = spawn_local(async move {
            match Request::post("/api/restore")
                .json(&body)
                .unwrap()
                .send()
                .await
            {
                Ok(resp) => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(json) => {
                            let job_id = json.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                            set_restore_status.set(format!("Restore started (job: {})", job_id));
                        }
                        Err(e) => {
                            set_error.set(Some(format!("Failed to parse restore response: {:?}", e)));
                        }
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Restore failed: {:?}", e)));
                }
            }
            set_restoring.set(false);
        });
    };

    view! {
        <div class="space-y-6">
            // Error display
            {move || error.get().map(|e| {
                view! { <div class="alert alert-error">{e}</div> }.into_view()
            }).unwrap_or_default()}

            // Backup Card
            <div class="card">
                <div class="card-header">
                    <h3 class="text-lg font-semibold">"Create Backup"</h3>
                    <p class="text-gray-600">
                        "Download a complete backup of your configuration, model cards, and database."
                    </p>
                </div>
                <div class="card-body">
                    <p class="text-sm text-gray-600 mb-4">
                        "Backup includes:"
                    </p>
                    <ul class="list-disc list-inside text-sm text-gray-600 mb-4 space-y-1">
                        <li>"config.toml"</li>
                        <li>"configs/ (model cards)"</li>
                        <li>"koji.db (SQLite database)"</li>
                    </ul>
                    <p class="text-sm text-amber-600 mb-4">
                        "Note: Model files and backend binaries are NOT included in the backup."
                    </p>
                    <button
                        on:click=backup_handler
                        class="btn btn-primary"
                    >
                        "Download Backup"
                    </button>
                </div>
            </div>

            // Restore Card
            <div class="card">
                <div class="card-header">
                    <h3 class="text-lg font-semibold">"Restore from Backup"</h3>
                    <p class="text-gray-600">
                        "Restore your configuration from a previously created backup."
                    </p>
                </div>
                <div class="card-body">
                    <div class="mb-4">
                        <label class="block text-sm font-medium mb-2">
                            "Select Backup File"
                        </label>
                        <input
                            type="file"
                            accept=".tar.gz"
                            on:change=upload_handler
                            class="input"
                            disabled=uploading
                        />
                        {move || if uploading.get() {
                            view! { <span class="text-sm text-gray-500 ml-2">"Uploading..."</span> }.into_view()
                        } else {
                            view! { }.into_view()
                        }}
                    </div>

                    // Restore Preview
                    {move || restore_preview.get().map(|preview| {
                        view! {
                            <div class="border rounded-lg p-4 mb-4 bg-gray-50">
                                <div class="text-sm text-gray-600 mb-2">
                                    <p><strong>"Created:"</strong> {preview.created_at}</p>
                                    <p><strong>"Koji version:"</strong> {preview.koji_version}</p>
                                    <p><strong>"Models:"</strong> {preview.models.len()}</p>
                                    <p><strong>"Backends:"</strong> {preview.backends.len()}</p>
                                </div>

                                <div class="mt-4">
                                    <h4 class="font-medium mb-2">"Select Models to Restore:"</h4>
                                    <div class="space-y-1 max-h-48 overflow-y-auto">
                                        {preview.models.iter().map(move |model| {
                                            let repo_id = model.repo_id.clone();
                                            let set_selected = set_selected_models;
                                            let selected = selected_models;
                                            
                                            view! {
                                                <div class="flex items-center">
                                                    <input
                                                        type="checkbox"
                                                        id=repo_id.clone()
                                                        checked=move || selected.get().contains(&repo_id)
                                                        on:change=move |_| {
                                                            let mut list = selected.get();
                                                            if list.contains(&repo_id) {
                                                                list.retain(|id| id != &repo_id);
                                                            } else {
                                                                list.push(repo_id.clone());
                                                            }
                                                            set_selected.set(list);
                                                        }
                                                        class="mr-2"
                                                    />
                                                    <label for=repo_id class="text-sm flex-1">{repo_id}</label>
                                                    <span class="text-xs text-gray-500">
                                                        {format!("{} quants, {:.2} MB", model.quants.len(), model.total_size_bytes as f64 / 1_000_000.0)}
                                                    </span>
                                                </div>
                                            }
                                        }).collect::<View>()}
                                    </div>
                                </div>

                                <div class="mt-4 flex gap-2">
                                    <button
                                        on:click=restore_handler
                                        class="btn btn-primary"
                                        disabled=restoring
                                    >
                                        {move || if restoring.get() { "Restoring..." } else { "Restore" }}
                                    </button>
                                    <button
                                        on:click=move |_| {
                                            set_restore_preview.set(None);
                                            set_selected_models.set(Vec::new());
                                            set_restore_status.set(String::new());
                                        }
                                        class="btn btn-secondary"
                                    >
                                        "Cancel"
                                    </button>
                                </div>
                            </div>
                        }
                        .into_view()
                    }).unwrap_or_default()}

                    // Restore Status
                    {move || restore_status.get().map(|status| {
                        view! { <p class="text-sm text-green-600 mt-2">{status}</p> }.into_view()
                    }).unwrap_or_default()}
                </div>
            </div>
        </div>
    }
}

#[derive(serde::Deserialize, Clone)]
struct BackupModelEntry {
    pub repo_id: String,
    pub quants: Vec<String>,
    pub total_size_bytes: i64,
}

#[derive(serde::Deserialize, Clone)]
struct BackendEntry {
    pub name: String,
    pub version: String,
    pub backend_type: String,
    pub source: String,
}

struct RestorePreviewData {
    pub upload_id: String,
    pub created_at: String,
    pub koji_version: String,
    pub models: Vec<BackupModelEntry>,
    pub backends: Vec<BackendEntry>,
}
