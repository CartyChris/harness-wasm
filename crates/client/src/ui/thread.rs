//! Thread: the transcript derived from the session's append-only log, plus
//! the live (not yet durable) tail of the open turn. Assistant text renders
//! through the safe Markdown AST only — no `innerHTML`, ever (DESIGN.md §6).

use crate::agent;
use crate::state::{current_session, AppCtx, LiveTurn, Phase};
use crate::js;
use crate::ui::md::render_markdown;
use crate::ui::timeline::tool_card;
use harness_core::events::{regenerable_turn, retryable_turn, StepItem, TranscriptItem, TurnStatus, TurnView};
use leptos::prelude::*;

const SUGGESTIONS: [(&str, &str); 4] = [
    ("Build a tip calculator", "Build a single-file HTML tip calculator in the workspace (index.html), then summarize it."),
    ("Write a Rust function", "Write src/lib.rs with a function that checks if a string is a palindrome, plus a unit test."),
    ("Explain this harness", "In a few bullets: what can you do in this harness, and how do approvals and undo work?"),
    ("Plan a weekend trip", "Give me a short 2-day plan for a weekend in Montreal, as a table."),
];

#[component]
pub fn Thread(ctx: AppCtx) -> impl IntoView {
    let keys = Memo::new(move |_| ctx.transcript.with(|t| t.iter().map(|i| i.key()).collect::<Vec<_>>()));
    let empty = move || ctx.transcript.with(|t| t.is_empty()) && ctx.live_in_current().is_none();
    let on_scroll = move |_| {
        let pinned = crate::js::thread_distance_from_bottom() < 96;
        if ctx.pinned.get_untracked() != pinned {
            ctx.pinned.set(pinned);
        }
    };

    view! {
        <main class="thread" id="thread" on:scroll=on_scroll>
            <div class="thread-inner">
                <For each=move || keys.get() key=|k| k.clone() let(key)>
                    <ItemView ctx=ctx item_key=key />
                </For>
            </div>
            <Show when=empty>
                <EmptyState ctx=ctx />
            </Show>
        </main>
    }
}

#[component]
fn EmptyState(ctx: AppCtx) -> impl IntoView {
    let chips: Vec<AnyView> = SUGGESTIONS
        .iter()
        .map(|(label, prompt)| {
            let prompt = prompt.to_string();
            view! {
                <button class="suggest" on:click=move |_| agent::send(&ctx, prompt.clone())>
                    <span class="suggest-label">{*label}</span>
                    <span class="suggest-arrow" aria-hidden="true">"↗"</span>
                </button>
            }
            .into_any()
        })
        .collect();
    view! {
        <div class="empty-state">
            <div class="empty-mark" aria-hidden="true"><span></span></div>
            <p class="empty-title">"What should we build?"</p>
            <p class="empty-hint">"Streams through your proxy. Tools run in a phone-local workspace — you approve every write."</p>
            <div class="suggest-list">{chips}</div>
        </div>
    }
}

#[component]
fn ItemView(ctx: AppCtx, item_key: String) -> impl IntoView {
    let item = Memo::new(move |_| ctx.transcript.with(|t| t.iter().find(|i| i.key() == item_key).cloned()));
    move || {
        item.get().map(|it| match it {
            TranscriptItem::User { text, .. } => view! {
                <article class="msg msg-user">
                    <div class="bubble"><p class="msg-text">{text}</p></div>
                </article>
            }
            .into_any(),
            TranscriptItem::Note { text, .. } => view! {
                <div class="note" role="status"><span class="note-glyph" aria-hidden="true">"↶"</span>{text}</div>
            }
            .into_any(),
            TranscriptItem::Turn(turn) => turn_view(ctx, turn),
        })
    }
}

fn group_items(ctx: AppCtx, turn: &TurnView) -> Vec<AnyView> {
    let mut out = Vec::new();
    let mut tools: Vec<AnyView> = Vec::new();
    let flush = |tools: &mut Vec<AnyView>, out: &mut Vec<AnyView>| {
        if !tools.is_empty() {
            let cards = std::mem::take(tools);
            out.push(view! { <div class="timeline">{cards}</div> }.into_any());
        }
    };
    for item in &turn.items {
        match item {
            StepItem::Text { text, .. } => {
                flush(&mut tools, &mut out);
                out.push(render_markdown(ctx, text));
            }
            StepItem::Tool(v) => tools.push(tool_card(ctx, turn.turn.clone(), v.clone())),
        }
    }
    flush(&mut tools, &mut out);
    out
}

