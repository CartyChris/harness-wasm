//! Small web_sys helpers: viewport tracking (§7) and scroll nudges.

use std::sync::atomic::{AtomicBool, Ordering};
use wasm_bindgen::{closure::Closure, JsCast};

static PAGE_HIDING: AtomicBool = AtomicBool::new(false);

/// True once the page is being torn down (reload / navigation / tab close).
/// A stream that dies because of that is settled as Paused, not Interrupted.
pub fn page_hiding() -> bool {
    PAGE_HIDING.load(Ordering::Relaxed)
}

pub fn setup_pagehide_tracking() {
    let on_hide = Closure::wrap(Box::new(move || PAGE_HIDING.store(true, Ordering::Relaxed)) as Box<dyn FnMut()>);
    let on_show = Closure::wrap(Box::new(move || PAGE_HIDING.store(false, Ordering::Relaxed)) as Box<dyn FnMut()>);
    let w = window();
    let _ = w.add_event_listener_with_callback("pagehide", on_hide.as_ref().unchecked_ref());
    let _ = w.add_event_listener_with_callback("beforeunload", on_hide.as_ref().unchecked_ref());
    let _ = w.add_event_listener_with_callback("pageshow", on_show.as_ref().unchecked_ref());
    on_hide.forget();
    on_show.forget();
}

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

/// Distance from the thread's bottom edge, in px.
pub fn thread_distance_from_bottom() -> i32 {
    document()
        .get_element_by_id("thread")
        .map(|el| el.scroll_height() - el.scroll_top() - el.client_height())
        .unwrap_or(0)
}

/// Scroll on the next tick, after DOM updates land — only while the reader
/// is pinned to the bottom, so streaming never yanks someone reading back.
pub fn schedule_scroll_to_bottom(ctx: &crate::state::AppCtx) {
    use leptos::prelude::GetUntracked;
    if !ctx.pinned.get_untracked() {
        return;
    }
    let callback = Closure::once_into_js(move || scroll_thread_to_bottom());
    let callback: js_sys::Function = callback.dyn_into().expect("closure as function");
    let _ = window().set_timeout_with_callback(&callback);
}
