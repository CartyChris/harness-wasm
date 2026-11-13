//! Composer: a Claude-style card — textarea on top, a chip row below.
//! Chips: approval mode (codex), context budget (aider), event log
//! (deepseek), Pause / Resume (goose). Send becomes Stop while a turn runs.
//! ≥16px input, ≥44px targets, safe-area bottom.

use crate::agent;
use crate::state::{current_session, AppCtx, Phase};
use crate::storage;
use harness_core::context::{assemble, DEFAULT_BUDGET_TOKENS};
use harness_core::events::open_turn;
use harness_core::permissions::ApprovalMode;
use harness_core::workspace::Workspace;
use leptos::ev::KeyboardEvent;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

pub fn short_tokens(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

fn autosize(el: &web_sys::HtmlElement) {
    let style = el.style();
    let _ = style.set_property("height", "auto");
    let h = el.scroll_height().clamp(24, 168);
    let _ = style.set_property("height", &format!("{h}px"));
}

#[component]
pub fn Composer(ctx: AppCtx) -> impl IntoView {
    let draft = ctx.draft;
    let busy = move || ctx.live.with(|l| l.is_some());
    let streaming = move || ctx.live.with(|l| l.as_ref().is_some_and(|l| l.phase == Phase::Streaming));
    let can_send = move || !busy() && !draft.get().trim().is_empty();
    let paused_open = Memo::new(move |_| !busy() && open_turn(&current_session(&ctx).events).is_some_and(|o| o.paused));
    let event_count = Memo::new(move |_| current_session(&ctx).events.len());
    let tokens = Memo::new(move |_| {
        let session = current_session(&ctx);
        let ws = Workspace::derive(&session.events);
        assemble(&session.events, &ws, DEFAULT_BUDGET_TOKENS).est_tokens
    });
    // Visible meaning on the chips ("125 tok", "0 events"); aria-label stays
    // the stable name and title spells it out. Amber near the budget.
    let near_budget = Memo::new(move |_| tokens.get() * 10 >= DEFAULT_BUDGET_TOKENS * 8);
    let input_ref = NodeRef::<leptos::html::Textarea>::new();

    let do_send = move || {
        let text = draft.get_untracked();
        if ctx.live.get_untracked().is_some() {
            agent::cancel(&ctx);
        } else if !text.trim().is_empty() {
            draft.set(String::new());
            if let Some(el) = input_ref.get_untracked() {
                let el: &web_sys::HtmlElement = el.unchecked_ref();
                let _ = el.style().remove_property("height");
            }
            agent::send(&ctx, text);
        }
    };
    let on_keydown = move |event: KeyboardEvent| {
        // Cmd/Ctrl+Enter sends on hardware keyboards; plain Enter is a
        // newline on mobile keyboards.
        if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
            event.prevent_default();
            do_send();
        }
    };
    let on_input = move |_| {
        if let Some(el) = input_ref.get_untracked() {
            autosize(el.unchecked_ref());
        }
    };

    let mode_label = move || ctx.mode.get().label();
    let mode_class = move || format!("chip mode-chip mode-{}", ctx.mode.get().as_str());

    let menu = move || {
        ctx.mode_menu_open.get().then(|| {
            let options: Vec<AnyView> = ApprovalMode::ALL
                .iter()
                .map(|&m| {
                    view! {
                        <button
                            class="menu-item"
                            class:is-active=move || ctx.mode.get() == m
                            role="menuitemradio"
                            aria-checked=move || (ctx.mode.get() == m).to_string()
                            on:click=move |_| {
                                ctx.mode.set(m);
                                storage::save_mode(m, &ctx);
                                ctx.mode_menu_open.set(false);
                            }
                        >
                            <span class=format!("menu-dot mode-{}", m.as_str()) aria-hidden="true"></span>
                            <span class="menu-text">
                                <span class="menu-title">{m.label()}</span>
                                <span class="menu-sub">{m.blurb()}</span>
                            </span>
                        </button>
                    }
                    .into_any()
                })
                .collect();
            view! {
                <div class="menu-layer">
                    <button class="menu-scrim" aria-label="Close approval menu" on:click=move |_| ctx.mode_menu_open.set(false)></button>
                    <div class="menu" role="menu" aria-label="Approval mode">
                        <div class="menu-head">"Approval mode"</div>
                        {options}
                    </div>
                </div>
            }
        })
    };

    view! {
        <div class="composer-zone">
            <Show when=move || !ctx.pinned.get() && !ctx.transcript.with(|t| t.is_empty())>
                <button
                    class="jump-btn"
                    aria-label="Jump to latest"
                    on:click=move |_| {
                        ctx.pinned.set(true);
                        crate::js::scroll_thread_to_bottom();
                    }
                >
                    <span class="jump-glyph" aria-hidden="true"></span>
                </button>
            </Show>
            {menu}
            <div class="composer-card" class:is-busy=busy>
                <textarea
                    class="composer-input"
                    node_ref=input_ref
                    prop:rows="1"
                    prop:placeholder="Message Harness…"
                    bind:value=draft
                    on:keydown=on_keydown
                    on:input=on_input
                    aria-label="Message"
                ></textarea>
                <div class="composer-row">
                    <button class=mode_class aria-label="Approval mode" aria-haspopup="menu" on:click=move |_| ctx.mode_menu_open.update(|v| *v = !*v)>
                        <span class="chip-icon shield" aria-hidden="true"></span>
                        <span>{mode_label}</span>
                    </button>
                    <button
                        class="chip ctx-chip"
                        class:is-near=near_budget
                        aria-label="Context"
                        title=move || format!("Context: about {} tokens of a {} budget", tokens.get(), short_tokens(DEFAULT_BUDGET_TOKENS))
                        on:click=move |_| ctx.context_open.set(true)
                    >
                        <span class="chip-icon ctx" aria-hidden="true"></span>
                        <span>{move || short_tokens(tokens.get())}<span class="chip-unit">" tok"</span></span>
                    </button>
                    <button
                        class="chip log-chip"
                        aria-label="Event log"
                        title=move || format!("Event log: {} events", event_count.get())
                        on:click=move |_| ctx.log_open.set(true)
                    >
                        <span class="chip-icon log" aria-hidden="true"></span>
                        <span>{move || event_count.get().to_string()}<span class="chip-unit">{move || if event_count.get() == 1 { " event" } else { " events" }}</span></span>
                    </button>
                    <span class="composer-spacer"></span>
                    <Show when=streaming>
                        <button class="chip pause-chip" aria-label="Pause" on:click=move |_| agent::pause(&ctx)>
                            <span class="pause-glyph" aria-hidden="true"></span>
                            <span>"Pause"</span>
                        </button>
                    </Show>
                    <Show when=move || paused_open.get()>
                        <button class="chip resume-chip" aria-label="Resume" on:click=move |_| agent::resume(&ctx)>
                            <span class="play-glyph" aria-hidden="true"></span>
                            <span>"Resume"</span>
                        </button>
                    </Show>
                    <button
                        class="send-btn"
                        class:is-stop=busy
                        disabled=move || !busy() && !can_send()
                        on:click=move |_| do_send()
                        aria-label=move || if busy() { "Stop" } else { "Send" }
                    >
                        <span class="send-glyph" class:is-stop=busy aria-hidden="true"></span>
                    </button>
                </div>
            </div>
            <div class="composer-foot"><span class="kbd-hint">"⌘⏎ send · "</span>{move || match ctx.mode.get() {
                ApprovalMode::ReadOnly => "Read-only: every write is blocked",
                ApprovalMode::Ask => "Ask: writes wait for your approval",
                ApprovalMode::Auto => "Auto: writes run without asking",
            }}</div>
        </div>
    }
}
