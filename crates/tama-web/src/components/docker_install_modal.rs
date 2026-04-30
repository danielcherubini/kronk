/// Docker backend install modal with template picker and custom YAML editor.
use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::components::docker_template_card::DockerTemplateGrid;
use crate::utils::target_value;

#[derive(Clone, PartialEq)]
enum ModalTab {
    Template,
    Custom,
}

#[component]
pub fn DockerInstallModal(
    open: RwSignal<bool>,
    #[prop(optional)] on_success: Option<Callback<()>>,
) -> impl IntoView {
    let active_tab = RwSignal::new(ModalTab::Template);
    let backend_name = RwSignal::new(String::new());
    let compose_yaml = RwSignal::new(String::new());
    let dockerfile = RwSignal::new(String::new());
    let target_port = RwSignal::new(String::new());
    let error = RwSignal::new(Option::<String>::None);
    let installing = RwSignal::new(false);
    let install_progress = RwSignal::new(Option::<String>::None);

    // Load template into editor
    let select_template = move |yaml: String| {
        compose_yaml.set(yaml);
        active_tab.set(ModalTab::Custom);
    };

    // Validate YAML
    let validate_yaml = move || {
        let yaml = compose_yaml.get();
        match serde_yml::from_str::<serde_yml::Value>(&yaml) {
            Ok(_) => {
                error.set(None);
                true
            }
            Err(e) => {
                error.set(Some(format!("Invalid YAML: {e}")));
                false
            }
        }
    };

    // Install handler
    let install = move |_| {
        let name = backend_name.get();
        let yaml = compose_yaml.get();
        let df = dockerfile.get();
        let port = target_port.get();

        if name.is_empty() {
            error.set(Some("Backend name is required".to_string()));
            return;
        }
        if yaml.is_empty() {
            error.set(Some("Compose YAML is required".to_string()));
            return;
        }
        if !validate_yaml() {
            return;
        }

        installing.set(true);
        error.set(None);

        let yaml_clone = yaml.clone();
        let port_clone = port.clone();
        let df_clone = df.clone();
        let name_clone = name.clone();

        // POST to install endpoint
        let url = "/tama/v1/backends/docker/install".to_string();
        let body = serde_json::json!({
            "name": name_clone,
            "compose_yaml": yaml_clone,
            "dockerfile": if df_clone.is_empty() { None::<String> } else { Some(df_clone) },
            "target_port": if port_clone.is_empty() { None::<u16> } else { port_clone.parse::<u16>().ok() },
            "version": None::<String>,
        });

        wasm_bindgen_futures::spawn_local(async move {
            let resp = gloo_net::http::Request::post(&url)
                .json(&body)
                .unwrap()
                .send()
                .await;

            match resp {
                Ok(response) => {
                    let data: serde_json::Value = response.json().await.unwrap_or_default();
                    let job_id = data.get("job_id").and_then(|v| v.as_str()).unwrap_or("");

                    // Subscribe to SSE stream using web_sys::EventSource
                    if !job_id.is_empty() {
                        let stream_url =
                            format!("/tama/v1/backends/docker/install/{}/stream", job_id);

                        let installing_sig = installing;

                        match web_sys::EventSource::new(&stream_url) {
                            Ok(es) => {
                                // Open event — install completed successfully
                                if let Some(ref cb) = on_success {
                                    let cb = cb.clone();
                                    let on_open =
                                        Closure::<dyn Fn(web_sys::Event)>::new(move |_| {
                                            installing_sig.set(false);
                                            cb.run(());
                                        });
                                    let _ = es.add_event_listener_with_callback(
                                        "open",
                                        on_open.as_ref().unchecked_ref(),
                                    );
                                    on_open.forget();
                                }

                                // Error event
                                let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_| {
                                    installing.set(false);
                                    error.set(Some("Install failed".to_string()));
                                });
                                let _ = es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
                                on_error.forget();

                                // Log event
                                let on_log = Closure::<dyn Fn(web_sys::MessageEvent)>::new(
                                    move |evt: web_sys::MessageEvent| {
                                        if let Some(data) = evt.data().as_string() {
                                            install_progress.set(Some(data));
                                        }
                                    },
                                );
                                let _ = es.add_event_listener_with_callback(
                                    "log",
                                    on_log.as_ref().unchecked_ref(),
                                );
                                on_log.forget();
                            }
                            Err(_) => {
                                installing.set(false);
                                error.set(Some("Failed to connect to install stream".to_string()));
                            }
                        }
                    } else {
                        installing.set(false);
                        error.set(Some("No job ID returned".to_string()));
                    }
                }
                Err(e) => {
                    installing.set(false);
                    error.set(Some(format!("Install failed: {e}")));
                }
            }
        });
    };

    let on_close = move |_| {
        open.set(false);
        error.set(None);
        install_progress.set(None);
    };

    view! {
        <div
            class="modal-overlay"
            style="display: if open.get() { block } else { none }; position: fixed; top: 0; left: 0; right: 0; bottom: 0; background: rgba(0,0,0,0.5); z-index: 1000;"
            on:click=on_close
        >
            <div
                class="modal-content"
                style="max-width: 800px; margin: 40px auto; background: white; border-radius: 12px; padding: 24px; max-height: 80vh; overflow-y: auto;"
                on:click=|e: leptos::ev::MouseEvent| e.stop_propagation()
            >
                <div class="modal-header" style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 20px;">
                    <h2 style="margin: 0;">"Add Docker Backend"</h2>
                    <button
                        type="button"
                        on:click=on_close
                        style="background: none; border: none; font-size: 1.5em; cursor: pointer;"
                    >"×"</button>
                </div>

                // Tabs
                <div class="tabs" style="display: flex; gap: 8px; margin-bottom: 20px; border-bottom: 1px solid #e0e0e0; padding-bottom: 8px;">
                    <button
                        type="button"
                        on:click=move |_| active_tab.set(ModalTab::Template)
                        style="padding: 8px 16px; border: none; background: if active_tab.get() == ModalTab::Template { \"#007bff\" } else { \"#f0f0f0\" }; color: if active_tab.get() == ModalTab::Template { \"white\" } else { \"black\" }; border-radius: 4px; cursor: pointer;"
                    >"Template"</button>
                    <button
                        type="button"
                        on:click=move |_| active_tab.set(ModalTab::Custom)
                        style="padding: 8px 16px; border: none; background: if active_tab.get() == ModalTab::Custom { \"#007bff\" } else { \"#f0f0f0\" }; color: if active_tab.get() == ModalTab::Custom { \"white\" } else { \"black\" }; border-radius: 4px; cursor: pointer;"
                    >"Custom YAML"</button>
                </div>

                // Template tab
                <Show when=move || active_tab.get() == ModalTab::Template>
                    <DockerTemplateGrid on_select=Callback::new(select_template) />
                </Show>

                // Custom tab
                <Show when=move || active_tab.get() == ModalTab::Custom>
                    <div style="display: flex; flex-direction: column; gap: 12px;">
                        <div>
                            <label style="display: block; margin-bottom: 4px; font-weight: 500;">"Backend Name"</label>
                            <input
                                type="text"
                                prop:value=move || backend_name.get()
                                on:input=move |e| backend_name.set(target_value(&e))
                                placeholder="my-backend"
                                style="width: 100%; padding: 8px; border: 1px solid #ddd; border-radius: 4px;"
                            />
                        </div>
                        <div>
                            <label style="display: block; margin-bottom: 4px; font-weight: 500;">"Compose YAML"</label>
                            <textarea
                                prop:value=move || compose_yaml.get()
                                on:input=move |e| compose_yaml.set(target_value(&e))
                                placeholder="services:\n  vllm:\n    image: ..."
                                style="width: 100%; height: 200px; padding: 8px; border: 1px solid #ddd; border-radius: 4px; font-family: monospace; font-size: 0.9em;"
                            />
                        </div>
                        <div>
                            <label style="display: block; margin-bottom: 4px; font-weight: 500;">"Dockerfile (optional)"</label>
                            <textarea
                                prop:value=move || dockerfile.get()
                                on:input=move |e| dockerfile.set(target_value(&e))
                                placeholder="FROM python:3.11-slim\n..."
                                style="width: 100%; height: 100px; padding: 8px; border: 1px solid #ddd; border-radius: 4px; font-family: monospace; font-size: 0.9em;"
                            />
                        </div>
                        <div>
                            <label style="display: block; margin-bottom: 4px; font-weight: 500;">"Target Port (optional)"</label>
                            <input
                                type="number"
                                prop:value=move || target_port.get()
                                on:input=move |e| target_port.set(target_value(&e))
                                placeholder="8000"
                                style="width: 100%; padding: 8px; border: 1px solid #ddd; border-radius: 4px;"
                            />
                        </div>
                    </div>
                </Show>

                // Error display
                <Show when=move || error.get().is_some()>
                    <div style="margin-top: 16px; padding: 12px; background: #fff0f0; border: 1px solid #ffcccc; border-radius: 4px; color: #cc0000;">
                        {error.get()}
                    </div>
                </Show>

                // Progress display
                <Show when=move || install_progress.get().is_some()>
                    <div style="margin-top: 16px; padding: 12px; background: #f0f0f0; border-radius: 4px; font-family: monospace; font-size: 0.85em; max-height: 150px; overflow-y: auto;">
                        {install_progress.get()}
                    </div>
                </Show>

                // Buttons
                <div style="display: flex; gap: 8px; margin-top: 20px; justify-content: flex-end;">
                    <button
                        type="button"
                        on:click=on_close
                        style="padding: 8px 16px; border: 1px solid #ddd; background: white; border-radius: 4px; cursor: pointer;"
                    >"Cancel"</button>
                    <button
                        type="button"
                        on:click=install
                        prop:disabled=move || installing.get()
                        style="padding: 8px 16px; border: none; background: if installing.get() { \"#ccc\" } else { \"#007bff\" }; color: white; border-radius: 4px; cursor: pointer;"
                    >{if installing.get() { "Installing..." } else { "Install" }}</button>
                </div>
            </div>
        </div>
    }
}
