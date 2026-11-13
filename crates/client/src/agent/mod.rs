//! Turn driver (DESIGN.md §1, §6, §8 bar 3).
//!
//! The loop itself is `harness_core::agent_loop::next_action` — a pure
//! function of the append-only log (codex-style tool loop). This driver only
//! performs the side effects it asks for: stream a model step, apply the
//! approval policy (codex mode + opencode per-tool rules), run a browser-side
//! tool, and append the resulting event. Pause / Resume (goose), Cancel,
//! Retry, and Undo (aider) are all new events, never edits.

use crate::net::{self, NetError, Outcome as NetOutcome};
use crate::state::{append_event, session_events, with_session, AppCtx, LiveTurn, Phase};
use crate::storage;
use harness_core::agent_loop::{next_action, LoopConfig, NextAction};
use harness_core::context::{assemble, DEFAULT_BUDGET_TOKENS};
use harness_core::events::{open_turn, regenerable_turn, retryable_turn, turn_live_edits, DecisionBy, EventKind, Outcome};
use harness_core::permissions::{decide as policy, Permission};
use harness_core::sse::ChunkDelta;
use harness_core::tools::{self, ToolCallAccumulator};
use harness_core::workspace::{can_undo, Workspace};
use leptos::prelude::*;
use std::cell::{Cell, RefCell};

const DENIED_OUTPUT: &str = "Denied: this tool call was not allowed. Do not retry it; continue without it or ask the user.";

