//! Event log inspector (DeepSeek-Harness pattern): the append-only
//! `SessionEvent` log the transcript and model history are derived from.
//! Unmounts on close.

use crate::state::{current_session, AppCtx};
use harness_core::events::derive_messages;
use leptos::prelude::*;

fn clock(ms: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    format!("{:02}:{:02}:{:02}", d.get_hours(), d.get_minutes(), d.get_seconds())
}

#[component]
pub fn LogSheet(ctx: AppCtx) -> impl IntoView {
    let copied = RwSignal::new(None::<String>);
    let close = move |_| ctx.log_open.set(false);
    let session = Memo::new(move |_| current_session(&ctx));
    let rows = move || {
        session
            .get()
            .events
            .iter()
            .rev()
            .map(|e| {
                let name = e.kind.name();
                let class = format!("ev-kind k-{}", name.split('/').next().unwrap_or("x"));
                view! {
                    <div class="ev">
                        <span class="ev-seq">{format!("#{}", e.seq)}</span>
                        <span class="ev-main">
                            <span class=class>{name}</span>
                            <span class="ev-sum">{e.kind.summary()}</span>
                        </span>
                        <span class="ev-time">{clock(e.at)}</span>
                    </div>
                }
                .into_any()
            })
            .collect::<Vec<_>>()
    };
    let head = move || {
        let s = session.get();
        format!("{} events · {} derived model messages", s.events.len(), derive_messages(&s.events).len())
    };
    let copy = move |_| {
        let raw = serde_json::to_string_pretty(&session.get_untracked().events).unwrap_or_default();
        wasm_bindgen_futures::spawn_local(async move {
            let ok = wasm_bindgen_futures::JsFuture::from(crate::js::window().navigator().clipboard().write_text(&raw))
                .await
                .is_ok();
            copied.set(Some(if ok { "Copied JSON.".into() } else { "Copy failed.".into() }));
        });
    };

    view! {
        <div class="overlay" role="dialog" aria-modal="true" aria-label="Event log">
            <button class="overlay-backdrop" aria-label="Close event log" on:click=close></button>
            <section class="sheet log-sheet">
                <div class="sheet-grab" aria-hidden="true"></div>
                <header class="sheet-head">
                    <h2>"Event log"</h2>
                    <button class="icon-btn" aria-label="Close" on:click=close>
                        <span class="icon-close" aria-hidden="true"></span>
                    </button>
                </header>
                <div class="log-sub">
                    <span class="log-badge">"append-only"</span>
                    <span>{head}</span>
                </div>
                <div class="sheet-body log-body">
                    {move || {
                        let r = rows();
                        if r.is_empty() {
                            view! { <div class="field-hint">"No events yet. Every message, step, tool decision, result, pause, and undo is appended here — the thread is derived from it."</div> }.into_any()
                        } else {
                            r.into_any()
                        }
                    }}
                </div>
                <footer class="sheet-foot">
                    <button class="secondary-btn" on:click=copy>"Copy JSON"</button>
                    <Show when=move || copied.get().is_some()>
                        <span class="health-note" role="status">{move || copied.get().unwrap_or_default()}</span>
                    </Show>
                </footer>
            </section>
        </div>
    }
}
