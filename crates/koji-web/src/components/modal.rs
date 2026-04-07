use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;

/// A general-purpose modal overlay.
///
/// The modal **always renders** its children in the DOM and toggles visibility
/// via the `modal-backdrop--open` CSS class. This preserves child component
/// state (signals, in-flight async work, SSE streams) across open/close
/// cycles. As a result, `children` must be `ChildrenFn`.
///
/// Dismissal: backdrop click, the X button in the header, and the Escape
/// key all invoke `on_close`. The host is responsible for setting `open` to
/// false in response — the modal does not hide itself.
#[component]
#[allow(dead_code)]
pub fn Modal(
    /// Whether the modal is currently visible.
    #[prop(into)]
    open: Signal<bool>,
    /// Called when the user dismisses via X / Escape / backdrop click.
    #[prop(into)]
    on_close: Callback<()>,
    /// Title shown in the modal header.
    #[prop(into)]
    title: String,
    /// Modal body. `ChildrenFn` so it can be projected into a reactive
    /// always-rendered tree.
    children: ChildrenFn,
) -> impl IntoView {
    // Register a keydown listener once at component setup. NOT in an Effect,
    // because an Effect would re-register on every signal change and leak
    // listeners. Style mirrors `dashboard.rs:115` —
    // `Closure::<dyn Fn(...)>::new(move |evt| { ... })`.
    {
        let closure = Closure::<dyn Fn(KeyboardEvent)>::new(move |e: KeyboardEvent| {
            if e.key() == "Escape" && open.get_untracked() {
                on_close.run(());
            }
        });
        let window = web_sys::window().expect("window");
        window
            .add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref())
            .expect("add keydown listener");
        // Don't clean up - the listener will be removed when the host unmounts.
    }

    // Click handlers.
    let close_cb = on_close;
    let on_backdrop_click = move |_| close_cb.run(());
    let on_modal_click = move |e: leptos::ev::MouseEvent| {
        e.stop_propagation();
    };
    let on_x_click = move |_| close_cb.run(());

    view! {
        <div
            class="modal-backdrop"
            class=("modal-backdrop--open", move || open.get())
            on:click=on_backdrop_click
        >
            <div class="modal" on:click=on_modal_click>
                <div class="modal-header">
                    <h2 class="modal-title">{title}</h2>
                    <button
                        type="button"
                        class="modal-close"
                        on:click=on_x_click
                        aria-label="Close"
                    >"✕"</button>
                </div>
                <div class="modal-body">
                    {children()}
                </div>
            </div>
        </div>
    }
}
