use leptos::prelude::*;
use std::boxed::Box;
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
    /// Title shown in the modal header.
    #[prop(into)]
    title: String,
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
