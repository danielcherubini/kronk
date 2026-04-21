//! Backup & Restore section for the Config page.

use leptos::prelude::*;

use crate::utils::post_request;

#[component]
pub fn BackupSection() -> impl IntoView {
    let (uploading, set_uploading) = signal(false);
    let (restore_preview, set_restore_preview) = signal::<Option<RestorePreviewData>>(None);
    let (selected_models, set_selected_models) = signal(Vec::<String>::new());
    let (restore_status, set_restore_status) = signal(None::<String>);
    let (restoring, set_restoring) = signal(false);
    let (error, set_error) = signal<Option<String>>(None);

    // Backup handler - downloads backup as file
    let backup_handler = move |_| {
        let set_error = set_error.clone();
        spawn_local(async move {
            match Request::get("/koji/v1/backup").send().await {
                Ok(resp) => {
                    match resp.blob().await {
                        Ok(blob) => {
                            // Create download link with safe error handling
                            if let Some(window) = web_sys::window() {
                                match window.create_object_url(&blob) {
                                    Ok(url) => {
                                        if let Some(doc) = window.document() {
                                            if let Ok(a) = doc.create_element("a") {
                                                let _ = a.set_attribute("href", &url);
                                                let _ = a.set_attribute("download", "koji-backup.tar.gz");
                                                a.click();
                                                let _ = window.revoke_object_url(&url);
                                            } else {
                                                set_error.set(Some("Failed to create download link".to_string()));
                                            }
                                        } else {
                                            set_error.set(Some("Failed to access document".to_string()));
                                        }
                                    }
                                    Err(e) => {
                                        set_error.set(Some(format!("Failed to create object URL: {:?}", e)));
                                    }
                                }
                            } else {
                                set_error.set(Some("Failed to access window".to_string()));
                            }
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

    // Upload handler — safe error handling for all DOM interactions.
    let upload_handler = move |ev: web_sys::Event| {
        let target = match ev.target() {
            Some(t) => t,
            None => return,
        };
        let input = match target.dyn_ref::<web_sys::HtmlInputElement>() {
            Some(i) => i,
            None => {
                set_error.set(Some("Expected an HTMLInputElement".to_string()));
                return;
            }
        };

        let files = match input.files() {
            Some(f) => f,
            None => {
                set_error.set(Some("File input has no files".to_string()));
                return;
            }
        };

        if files.length() == 0 {
            return;
        }

        let file = match files.get(0) {
            Some(f) => f,
            None => {
                set_error.set(Some("Failed to get first file".to_string()));
                return;
            }
        };

        // Note: File upload via FormData requires the fetch API with FormData.
        // gloo_net doesn't support multipart uploads directly.
        // For now, show a message that upload is not yet implemented.
        set_uploading.set(false);
        set_restore_preview.set(None);
        set_selected_models.set(Vec::new());
        set_error.set(Some("File upload not yet implemented in web UI. Use CLI instead.".to_string()));
    };

    // Restore handler — safe error handling for signal reads and JSON building.
    let restore_handler = move |_| {
        let preview = restore_preview.get();
        let selected = selected_models.get();

        if preview.is_none() {
            set_error.set(Some("Please upload a backup file first".to_string()));
            return;
        }

        let preview = match preview {
            Some(p) => p,
            None => return, // unreachable but satisfies the compiler
        };

        let body_json = serde_json::json!({
            "upload_id": preview.upload_id,
            "selected_models": selected,
            "skip_backends": false,
            "skip_models": false
        });

        let json_str = match body_json.to_string() {
            Ok(s) => s,
            Err(e) => {
                set_error.set(Some(format!("Failed to serialize restore request: {e}")));
                return;
            }
        };

        let set_restoring = set_restoring;
        let set_restore_status = set_restore_status;
        let set_error = set_error;

        set_restoring.set(true);
        set_error.set(None);

        spawn_local(async move {
            let body: serde_json::Value = match serde_json::from_str(&json_str) {
                Ok(v) => v,
                Err(e) => {
                    set_restoring.set(false);
                    set_error.set(Some(format!("Failed to serialize restore request: {e}")));
                    return;
                }
            };

            let build_result = match post_request("/koji/v1/restore").json(&body) {
                Ok(r) => r,
                Err(e) => {
                    set_restoring.set(false);
                    set_error.set(Some(format!("Failed to build request: {e}")));
                    return;
                }
            };

            match build_result.send().await {
                Ok(resp) => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(json) => {
                            let job_id = json.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                            set_restore_status.set(Some(format!("Restore started (job: {})", job_id)));
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
                                            set_restore_status.set(None);
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

#[derive(serde::Deserialize, Clone)]
struct RestorePreviewData {
    pub upload_id: String,
    pub created_at: String,
    pub koji_version: String,
    pub models: Vec<BackupModelEntry>,
    pub backends: Vec<BackendEntry>,
}