fn live_view(ctx: AppCtx, live: LiveTurn) -> AnyView {
    match live.phase {
        Phase::Streaming => {
            let status = if let Some(name) = &live.tool_name {
                Some(format!("Calling {name}…"))
            } else if live.text.is_empty() && live.reasoning_chars > 0 {
                Some("Thinking…".to_string())
            } else if live.text.is_empty() {
                Some(String::new())
            } else {
                None
            };
            view! {
                <div class="live" class:has-text=!live.text.is_empty()>
                    {(!live.text.is_empty()).then(|| render_markdown(ctx, &live.text))}
                    {status.map(|s| view! {
                        <div class="live-status" role="status">
                            <span class="typing" aria-hidden="true"><span></span><span></span><span></span></span>
                            {(!s.is_empty()).then(|| view! { <span class="live-label">{s.clone()}</span> })}
                        </div>
                    })}
                </div>
            }
            .into_any()
        }
        Phase::Running(name) => view! {
            <div class="live-status" role="status">
                <span class="typing" aria-hidden="true"><span></span><span></span><span></span></span>
                <span class="live-label">{format!("Running {name}…")}</span>
            </div>
        }
        .into_any(),
        Phase::Approval(_) => ().into_any(),
    }
}

fn turn_view(ctx: AppCtx, turn: TurnView) -> AnyView {
    let body = group_items(ctx, &turn);
    let turn_id = turn.turn.clone();
    let live_tail = {
        let turn_id = turn_id.clone();
        move || ctx.live.get().filter(|l| l.turn == turn_id).map(|l| live_view(ctx, l))
    };
    let can_retry = {
        let turn_id = turn_id.clone();
        Memo::new(move |_| ctx.live.with(|l| l.is_none()) && retryable_turn(&current_session(&ctx).events).as_deref() == Some(turn_id.as_str()))
    };
    let can_regen = {
        let turn_id = turn_id.clone();
        Memo::new(move |_| ctx.live.with(|l| l.is_none()) && regenerable_turn(&current_session(&ctx).events).as_deref() == Some(turn_id.as_str()))
    };
    // Copy = the reply text the user sees (tool cards excluded).
    let reply_text = turn
        .items
        .iter()
        .filter_map(|i| if let StepItem::Text { text, .. } = i { Some(text.trim()) } else { None })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let copy_src = (!reply_text.is_empty()).then_some(reply_text);
    let copy_btn = move || {
        copy_src.clone().map(|text| view! {
            <button class="foot-btn copy-btn" aria-label="Copy reply" on:click=move |_| js::copy_text(ctx, text.clone())>
                <span class="copy-glyph" aria-hidden="true"></span><span>"Copy"</span>
            </button>
        })
    };
    let retry_btn = move || {
        can_retry.get().then(|| view! {
            <button class="mini-btn retry-btn" on:click=move |_| agent::retry(&ctx)>"↻ Retry"</button>
        })
    };
    let regen_btn = move || {
        can_regen.get().then(|| view! {
            <button class="foot-btn regen-btn" aria-label="Regenerate reply" on:click=move |_| agent::regenerate(&ctx)>
                <span aria-hidden="true">"↻"</span><span>"Regenerate"</span>
            </button>
        })
    };
    let actions = move || view! { <div class="turn-actions">{copy_btn()}{regen_btn}</div> };
    let model_short = turn.model.rsplit('/').next().unwrap_or(&turn.model).to_string();
    let tools_n = turn.tool_count();
    let meta = {
        let mut parts = vec![model_short];
        if turn.steps > 1 {
            parts.push(format!("{} steps", turn.steps));
        }
        if tools_n > 0 {
            parts.push(format!("{tools_n} tool{}", if tools_n == 1 { "" } else { "s" }));
        }
        parts.push(turn.mode.label().to_string());
        parts.join(" · ")
    };
    let empty_done = turn.items.is_empty() && turn.status == TurnStatus::Done;
    let footer = match turn.status.clone() {
        TurnStatus::Running => ().into_any(),
        TurnStatus::Paused { reason } => view! {
            <div class="turn-foot">
                <div class="turn-status is-paused"><span class="pause-glyph" aria-hidden="true"></span>{format!("Paused · {reason}")}<span class="turn-hint">" — tap Resume below"</span></div>
            </div>
        }
        .into_any(),
        TurnStatus::Done => view! {
            <div class="turn-foot">
                {empty_done.then(|| view! { <div class="turn-status">"(no reply text)"</div> })}
                {actions}
                <div class="turn-meta">{meta.clone()}</div>
            </div>
        }
        .into_any(),
        TurnStatus::Cancelled => view! {
            <div class="turn-foot">
                <div class="turn-status is-warn">"Stopped — partial reply kept."</div>
                {retry_btn}
                {actions}
            </div>
        }
        .into_any(),
        TurnStatus::Interrupted { reason } => view! {
            <div class="turn-foot">
                <div class="turn-status is-warn">{reason}</div>
                {retry_btn}
            </div>
        }
        .into_any(),
        TurnStatus::Failed { reason } => view! {
            <div class="turn-foot">
                <div class="turn-status is-err">{reason}</div>
                {retry_btn}
            </div>
        }
        .into_any(),
        TurnStatus::StepLimit => view! {
            <div class="turn-foot">
                <div class="turn-status is-warn">"Stopped at the step limit. Send a message to keep going."</div>
                {actions}
                <div class="turn-meta">{meta.clone()}</div>
            </div>
        }
        .into_any(),
    };
    view! {
        <article class="msg msg-assistant" data-turn=turn_id>
            <div class="turn-body">{body}{live_tail}</div>
            {footer}
        </article>
    }
    .into_any()
}
