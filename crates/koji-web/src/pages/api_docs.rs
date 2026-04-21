#![allow(clippy::useless_conversion)]

use wasm_bindgen::JsCast;

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
                // Check if Redoc is already loaded (e.g. from a previous navigation).
                let redoc_js = js_sys::Reflect::get(
                    &wasm_bindgen::JsValue::from(&*window),
                    &wasm_bindgen::JsValue::from_str("Redoc"),
                )
                .ok();

                // If Redoc isn't loaded yet, load the script first.
                if redoc_js.as_ref().is_none_or(|r| r.is_undefined()) {
                    let script = match window
                        .document()
                        .and_then(|d| d.create_element("script").ok())
                    {
                        Some(s) => s,
                        None => {
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

                    // Wait for the script to load using a timeout fallback.
                    let mut loaded = false;
                    {
                        use wasm_bindgen::closure::Closure;
                        let loaded_ref = &mut loaded;
                        let load_cb = Closure::wrap(Box::new(move |_event: web_sys::Event| {
                            *loaded_ref = true;
                        })
                            as Box<dyn FnMut(_)>);
                        script
                            .add_event_listener_with_callback(
                                "load",
                                load_cb.as_ref().unchecked_ref(),
                            )
                            .unwrap();
                        load_cb.forget();
                    }

                    // Wait up to 10 seconds for the script to load.
                    gloo_timers::future::TimeoutFuture::new(10_000).await;
                    if !loaded {
                        error.set(Some("Failed to load Redoc library from CDN".to_string()));
                        loading.set(false);
                        return;
                    }

                    // Get Redoc after script loads.
                    let _redoc_js = js_sys::Reflect::get(
                        &wasm_bindgen::JsValue::from(&*window),
                        &wasm_bindgen::JsValue::from_str("Redoc"),
                    )
                    .ok();
                }

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

                // Build the config object with dark theme matching the app.
                let config = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("specUrl"),
                    &wasm_bindgen::JsValue::from_str("/koji/v1/docs"),
                )
                .unwrap();

                // Theme config — native Redoc theming (not CSS overrides).
                let theme = js_sys::Object::new();

                // Colors
                let colors = js_sys::Object::new();
                let primary_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&primary_colors, &"main".into(), &"#58a6ff".into());
                let _ = js_sys::Reflect::set(&colors, &"primary".into(), &primary_colors);

                let success_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&success_colors, &"main".into(), &"#3fb950".into());
                let _ = js_sys::Reflect::set(&colors, &"success".into(), &success_colors);

                let warning_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&warning_colors, &"main".into(), &"#d29922".into());
                let _ = js_sys::Reflect::set(&colors, &"warning".into(), &warning_colors);

                let error_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&error_colors, &"main".into(), &"#f85149".into());
                let _ = js_sys::Reflect::set(&colors, &"error".into(), &error_colors);

                // HTTP method colors
                let http_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&http_colors, &"main".into(), &"#d29922".into());
                let _ = js_sys::Reflect::set(&colors, &"http".into(), &http_colors);

                let get_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&get_colors, &"main".into(), &"#3fb950".into());
                let _ = js_sys::Reflect::set(&colors, &"get".into(), &get_colors);

                let post_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&post_colors, &"main".into(), &"#58a6ff".into());
                let _ = js_sys::Reflect::set(&colors, &"post".into(), &post_colors);

                let put_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&put_colors, &"main".into(), &"#bc8cff".into());
                let _ = js_sys::Reflect::set(&colors, &"put".into(), &put_colors);

                let delete_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&delete_colors, &"main".into(), &"#f85149".into());
                let _ = js_sys::Reflect::set(&colors, &"delete".into(), &delete_colors);

                let patch_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&patch_colors, &"main".into(), &"#39d2c0".into());
                let _ = js_sys::Reflect::set(&colors, &"patch".into(), &patch_colors);

                // Background colors
                let bg_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&bg_colors, &"light".into(), &"#0d1117".into());
                let _ =
                    js_sys::Reflect::set(&bg_colors, &"light-secondary".into(), &"#161b22".into());
                let _ =
                    js_sys::Reflect::set(&bg_colors, &"light-tertiary".into(), &"#21262d".into());
                let _ = js_sys::Reflect::set(&bg_colors, &"dark".into(), &"#0d1117".into());
                let _ =
                    js_sys::Reflect::set(&bg_colors, &"dark-secondary".into(), &"#161b22".into());
                let _ =
                    js_sys::Reflect::set(&bg_colors, &"dark-tertiary".into(), &"#21262d".into());
                let _ = js_sys::Reflect::set(&colors, &"background".into(), &bg_colors);

                // Text colors
                let text_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&text_colors, &"light".into(), &"#e6edf3".into());
                let _ = js_sys::Reflect::set(
                    &text_colors,
                    &"light-secondary".into(),
                    &"#8b949e".into(),
                );
                let _ = js_sys::Reflect::set(&text_colors, &"dark".into(), &"#e6edf3".into());
                let _ =
                    js_sys::Reflect::set(&text_colors, &"dark-secondary".into(), &"#8b949e".into());
                let _ = js_sys::Reflect::set(&colors, &"text".into(), &text_colors);

                // Borders
                let border_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&border_colors, &"light".into(), &"#21262d".into());
                let _ = js_sys::Reflect::set(&border_colors, &"dark".into(), &"#21262d".into());
                let _ = js_sys::Reflect::set(&colors, &"border".into(), &border_colors);

                // Sidebar colors
                let sidebar_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&sidebar_colors, &"light".into(), &"#0d1117".into());
                let _ = js_sys::Reflect::set(&sidebar_colors, &"dark".into(), &"#0d1117".into());
                let _ = js_sys::Reflect::set(&colors, &"sidebar".into(), &sidebar_colors);

                // Code colors
                let code_colors = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&code_colors, &"light".into(), &"#e6edf3".into());
                let _ = js_sys::Reflect::set(&code_colors, &"dark".into(), &"#e6edf3".into());
                let _ = js_sys::Reflect::set(&colors, &"code".into(), &code_colors);

                // Apply colors to theme
                let _ = js_sys::Reflect::set(&theme, &"colors".into(), &colors);

                // Typography
                let typography = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &typography,
                    &wasm_bindgen::JsValue::from_str("fontFamily"),
                    &"-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif".into(),
                );
                let _ = js_sys::Reflect::set(&typography, &"fontSize".into(), &"14px".into());
                let _ = js_sys::Reflect::set(&theme, &"typography".into(), &typography);

                // Apply theme to config
                let _ = js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("theme").into(),
                    &theme,
                );

                // Other options
                let _ = js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("hideHostname"),
                    &wasm_bindgen::JsValue::from_bool(true),
                )
                .unwrap();
                let _ = js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("disableSearch"),
                    &wasm_bindgen::JsValue::from_bool(true),
                )
                .unwrap();
                let _ = js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("onlyRequiredInSamples"),
                    &wasm_bindgen::JsValue::from_bool(false),
                )
                .unwrap();
                let _ = js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("pathInMiddlePanel"),
                    &wasm_bindgen::JsValue::from_bool(true),
                )
                .unwrap();
                let _ = js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("hideDownloadButton"),
                    &wasm_bindgen::JsValue::from_bool(true),
                )
                .unwrap();
                let expand = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&expand, &"200".into(), &"open".into());
                let _ = js_sys::Reflect::set(&expand, &"4xx".into(), &"close".into());
                let _ = js_sys::Reflect::set(&expand, &"5xx".into(), &"close".into());
                let _ = js_sys::Reflect::set(
                    &config,
                    &wasm_bindgen::JsValue::from_str("expandResponses"),
                    &expand,
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

                // Call Redoc.init(div, config).
                match init_fn.call2(&redoc, &redoc_div, &config) {
                    Ok(_) => {
                        // Redoc.render is async — wait for it to finish.
                        gloo_timers::future::TimeoutFuture::new(1500).await;
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
