//! ActivityPanel - generic UI shell for in-progress activity displays.
//!
//! Provides a header bar (title + status badge + optional close button) and
//! a scrollable body. Renders either a connection error, "Connecting..." empty
//! state, or the `children` content. No SSE logic - purely presentational.

use leptos::prelude::*;

/// ActivityPanel - a presentational shell for activity/progress displays.
///
/// Renders:
/// - Header bar with title, status badge, and optional close button
/// - Scrollable body showing: connection error (if any), empty state, or children
#[component]
pub fn ActivityPanel(
    /// Panel title displayed in the header bar.
    title: String,
    /// Current status - drives the status badge text (running/succeeded/failed/other).
    status: RwSignal<String>,
    /// Connection error. `Some(msg)` shows error in red, `None` shows normal content.
    connection_error: RwSignal<Option<String>>,
    /// Called when user clicks the close button. If `None`, no close button is shown.
    #[prop(default = None)]
    on_close: Option<Callback<()>>,
    /// Child content rendered in the scrollable body when no error is present.
    children: Children,
) -> impl IntoView {
    let on_close_handler = move |_| {
        if let Some(cb) = &on_close {
            cb.run(());
        }
    };

    view! {
        <div
            style="margin-top:1rem;border:1px solid var(--border,#ccc);border-radius:6px;background:#0f172a;color:#e2e8f0;font-family:monospace;font-size:0.75rem;max-height:300px;display:flex;flex-direction:column;"
        >
            <div
                style="display:flex;justify-content:space-between;align-items:center;padding:0.5rem 0.75rem;background:#1e293b;border-bottom:1px solid #334155;"
            >
                <div style="display:flex;align-items:center;gap:0.5rem;">
                    <span style="font-weight:600;">{title}</span>
                    <span style="font-size:0.75rem;color:#94a3b8;">
                        {move || {
                            let s = status.get();
                            match s.as_str() {
                                "running" => "● Running",
                                "succeeded" => "✓ Succeeded",
                                "failed" => "✗ Failed",
                                _ => "● Unknown",
                            }
                        }}
                    </span>
                </div>
                {move || {
                    if on_close.is_some() {
                        view! {
                            <button
                                type="button"
                                style="background:none;border:none;color:#94a3b8;cursor:pointer;font-size:1rem;"
                                on:click=on_close_handler
                            >
                                "×"
                            </button>
                        }
                        .into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                }}
            </div>

            <div style="overflow-y:auto;padding:0.5rem 0.75rem;flex:1;">
                {move || {
                    connection_error.get().map(|err| {
                        view! { <div style="color:#ef4444;">{err}</div> }.into_any()
                    })
                }}
                {children()}
            </div>
        </div>
    }
}
