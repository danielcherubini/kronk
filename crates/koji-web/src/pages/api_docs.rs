use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use leptos::prelude::*;

/// API Documentation page using Redoc (OpenAPI 3.1.0 viewer).
#[component]
pub fn ApiDocs() -> impl IntoView {
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Inject <redoc> tag FIRST, then load the script so it finds the element.
    Effect::new(move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(window) = web_sys::window() {
                if let Some(doc) = window.document() {
                    // Step 1: Inject <redoc> tag into container so it exists before script loads.
                    if let Some(container) = doc.get_element_by_id("api-docs-redoc-container") {
                        container.set_inner_html(
                            r#"<redoc spec-url="/koji/v1/docs" hide-hostname disable-search only-required-in-samples="false" path-in-middle-panel hide-download-button></redoc>"#,
                        );
                    } else {
                        error.set(Some("Failed to find API docs container".to_string()));
                        loading.set(false);
                        return;
                    }

                    // Step 2: Load Redoc script — it will find our <redoc> tag and initialize.
                    let script = match doc.create_element("script") {
                        Ok(s) => s,
                        Err(_) => {
                            error.set(Some("Failed to create script element".to_string()));
                            loading.set(false);
                            return;
                        }
                    };
                    script
                        .set_attribute(
                            "src",
                            "https://cdn.redoc.ly/redoc/v2.1.3/bundles/redoc.standalone.js",
                        )
                        .unwrap();

                    let err_cb = Closure::wrap(Box::new(move |_event: web_sys::Event| {
                        wasm_bindgen_futures::spawn_local(async move {
                            error.set(Some("Failed to load Redoc from CDN".to_string()));
                        });
                    }) as Box<dyn FnMut(_)>);
                    script
                        .add_event_listener_with_callback("error", err_cb.as_ref().unchecked_ref())
                        .unwrap();
                    err_cb.forget();

                    let _ = doc.body().unwrap().append_child(&script);
                } else {
                    error.set(Some("No document available".to_string()));
                }
            } else {
                error.set(Some("No window available".to_string()));
            }
            loading.set(false);
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
