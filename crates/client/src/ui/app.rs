//! App shell: header, banners, thread, composer, sheets. Mounts into the
//! `#app` element left by the boot shell.

use crate::js;
use crate::state::{current_session, AppCtx};
use crate::storage;
use crate::ui::artifact::ArtifactSheet;
use crate::ui::composer::Composer;
use crate::ui::context_sheet::ContextSheet;
use crate::ui::drawer::{new_session, SessionDrawer};
use crate::ui::log_sheet::LogSheet;
use crate::ui::settings_sheet::SettingsSheet;
use crate::ui::thread::Thread;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

pub fn mount() {
    js::setup_viewport_tracking();
    js::setup_pagehide_tracking();

    let document = js::document();
    // The boot shell's job is done once WASM boots: remove it so no stale
    // loading UI lingers under the app.
    if let Some(shell) = document.get_element_by_id("shell") {
        if let Some(parent) = shell.parent_node() {
            let _ = parent.remove_child(&shell);
        }
    }

    let root = document
        .get_element_by_id("app")
        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        .unwrap_or_else(|| {
            document
                .create_element("div")
                .expect("div")
                .dyn_into::<web_sys::HtmlElement>()
                .expect("html element")
        });
    // forget(): the UnmountHandle owns the reactive root — dropping it would
    // unmount the app immediately after mount.
    leptos::mount::mount_to(root, App).forget();
}

#[component]
pub fn App() -> impl IntoView {
    let boot = storage::boot();
    let ctx = AppCtx::new(boot);
    provide_context(ctx);

    // Keep the latest output in view while streaming — only while the
    // reader is pinned to the bottom (tokens land without yanking).
    Effect::new(move |_| {
        ctx.transcript.track();
        ctx.live.with(|l| l.as_ref().map(|l| l.text.len()));
        js::schedule_scroll_to_bottom(&ctx);
    });
    // Switching sessions always lands at the latest message.
    Effect::new(move |_| {
        ctx.current_id.track();
        ctx.pinned.set(true);
        js::schedule_scroll_to_bottom(&ctx);
    });

    let title = move || current_session(&ctx).title;
    let model = move || {
        let m = ctx.model.get();
        m.rsplit('/').next().unwrap_or(&m).to_string()
    };
    let busy = move || ctx.live.with(|l| l.is_some());
    let needs_connect = move || ctx.connected_hint_needed();
    let storage_error = move || ctx.storage_error.get();
    let notes = move || ctx.notes.get();

    let drawer_slot = move || ctx.drawer_open.get().then(|| view! { <SessionDrawer ctx=ctx /> });
    let settings_slot = move || ctx.settings_open.get().then(|| view! { <SettingsSheet ctx=ctx /> });
    let context_slot = move || ctx.context_open.get().then(|| view! { <ContextSheet ctx=ctx /> });
    let log_slot = move || ctx.log_open.get().then(|| view! { <LogSheet ctx=ctx /> });
    let artifact_slot = move || ctx.artifact.get().map(|artifact| view! { <ArtifactSheet ctx=ctx artifact /> });

    view! {
        <div class="app" class:is-streaming=busy>
            <header class="topbar">
                <button class="icon-btn ghost" aria-label="Sessions" on:click=move |_| ctx.drawer_open.set(true)>
                    <span class="icon-menu" aria-hidden="true"></span>
                </button>
                <button class="model-chip" aria-label="Model and connection settings" on:click=move |_| ctx.settings_open.set(true)>
                    <span class="topbar-title" aria-live="polite">{title}</span>
                    <span class="model-line">
                        <span class="chip-dot" class:is-live=busy aria-hidden="true"></span>
                        <span class="model-name">{model}</span>
                        <span class="chev" aria-hidden="true"></span>
                    </span>
                </button>
                <button class="icon-btn ghost" aria-label="New chat" on:click=move |_| new_session(&ctx)>
                    <span class="icon-new" aria-hidden="true"></span>
                </button>
            </header>

            <Show when=needs_connect>
                <button class="banner banner-connect" on:click=move |_| ctx.settings_open.set(true)>
                    "No proxy connected — tap to set the proxy URL and access token."
                </button>
            </Show>
            <Show when=move || storage_error().is_some()>
                <div class="banner banner-warn" role="status">
                    {move || storage_error().unwrap_or_default()}
                </div>
            </Show>
            <Show when=move || !notes().is_empty()>
                <div class="banner banner-note" role="status">
                    <ul>
                        {move || notes().into_iter().map(|n| view! { <li>{n}</li> }).collect::<Vec<_>>()}
                    </ul>
                    <button class="mini-btn" on:click=move |_| ctx.notes.update(|n| n.clear())>"Dismiss"</button>
                </div>
            </Show>

            <Thread ctx=ctx />
            <Composer ctx=ctx />
        </div>

        // Overlays unmount fully on close (DESIGN.md §7).
        {drawer_slot}
        {settings_slot}
        {context_slot}
        {log_slot}
        {artifact_slot}
    }
}
