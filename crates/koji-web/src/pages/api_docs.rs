use leptos::prelude::*;

/// API Documentation page using Redoc (OpenAPI 3.1.0 viewer).
#[component]
pub fn ApiDocs() -> impl IntoView {
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Inject the <redoc> web component into the container.
    // Redoc is loaded in index.html before Leptos mounts, so the custom element
    // is already registered — dynamically adding the tag auto-upgrades it.
    Effect::new(move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(window) = web_sys::window() {
                if let Some(doc) = window.document() {
                    if let Some(container) = doc.get_element_by_id("api-docs-redoc-container") {
                        container.set_inner_html(
                            r#"<redoc spec-url="/koji/v1/docs" hide-hostname disable-search only-required-in-samples="false" path-in-middle-panel hide-download-button></redoc>"#,
                        );
                    } else {
                        error.set(Some("Failed to find API docs container".to_string()));
                    }
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
