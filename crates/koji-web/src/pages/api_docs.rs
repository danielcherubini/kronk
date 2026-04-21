use leptos::prelude::*;

/// API Documentation page using Redoc (OpenAPI 3.1.0 viewer).
#[component]
pub fn ApiDocs() -> impl IntoView {
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    Effect::new(move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            // Wait for Leptos to render the DOM first.
            gloo_timers::future::TimeoutFuture::new(100).await;

            if let Some(window) = web_sys::window() {
                if let Some(doc) = window.document() {
                    // Step 1: Inject <redoc> tag into container.
                    if let Some(container) = doc.get_element_by_id("api-docs-redoc-container") {
                        // Inject <redoc> + dark theme CSS overrides + script.
                        let css = r#"<style>
.redoc-wrap { background: #0d1117 !important; color: #e6edf3 !important; }
.redoc-section__title, .redoc-model__title { color: #e6edf3 !important; border-color: #21262d !important; }
.redoc-sidebar { background: #0d1117 !important; border-right-color: #21262d !important; }
.redoc-menu__link, .redoc-markdown p, .redoc-markdown li, .redoc-markdown td, .redoc-markdown th { color: #8b949e !important; }
.redoc-markdown h1, .redoc-markdown h2, .redoc-markdown h3, .redoc-markdown h4 { color: #e6edf3 !important; }
.redoc-section__children { border-color: #21262d !important; }
.redoc-operation__summary { background: transparent !important; }
.redoc-model { background: #161b22 !important; border-color: #21262d !important; color: #8b949e !important; }
.redoc-model--title { background: #21262d !important; color: #e6edf3 !important; }
.redoc-op-tags { border-color: #21262d !important; }
.redoc-tag__section .redoc-section:first-child { padding-top: 0 !important; }
.badge { background: #21262d !important; color: #8b949e !important; }
.badge--get { background: #3fb950 !important; }
.badge--post { background: #58a6ff !important; }
.badge--put { background: #bc8cff !important; }
.badge--delete { background: #f85149 !important; }
.badge--patch { background: #39d2c0 !important; }
.redoc-op-tag__title { color: #e6edf3 !important; }
.redoc-nav__item.is-active .redoc-nav__link, .redoc-menu__link--active { color: #58a6ff !important; }
.redoc-nav__item:hover .redoc-nav__link, .redoc-menu__link:hover { color: #e6edf3 !important; }
.redoc-sidebar::-webkit-scrollbar { width: 6px; }
.redoc-sidebar::-webkit-scrollbar-track { background: #0d1117; }
.redoc-sidebar::-webkit-scrollbar-thumb { background: #21262d; border-radius: 3px; }
.redoc-op-http-methods, .redoc-op-url { color: #e6edf3 !important; }
.redoc-parameter__name { color: #e6edf3 !important; }
.redoc-parameter__description { color: #8b949e !important; }
.redoc-parameter__required { color: #f85149 !important; }
.redoc-parameter__type { color: #39d2c0 !important; }
.redoc-op-servers, .redoc-op-security { border-color: #21262d !important; }
code, pre { background: #161b22 !important; color: #e6edf3 !important; border-color: #21262d !important; }
pre code { color: #e6edf3 !important; }
.redoc-json-preview { background: #0d1117 !important; color: #8b949e !important; }
</style>
<redoc spec-url="/koji/v1/docs" hide-hostname disable-search only-required-in-samples="false" path-in-middle-panel hide-download-button></redoc>"#;
                        container.set_inner_html(css);

                        // Step 2: Create and append the script element AFTER the <redoc> tag exists.
                        // This ensures Redoc finds the element when it scans the DOM.
                        let script = match doc.create_element("script") {
                            Ok(s) => s,
                            Err(_) => {
                                error.set(Some("Failed to create script".to_string()));
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

                        // Append to body so the script executes after DOM parsing is complete.
                        if let Some(body) = doc.body() {
                            let _ = body.append_child(&script);
                        }
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
