//! CSS contract tests for `style.css`.
//!
//! These tests don't run a browser — they assert that specific selectors are
//! present in the stylesheet so that visual contracts depended on by the
//! Leptos templates (e.g. `dashboard.rs`'s "Active Models" section) can't be
//! silently dropped.
//!
//! Each test reads `style.css` via `include_str!` so it runs as a normal
//! Rust integration test (`cargo test --package koji-web`) without needing
//! a WASM toolchain.

const STYLE_CSS: &str = include_str!("../style.css");

/// Strip C-style block comments (`/* ... */`) from a CSS source. We use this
/// so that selector-presence assertions can't be satisfied accidentally by
/// commented-out rules.
fn strip_css_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            // Skip until matching `*/`
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Find a CSS rule block (the `{ ... }` body) for the given selector. Returns
/// `None` if the selector doesn't appear at the top level. Selector matching
/// is whitespace-insensitive at the boundary so `.foo .bar` matches both
/// `.foo .bar {` and `.foo  .bar {`.
fn rule_body<'a>(css: &'a str, selector: &str) -> Option<&'a str> {
    // Split into selector groups separated by `{`. We then check each preceding
    // chunk for an exact (trimmed) match against `selector`.
    let mut search_from = 0usize;
    while let Some(brace) = css[search_from..].find('{') {
        let abs_brace = search_from + brace;
        // Walk backwards from `abs_brace` to find the start of the selector
        // (after the previous `}` or start-of-file).
        let sel_start = css[..abs_brace].rfind('}').map(|p| p + 1).unwrap_or(0);
        let raw_selector = css[sel_start..abs_brace].trim();
        // Compare normalised whitespace.
        let normalised: String = raw_selector
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let target: String = selector.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalised == target {
            // Find the matching closing brace.
            let body_start = abs_brace + 1;
            let mut depth = 1i32;
            let mut idx = body_start;
            while idx < css.len() && depth > 0 {
                let c = css.as_bytes()[idx];
                if c == b'{' {
                    depth += 1;
                } else if c == b'}' {
                    depth -= 1;
                }
                idx += 1;
            }
            if depth == 0 {
                // idx is one past the closing brace.
                return Some(&css[body_start..idx - 1]);
            }
            return None;
        }
        search_from = abs_brace + 1;
    }
    None
}

/// The dashboard's "Active Models" `<section>` is wrapped in a
/// `.dashboard-models` class. The CSS must give that section vertical
/// breathing room so it doesn't visually collide with the system metric
/// cards directly above it.
#[test]
fn style_css_defines_dashboard_models_section_spacing() {
    let css = strip_css_comments(STYLE_CSS);
    let body = rule_body(&css, ".dashboard-models")
        .expect("style.css must define a `.dashboard-models` rule");
    assert!(
        body.contains("margin-top"),
        "`.dashboard-models` rule must set `margin-top` to separate the section from the stats grid; got: {body}"
    );
}

/// Inside `.dashboard-models` we render a `.page-header` row containing the
/// section title and the summary count. It needs its own bottom margin so the
/// header doesn't sit flush against the model cards grid.
#[test]
fn style_css_defines_dashboard_models_page_header_spacing() {
    let css = strip_css_comments(STYLE_CSS);
    let body = rule_body(&css, ".dashboard-models .page-header")
        .expect("style.css must define a `.dashboard-models .page-header` rule");
    assert!(
        body.contains("margin-bottom"),
        "`.dashboard-models .page-header` rule must set `margin-bottom` to separate the header from the grid; got: {body}"
    );
}

/// Sanity-check the helper: it must locate top-level rules and ignore
/// commented-out copies. This guards against false positives in the two
/// dashboard-section assertions above.
#[test]
fn rule_body_finds_top_level_rules_and_ignores_comments() {
    let css = strip_css_comments(
        "/* .foo { margin-top: 1rem; } */\n.foo { margin-top: 2rem; }\n.bar .baz { margin-bottom: 0.5rem; }",
    );
    let foo = rule_body(&css, ".foo").expect("`.foo` rule should be found");
    assert!(foo.contains("margin-top: 2rem"));
    assert!(!foo.contains("1rem"), "commented-out copy must be stripped");

    let bar_baz = rule_body(&css, ".bar .baz").expect("`.bar .baz` rule should be found");
    assert!(bar_baz.contains("margin-bottom: 0.5rem"));

    assert!(rule_body(&css, ".missing").is_none());
}

/// The `.model-section` container wraps each section of model cards.
/// It needs vertical spacing (`margin-bottom`) to separate sections visually.
#[test]
fn style_css_defines_model_section_spacing() {
    let css = strip_css_comments(STYLE_CSS);
    let body =
        rule_body(&css, ".model-section").expect("style.css must define a `.model-section` rule");
    assert!(
        body.contains("margin-bottom"),
        "`.model-section` rule must set `margin-bottom` to separate sections; got: {body}"
    );
}

/// The last `.model-section` should not have extra bottom margin.
#[test]
fn style_css_defines_model_section_last_child_spacing() {
    let css = strip_css_comments(STYLE_CSS);
    let body = rule_body(&css, ".model-section:last-child")
        .expect("style.css must define a `.model-section:last-child` rule");
    assert!(
        body.contains("margin-bottom: 0"),
        "`.model-section:last-child` rule must set `margin-bottom: 0`; got: {body}"
    );
}

/// The `.model-section__title` element styles the section header.
/// It needs appropriate typography and a bottom border for visual separation.
#[test]
fn style_css_defines_model_section_title_styling() {
    let css = strip_css_comments(STYLE_CSS);
    let body = rule_body(&css, ".model-section__title")
        .expect("style.css must define a `.model-section__title` rule");
    assert!(
        body.contains("font-size")
            && body.contains("font-weight")
            && body.contains("border-bottom")
            && body.contains("padding-bottom"),
        "`.model-section__title` rule must set typography and border styles; got: {body}"
    );
}
