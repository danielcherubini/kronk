use wasm_bindgen::JsCast;

use leptos::prelude::*;

/// API Documentation page using Redoc (OpenAPI 3.1.0 viewer).
#[component]
pub fn ApiDocs() -> impl IntoView {
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    Effect::new(move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(window) = web_sys::window() {
                // Get the Redoc constructor from window.
                let redoc_js = js_sys::Reflect::get(
                    &wasm_bindgen::JsValue::from(&*window),
                    &wasm_bindgen::JsValue::from_str("Redoc"),
                )
                .ok();
                let redoc = match redoc_js {
                    Some(r) if !r.is_undefined() => r,
                    _ => {
                        error.set(Some(
                            "Failed to load Redoc library. Is the CDN available?".to_string(),
                        ));
                        loading.set(false);
                        return;
                    }
                };

                // Get the document body and find our container.
                let doc = match window.document() {
                    Some(d) => d,
                    None => {
                        error.set(Some("No document available".to_string()));
                        loading.set(false);
                        return;
                    }
                };

                let container = match doc.get_element_by_id("api-docs-redoc-container") {
                    Some(el) => el,
                    None => {
                        error.set(Some("Failed to find API docs container".to_string()));
                        loading.set(false);
                        return;
                    }
                };

                // Create a <div> inside the container for Redoc to render into.
                let redoc_div = match doc.create_element("div") {
                    Ok(el) => el,
                    Err(_) => {
                        error.set(Some("Failed to create element".to_string()));
                        loading.set(false);
                        return;
                    }
                };

                // Build the config object using js_sys::Object.
                let config = js_sys::Object::new();
                js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("specUrl"),
                    &wasm_bindgen::JsValue::from_str("/koji/v1/docs"),
                )
                .unwrap();
                js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("hideHostname"),
                    &wasm_bindgen::JsValue::from_bool(true),
                )
                .unwrap();
                js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("disableSearch"),
                    &wasm_bindgen::JsValue::from_bool(true),
                )
                .unwrap();
                js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("onlyRequiredInSamples"),
                    &wasm_bindgen::JsValue::from_bool(false),
                )
                .unwrap();
                js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("pathInMiddlePanel"),
                    &wasm_bindgen::JsValue::from_bool(true),
                )
                .unwrap();
                js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("hideDownloadButton"),
                    &wasm_bindgen::JsValue::from_bool(true),
                )
                .unwrap();

                // Append the div to our container.
                let _ = container.append_child(&redoc_div);

                // Get Redoc.init function.
                let init_fn =
                    match js_sys::Reflect::get(&redoc, &wasm_bindgen::JsValue::from_str("init"))
                        .ok()
                        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
                    {
                        Some(f) => f,
                        None => {
                            error.set(Some("Redoc.init function not found".to_string()));
                            loading.set(false);
                            return;
                        }
                    };

                // Call Redoc.init(div, config). This renders asynchronously.
                match init_fn.call2(&redoc, &redoc_div, &config) {
                    Ok(_) => {
                        // Redoc.render is async — wait a moment for it to finish rendering.
                        gloo_timers::future::TimeoutFuture::new(1000).await;
                        loading.set(false);
                    }
                    Err(e) => {
                        error.set(Some(format!("Redoc init failed: {e:?}")));
                        loading.set(false);
                    }
                }
            } else {
                error.set(Some("No window available".to_string()));
            }
        });
    });

    view! {
        <div class="page api-docs-page">
            <h1 class="page__title">"API Documentation"</h1>
            <p class="api-docs-subtitle">
                "Interactive reference for the Koji Web API (OpenAPI 3.1.0). "
            </p>

            <div id="api-docs-redoc-container" class="api-docs-container" />

            {move || loading.get().then(|| view! {
                <div class="api-docs-loading">
                    <div class="spinner" />
                    "Loading API documentation..."
                </div>
            })}

            {move || error.get().map(|e| view! {
                <div class="error-banner">{e}</div>
            })}
        </div>
    }
}
