//! Small web_sys helpers: viewport tracking (§7) and scroll nudges.

use wasm_bindgen::{closure::Closure, JsCast};

pub fn window() -> web_sys::Window {
    web_sys::window().expect("no window")
}

pub fn document() -> web_sys::Document {
    window().document().expect("no document")
}

/// Keep `--vvh` / `--vvtop` in sync with the visual viewport so the composer
/// stays above the keyboard on iPhone Safari. Listens to both `resize` and
/// `scroll` on `visualViewport`, per DESIGN.md §7.
pub fn setup_viewport_tracking() {
    let window = window();
    let Some(viewport) = window.visual_viewport() else {
        return;
    };
    let Some(root) = document()
        .document_element()
        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };

    let update_viewport = {
        let viewport = viewport.clone();
        let root = root.clone();
        move || {
            let height = viewport.height();
            let top = viewport.offset_top();
            let style = root.style();
            let _ = style.set_property("--vvh", &format!("{height}px"));
            let _ = style.set_property("--vvtop", &format!("{top}px"));
        }
    };
    update_viewport();

    let on_change =
        Closure::wrap(Box::new(update_viewport) as Box<dyn FnMut()>);
    let target: &web_sys::EventTarget = viewport.dyn_ref().expect("viewport as EventTarget");
    let _ = target.add_event_listener_with_callback("resize", on_change.as_ref().unchecked_ref());
    let _ = target.add_event_listener_with_callback("scroll", on_change.as_ref().unchecked_ref());
    // Leaked on purpose: lives for the app's lifetime.
    on_change.forget();
}

/// Scroll the thread to its bottom (latest message).
pub fn scroll_thread_to_bottom() {
    if let Some(el) = document().get_element_by_id("thread") {
        el.set_scroll_top(el.scroll_height());
    }
}

/// Scroll on the next tick, after DOM updates land.
pub fn schedule_scroll_to_bottom() {
    let callback = Closure::once_into_js(move || scroll_thread_to_bottom());
    let callback: js_sys::Function = callback.dyn_into().expect("closure as function");
    let _ = window().set_timeout_with_callback(&callback);
}
