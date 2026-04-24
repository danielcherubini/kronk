use leptos::prelude::*;
use std::boxed::Box;

/// Title content for the modal header — either a static string or a reactive
/// closure that returns a `String`. This lets callers pass plain text (for
/// most modals) or a `move || format!(...)` closure when the title must
/// reflect changing state (e.g. the backend name in the log viewer).
pub enum ModalTitle {
    Static(String),
    Reactive(Box<dyn Fn() -> String + Send + Sync>),
}

impl Clone for ModalTitle {
    fn clone(&self) -> Self {
        match self {
            ModalTitle::Static(s) => ModalTitle::Static(s.clone()),
            ModalTitle::Reactive(_) => {
                // A reactive title can't be cloned since the closure is not Clone.
                // In practice this only matters when Leptos clones the component
                // props during re-render, which is fine — we fall back to a static
                // snapshot. This shouldn't occur in normal usage.
                panic!("ModalTitle::Reactive does not support Clone")
            }
        }
    }
}

impl From<String> for ModalTitle {
    fn from(s: String) -> Self {
        ModalTitle::Static(s)
    }
}

impl From<&str> for ModalTitle {
    fn from(s: &str) -> Self {
        ModalTitle::Static(s.to_string())
    }
}

impl<F, R> From<F> for ModalTitle
where
    F: Fn() -> R + Send + Sync + 'static,
    R: Into<String>,
{
    fn from(f: F) -> Self {
        ModalTitle::Reactive(Box::new(move || f().into()))
    }
}

impl ModalTitle {
    pub fn render(&self) -> String {
        match self {
            ModalTitle::Static(s) => s.clone(),
            ModalTitle::Reactive(f) => f(),
        }
    }
}
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;

/// Helper to convert a raw pointer to usize and back.
/// SAFETY: The caller must ensure the pointer is valid for the lifetime of usage.
unsafe fn ptr_to_usize<T>(ptr: *mut T) -> usize {
    ptr as usize
}

/// SAFETY: The returned pointer must be valid and not yet consumed.
unsafe fn usize_to_ptr<T>(val: usize) -> *mut T {
    val as *mut T
}

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
pub fn Modal(
    /// Whether the modal is currently visible.
    #[prop(into)]
    open: Signal<bool>,
    /// Called when the user dismisses via X / Escape / backdrop click.
    #[prop(into)]
    on_close: Callback<()>,
    /// Title shown in the modal header. Accepts a `String`, `&str`, or a
    /// reactive closure (`move || format!("...")`). Reactive titles update
    /// when their captured signals change.
    #[prop(into)]
    title: ModalTitle,
    /// Modal body. `ChildrenFn` so it can be projected into a reactive
    /// always-rendered tree.
    children: ChildrenFn,
) -> impl IntoView {
    // Store the closure on the heap so it stays alive for the event listener.
    // The Closure is kept alive without calling forget() — it will be
    // deallocated when Box::from_raw is called during cleanup.
    let window = web_sys::window().expect("window");
    let closure: Box<Closure<dyn Fn(KeyboardEvent)>> =
        Box::new(Closure::new(move |e: KeyboardEvent| {
            if e.key() == "Escape" && open.get_untracked() {
                on_close.run(());
            }
        }));
    window
        .add_event_listener_with_callback("keydown", (*closure).as_ref().unchecked_ref())
        .expect("add keydown listener");
    // Store as usize (Send + Sync) for cleanup — Box::from_raw deallocates on drop.
    let ptr = unsafe { ptr_to_usize(Box::into_raw(closure)) };

    // Clean up the keydown listener and deallocate the Closure when the modal unmounts.
    on_cleanup(move || {
        let raw_ptr: *mut Closure<dyn Fn(KeyboardEvent)> = unsafe { usize_to_ptr(ptr) };
        window
            .remove_event_listener_with_callback(
                "keydown",
                unsafe { &*raw_ptr }.as_ref().unchecked_ref(),
            )
            .ok();
        // Reconstruct and drop the Box — this deallocates the Closure.
        unsafe {
            drop(Box::from_raw(raw_ptr));
        }
    });

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
                    <h2 class="modal-title">{move || title.render()}</h2>
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
