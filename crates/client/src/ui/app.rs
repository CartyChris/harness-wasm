//! App shell: header, banners, thread, composer, sheets. Mounts into the
//! `#app` element left by the boot shell.

use crate::js;
use crate::state::{current_session, AppCtx};
use crate::storage;
use crate::ui::artifact::ArtifactSheet;
use crate::ui::composer::Composer;
use crate::ui::drawer::SessionDrawer;
use crate::ui::settings_sheet::SettingsSheet;
use crate::ui::thread::Thread;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

pub fn mount() {
    js::setup_viewport_tracking();

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
    provide_context(ctx.clone());

    // Keep the latest message in view while streaming.
    let scroll_ctx = ctx.clone();
    Effect::new(move |_| {
        let session = current_session(&scroll_ctx);
        let _content_len = session.messages.last().map(|m| m.content.len());
        js::schedule_scroll_to_bottom();
    });

    let title_ctx = ctx.clone();
    let title = move || current_session(&title_ctx).title;
    let model_ctx = ctx.clone();
    let model = move || model_ctx.model.get();
    let streaming_ctx = ctx.clone();
    let streaming = move || streaming_ctx.turn.get().is_some();
    let connect_ctx = ctx.clone();
    let needs_connect = move || connect_ctx.connected_hint_needed();
    let storage_error = move || ctx.storage_error.get();
    let notes = move || ctx.notes.get();

    let open_drawer = {
        let ctx = ctx.clone();
        move |_| ctx.drawer_open.set(true)
    };
    let open_settings = {
        let ctx = ctx.clone();
        move |_| ctx.settings_open.set(true)
    };
    let open_settings_from_banner = {
        let ctx = ctx.clone();
        move |_| ctx.settings_open.set(true)
    };
    let dismiss_notes = {
        let ctx = ctx.clone();
        move |_| ctx.notes.update(|n| n.clear())
    };

    let drawer_ctx = ctx.clone();
    let drawer_slot = move || {
        drawer_ctx
            .drawer_open
            .get()
            .then(|| view! { <SessionDrawer ctx=drawer_ctx.clone() /> })
    };
    let settings_ctx = ctx.clone();
    let settings_slot = move || {
        settings_ctx
            .settings_open
            .get()
            .then(|| view! { <SettingsSheet ctx=settings_ctx.clone() /> })
    };
    let artifact_ctx = ctx.clone();
    let artifact_slot = move || {
        artifact_ctx
            .artifact
            .get()
            .map(|artifact| view! { <ArtifactSheet ctx=artifact_ctx.clone() artifact /> })
    };

    view! {
        <div class="app" class:is-streaming=streaming>
            <header class="topbar">
                <button class="icon-btn" aria-label="Sessions" on:click=open_drawer>
                    <span class="icon-menu" aria-hidden="true"></span>
                </button>
                <div class="topbar-title" aria-live="polite">{title}</div>
                <button class="model-chip" aria-label="Model and connection settings" on:click=open_settings>
                    <span class="chip-dot" aria-hidden="true"></span>
                    {model}
                </button>
            </header>

            <Show when=needs_connect>
                <button class="banner banner-connect" on:click=open_settings_from_banner>
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
                    <button class="mini-btn" on:click=dismiss_notes>"Dismiss"</button>
                </div>
            </Show>

            <Thread ctx=ctx.clone() />
            <Composer ctx=ctx.clone() />
        </div>

        // Overlays unmount fully on close (DESIGN.md §7).
        {drawer_slot}
        {settings_slot}
        {artifact_slot}
    }
}