/// Send a user message and start a turn.
pub fn send(ctx: &AppCtx, text: String) {
    if ctx.live.get_untracked().is_some() {
        return;
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    let session_id = ctx.current_id.get_untracked();
    // A paused turn the user moved on from is closed, not silently resumed.
    if let Some(open) = open_turn(&session_events(ctx, &session_id)) {
        append_event(ctx, &session_id, EventKind::TurnEnd { turn: open.turn, outcome: Outcome::Cancelled });
    }
    with_session(ctx, &session_id, |session| {
        if session.title.is_empty() || session.title == storage::NEW_CHAT_TITLE {
            session.title = storage::title_from(&text);
        }
    });
    append_event(ctx, &session_id, EventKind::UserMessage { id: storage::uid("m"), text });
    start_turn(ctx, &session_id, None);
}

fn start_turn(ctx: &AppCtx, session_id: &str, retry_of: Option<String>) {
    let turn = storage::uid("t");
    append_event(
        ctx,
        session_id,
        EventKind::TurnStart {
            turn: turn.clone(),
            model: ctx.model.get_untracked(),
            mode: ctx.mode.get_untracked(),
            retry_of,
        },
    );
    ctx.pinned.set(true);
    spawn_drive(*ctx, session_id.to_string(), turn);
}

/// Cancel: abort the stream (keeps the partial) or drop a pending approval;
/// the turn closes as Cancelled and the composer returns.
pub fn cancel(ctx: &AppCtx) {
    let Some(live) = ctx.live.get_untracked() else { return };
    match &live.abort {
        Some(abort) if live.phase == Phase::Streaming => abort.abort(),
        _ => {
            append_event(ctx, &live.session_id, EventKind::TurnEnd { turn: live.turn.clone(), outcome: Outcome::Cancelled });
            ctx.live.set(None);
        }
    }
}

/// Pause (goose): stop the stream but keep the turn open and resumable.
pub fn pause(ctx: &AppCtx) {
    let Some(live) = ctx.live.get_untracked() else { return };
    match &live.abort {
        Some(abort) if live.phase == Phase::Streaming => {
            ctx.live.update(|l| {
                if let Some(l) = l {
                    l.pause_requested = true;
                }
            });
            abort.abort();
        }
        _ => {
            append_event(ctx, &live.session_id, EventKind::TurnPause { turn: live.turn.clone(), reason: "you paused".into() });
            ctx.live.set(None);
        }
    }
}

/// Resume a paused turn in the current session exactly where it stopped.
pub fn resume(ctx: &AppCtx) {
    if ctx.live.get_untracked().is_some() {
        return;
    }
    let session_id = ctx.current_id.get_untracked();
    let Some(open) = open_turn(&session_events(ctx, &session_id)) else { return };
    if !open.paused {
        return;
    }
    append_event(ctx, &session_id, EventKind::TurnResume { turn: open.turn.clone() });
    ctx.pinned.set(true);
    spawn_drive(*ctx, session_id, open.turn);
}

/// Retry the last turn if it failed / was interrupted / cancelled. Appends a
/// new turn that supersedes it; the user event is not duplicated.
pub fn retry(ctx: &AppCtx) {
    if ctx.live.get_untracked().is_some() {
        return;
    }
    let session_id = ctx.current_id.get_untracked();
    if let Some(old) = retryable_turn(&session_events(ctx, &session_id)) {
        start_turn(ctx, &session_id, Some(old));
    }
}

/// Claude-style Regenerate of the latest finished turn: its file edits are
/// undone first (newest first, as `edit/undo` events) so the new attempt
/// starts from the same workspace, then a fresh turn is started with
/// `retry_of` — no new user event, the old attempt drops out of history.
pub fn regenerate(ctx: &AppCtx) {
    if ctx.live.get_untracked().is_some() {
        return;
    }
    let session_id = ctx.current_id.get_untracked();
    let events = session_events(ctx, &session_id);
    let Some(old) = regenerable_turn(&events) else { return };
    for seq in turn_live_edits(&events, &old) {
        if can_undo(&session_events(ctx, &session_id), seq) {
            append_event(ctx, &session_id, EventKind::EditUndo { edit_seq: seq });
        }
    }
    start_turn(ctx, &session_id, Some(old));
}

/// The user's answer on an approval card.
pub fn answer_approval(ctx: &AppCtx, call_id: &str, allow: bool, always: bool) {
    let Some(live) = ctx.live.get_untracked() else { return };
    let Phase::Approval(call) = &live.phase else { return };
    if call.id != call_id {
        return;
    }
    if always && allow {
        // opencode "always": persist a per-tool rule.
        ctx.rules.update(|r| r.set(&call.name, Some(Permission::Allow)));
        storage::save_rules(&ctx.rules.get_untracked(), ctx);
    }
    let by = if always { DecisionBy::UserAlways } else { DecisionBy::User };
    append_event(ctx, &live.session_id, EventKind::ToolDecision { turn: live.turn.clone(), call_id: call.id.clone(), allow, by });
    spawn_drive(*ctx, live.session_id.clone(), live.turn.clone());
}

/// Aider-style undo of one agent edit (appends `edit/undo`).
pub fn undo(ctx: &AppCtx, edit_seq: u64) {
    if ctx.live.get_untracked().is_some() {
        return;
    }
    let session_id = ctx.current_id.get_untracked();
    if can_undo(&session_events(ctx, &session_id), edit_seq) {
        append_event(ctx, &session_id, EventKind::EditUndo { edit_seq });
    }
}

fn spawn_drive(ctx: AppCtx, session_id: String, turn: String) {
    wasm_bindgen_futures::spawn_local(async move {
        drive(&ctx, &session_id, &turn).await;
        crate::js::schedule_scroll_to_bottom(&ctx);
    });
}

fn set_live(ctx: &AppCtx, session_id: &str, turn: &str, step: u32, phase: Phase, abort: Option<web_sys::AbortController>) {
    ctx.live.set(Some(LiveTurn {
        session_id: session_id.to_string(),
        turn: turn.to_string(),
        step,
        phase,
        text: String::new(),
        reasoning_chars: 0,
        tool_name: None,
        abort,
        pause_requested: false,
    }));
}

async fn drive(ctx: &AppCtx, session_id: &str, turn: &str) {
    let cfg = LoopConfig::default();
    // Hard guard against a runaway loop even if the log were malformed.
    for _ in 0..64 {
        let events = session_events(ctx, session_id);
        match next_action(&events, turn, &cfg) {
            NextAction::CallModel { step, .. } => {
                if !stream_step(ctx, session_id, turn, step).await {
                    return;
                }
            }
            NextAction::Decide(call) => {
                let decision = match tools::spec(&call.name) {
                    None => Permission::Deny,
                    Some(spec) => policy(spec, ctx.mode.get_untracked(), &ctx.rules.get_untracked()),
                };
                match decision {
                    Permission::Allow | Permission::Deny => {
                        append_event(
                            ctx,
                            session_id,
                            EventKind::ToolDecision {
                                turn: turn.to_string(),
                                call_id: call.id.clone(),
                                allow: decision == Permission::Allow,
                                by: DecisionBy::Policy,
                            },
                        );
                    }
                    Permission::Ask => {
                        let step = ctx.live.get_untracked().map(|l| l.step).unwrap_or(1);
                        set_live(ctx, session_id, turn, step, Phase::Approval(call), None);
                        ctx.pinned.set(true);
                        return; // resumes from answer_approval()
                    }
                }
            }
            NextAction::Run(call) => {
                let step = ctx.live.get_untracked().map(|l| l.step).unwrap_or(1);
                set_live(ctx, session_id, turn, step, Phase::Running(call.name.clone()), None);
                let outcome = tools::execute(&call, &Workspace::derive(&events));
                append_event(
                    ctx,
                    session_id,
                    EventKind::ToolResult {
                        turn: turn.to_string(),
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        ok: outcome.ok,
                        output: outcome.output,
                        edit: outcome.edit,
                    },
                );
            }
            NextAction::Refuse(call) => {
                append_event(
                    ctx,
                    session_id,
                    EventKind::ToolResult {
                        turn: turn.to_string(),
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        ok: false,
                        output: DENIED_OUTPUT.to_string(),
                        edit: None,
                    },
                );
            }
            NextAction::Finish(outcome) => {
                append_event(ctx, session_id, EventKind::TurnEnd { turn: turn.to_string(), outcome });
                ctx.live.set(None);
                return;
            }
            NextAction::Wait => {
                ctx.live.set(None);
                return;
            }
        }
    }
    append_event(ctx, session_id, EventKind::TurnEnd { turn: turn.to_string(), outcome: Outcome::StepLimit });
    ctx.live.set(None);
}

/// Stream one model step and settle it into the log. Returns true when the
/// loop should continue (the step settled cleanly).
async fn stream_step(ctx: &AppCtx, session_id: &str, turn: &str, step: u32) -> bool {
    let events = session_events(ctx, session_id);
    let workspace = Workspace::derive(&events);
    let plan = assemble(&events, &workspace, DEFAULT_BUDGET_TOKENS);
    let mut body = serde_json::json!({
        "model": ctx.model.get_untracked(),
        "messages": plan.messages,
        "stream": true,
    });
    if ctx.tools_enabled.get_untracked() {
        body["tools"] = tools::tools_json();
    }
    let settings = ctx.settings.get_untracked();

    let abort = web_sys::AbortController::new().expect("AbortController");
    set_live(ctx, session_id, turn, step, Phase::Streaming, Some(abort.clone()));
    crate::js::schedule_scroll_to_bottom(ctx);

    let text = RefCell::new(String::new());
    let acc = RefCell::new(ToolCallAccumulator::default());
    let last_checkpoint = Cell::new(0.0f64);
    let live = ctx.live;
    let (cp_session, cp_turn) = (session_id.to_string(), turn.to_string());
    let on_chunk = |chunk: ChunkDelta| {
        let mut changed_tool = false;
        for frag in &chunk.tool_calls {
            acc.borrow_mut().push(frag);
            changed_tool = true;
        }
        let content = chunk.content.filter(|c| !c.is_empty());
        if let Some(c) = &content {
            text.borrow_mut().push_str(c);
        }
        let reasoning = chunk.reasoning.map(|r| r.chars().count()).unwrap_or(0);
        if content.is_none() && reasoning == 0 && !changed_tool {
            return;
        }
        let tool_name = if changed_tool { acc.borrow().current_name() } else { None };
        live.update(|l| {
            if let Some(l) = l {
                if let Some(c) = &content {
                    l.text.push_str(c);
                }
                l.reasoning_chars += reasoning;
                if tool_name.is_some() {
                    l.tool_name = tool_name;
                }
            }
        });
        // Reload safety net (goose): checkpoint the partial ~once a second.
        let now = js_sys::Date::now();
        if content.is_some() && now - last_checkpoint.get() > 1000.0 {
            last_checkpoint.set(now);
            storage::save_live_checkpoint(&cp_session, &cp_turn, step, &text.borrow());
        }
    };

    let result = net::stream_chat(&settings, &body, &on_chunk, &abort).await;
    storage::clear_live_checkpoint();
    let text = text.into_inner();
    let calls = acc.into_inner().finish(&turn.chars().rev().take(6).collect::<String>());
    let pause_requested = ctx.live.get_untracked().map(|l| l.pause_requested).unwrap_or(false);
    let turn_s = turn.to_string();
    let settle_partial = |ctx: &AppCtx, text: &str| {
        if !text.is_empty() {
            append_event(
                ctx,
                session_id,
                EventKind::AssistantMessage { turn: turn_s.clone(), step, text: text.to_string(), tool_calls: Vec::new(), partial: true },
            );
        }
    };
    let end = |ctx: &AppCtx, outcome: Outcome| {
        append_event(ctx, session_id, EventKind::TurnEnd { turn: turn.to_string(), outcome });
        ctx.live.set(None);
    };

    // goose: a page teardown is a pause, never a failure — the turn stays
    // open and resumable after reload.
    if crate::js::page_hiding() && !matches!(result, Ok(NetOutcome::Done)) {
        // Synchronous net first: the IndexedDB commit below is async and may
        // not land before teardown; boot recovers from this checkpoint.
        storage::save_live_checkpoint(session_id, turn, step, &text);
        settle_partial(ctx, &text);
        append_event(ctx, session_id, EventKind::TurnPause { turn: turn.to_string(), reason: "page reloaded".into() });
        ctx.live.set(None);
        return false;
    }

    match result {
        Ok(NetOutcome::Done) => {
            append_event(
                ctx,
                session_id,
                EventKind::AssistantMessage { turn: turn.to_string(), step, text, tool_calls: calls, partial: false },
            );
            true
        }
        Ok(NetOutcome::Cancelled) => {
            settle_partial(ctx, &text);
            if pause_requested {
                append_event(ctx, session_id, EventKind::TurnPause { turn: turn.to_string(), reason: "you paused".into() });
                ctx.live.set(None);
            } else {
                end(ctx, Outcome::Cancelled);
            }
            false
        }
        Err(NetError::Unauthorized) => {
            settle_partial(ctx, &text);
            end(ctx, Outcome::Failed { reason: "Connect required — missing or wrong access token.".into() });
            ctx.settings_open.set(true);
            false
        }
        Err(NetError::Unreachable(reason)) => {
            if text.is_empty() {
                end(ctx, Outcome::Failed { reason: format!("Proxy unreachable — {reason}") });
            } else {
                settle_partial(ctx, &text);
                end(ctx, Outcome::Interrupted { reason: "The stream dropped. Retry continues from your last message.".into() });
            }
            false
        }
        Err(NetError::Api { message, .. }) => {
            if text.is_empty() {
                end(ctx, Outcome::Failed { reason: message });
            } else {
                settle_partial(ctx, &text);
                end(ctx, Outcome::Interrupted { reason: message });
            }
            false
        }
    }
}
