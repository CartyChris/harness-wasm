//! Composer: textarea + Send/Stop. Stays usable while a turn runs; Send
//! becomes Stop (cancel). ≥16px input, ≥44px targets, safe-area bottom.

use crate::agent;
use crate::state::AppCtx;
use leptos::prelude::*;
use leptos::ev::KeyboardEvent;

#[component]
pub fn Composer(ctx: AppCtx) -> impl IntoView {
    let draft = RwSignal::new(String::new());
    let streaming = move || ctx.turn.get().is_some();
    let can_send = move || !streaming() && !draft.get().trim().is_empty();

    let do_send = move || {
        let text = draft.get_untracked();
        if ctx.turn.get_untracked().is_some() {
            agent::cancel(&ctx);
        } else if !text.trim().is_empty() {
            draft.set(String::new());
            agent::send(&ctx, text);
        }
    };

    let on_click = {
        let do_send = do_send.clone();
        move |_| do_send()
    };
    let on_keydown = move |event: KeyboardEvent| {
        // Cmd/Ctrl+Enter sends on hardware keyboards; plain Enter is a
        // newline on mobile keyboards.
        if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
            event.prevent_default();
            do_send();
        }
    };

    view! {
        <div class="composer-zone">
            <div class="composer">
                <textarea
                    class="composer-input"
                    prop:rows="1"
                    prop:placeholder="Message Harness…"
                    bind:value=draft
                    on:keydown=on_keydown
                    aria-label="Message"
                ></textarea>
                <button
                    class="send-btn"
                    class:is-stop=streaming
                    disabled=move || !streaming() && !can_send()
                    on:click=on_click
                    aria-label=move || if streaming() { "Stop" } else { "Send" }
                >
                    <span class="send-glyph" class:is-stop=streaming aria-hidden="true"></span>
                </button>
            </div>
            <div class="composer-foot">"⏎ newline · ⌘⏎ send"</div>
        </div>
    }
}
