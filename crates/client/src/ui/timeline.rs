//! Turn activity timeline (OpenHands event-stream pattern, ported): every
//! tool call renders as an Action card paired with its Observation, on a
//! step rail. Write results carry a diff card with Aider-style Undo; calls
//! waiting on the approval policy carry the opencode Allow / Always / Deny
//! card.

use crate::agent;
use crate::state::{current_session, AppCtx, Artifact, Phase};
use harness_core::events::{DecisionBy, ToolView};
use harness_core::tools::{self, ToolKind};
use harness_core::workspace::{can_undo, diff_hunks, diff_stats, line_diff, DiffTag, FileEdit, Workspace};
use leptos::prelude::*;

const MAX_DIFF_ROWS: usize = 48;
/// Proposed diffs open collapsed to this many rows ("Show all" expands).
const PROPOSED_ROWS: usize = 12;
/// DOM sanity cap for "Show all" (the full file is one tap away in Open).
const PROPOSED_MAX_ROWS: usize = 1500;

/// Applied edits carry Undo; proposed ones (pre-approval) never do.
#[derive(Clone, Copy, PartialEq)]
enum DiffMode {
    Applied { seq: u64, undone: bool },
    Proposed,
}

fn status_of(view: &ToolView, awaiting: bool, running: bool) -> (&'static str, &'static str) {
    match (&view.result, view.decision) {
        (Some(r), _) if r.undone => ("undone", "Undone"),
        (Some(r), Some((false, _))) if !r.ok => ("denied", "Denied"),
        (Some(r), _) if r.ok => ("ok", "Done"),
        (Some(_), _) => ("err", "Error"),
        (None, _) if awaiting => ("ask", "Needs approval"),
        (None, _) if running => ("run", "Running"),
        (None, Some((false, _))) => ("denied", "Denied"),
        (None, _) => ("wait", "Waiting"),
    }
}

fn pretty_args(raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => {
            // Long file bodies are shown in the diff, not twice.
            let mut v = v;
            if let Some(obj) = v.as_object_mut() {
                for key in ["content", "search", "replace"] {
                    if let Some(serde_json::Value::String(s)) = obj.get(key) {
                        let lines = s.lines().count();
                        if lines > 6 || s.len() > 280 {
                            obj.insert(key.to_string(), serde_json::Value::String(format!("<{lines} lines>")));
                        }
                    }
                }
            }
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string())
        }
        Err(_) => raw.to_string(),
    }
}

pub fn tool_card(ctx: AppCtx, turn: String, view: ToolView) -> AnyView {
    let call_id = view.call.id.clone();
    let spec = tools::spec(&view.call.name);
    let kind_class = match spec.map(|s| s.kind) {
        Some(ToolKind::Write) => "act-kind write",
        _ => "act-kind read",
    };
    let title = spec.map(|s| s.title.to_string()).unwrap_or_else(|| view.call.name.clone());
    let args = view.call.args();
    let target = args
        .get("path")
        .or_else(|| args.get("query"))
        .and_then(|v| v.as_str())
        .map(|s| tools::truncate(s, 40))
        .unwrap_or_default();

    let awaiting = {
        let call_id = call_id.clone();
        let turn = turn.clone();
        Memo::new(move |_| {
            ctx.live.with(|l| {
                l.as_ref().is_some_and(|l| l.turn == turn && matches!(&l.phase, Phase::Approval(c) if c.id == call_id))
            })
        })
    };
    let running = {
        let turn = turn.clone();
        let name = view.call.name.clone();
        let has_result = view.result.is_some();
        Memo::new(move |_| {
            !has_result
                && ctx.live.with(|l| l.as_ref().is_some_and(|l| l.turn == turn && l.phase == Phase::Running(name.clone())))
        })
    };
    let expanded = {
        let call_id = call_id.clone();
        Memo::new(move |_| ctx.expanded.with(|e| e.contains(&call_id)))
    };
    let toggle = {
        let call_id = call_id.clone();
        move |_| {
            ctx.expanded.update(|e| {
                if !e.remove(&call_id) {
                    e.insert(call_id.clone());
                }
            })
        }
    };

    let view_for_status = view.clone();
    let status = Memo::new(move |_| status_of(&view_for_status, awaiting.get(), running.get()));
    let decided_by = view.decision.map(|(allow, by)| {
        let who = match by {
            DecisionBy::Policy => "by policy",
            DecisionBy::User => "by you",
            DecisionBy::UserAlways => "by you · always",
        };
        format!("{} {who}", if allow { "Allowed" } else { "Denied" })
    });
    let action_text = pretty_args(&view.call.arguments);
    let observation = view.result.as_ref().map(|r| r.output.clone());
    let step_label = format!("Step {}", view.step);
    let diff = view
        .result
        .as_ref()
        .and_then(|r| r.edit.clone().map(|e| (r.seq, r.undone, e)))
        .map(|(seq, undone, edit)| diff_card(ctx, seq, undone, edit));

    let approval = {
        let call_id = call_id.clone();
        let name = view.call.name.clone();
        let call = view.call.clone();
        move || {
            awaiting.get().then(|| {
                let (a, b, c) = (call_id.clone(), call_id.clone(), call_id.clone());
                // Pure dry run against the workspace as it is right now:
                // exactly the edit Allow would log (codex / opencode show the
                // change before asking).
                let ws = Workspace::derive(&current_session(&ctx).events);
                let outcome = tools::execute(&call, &ws);
                let proposal = match (outcome.ok, outcome.edit) {
                    (true, Some(edit)) => diff_view(ctx, edit, DiffMode::Proposed),
                    (true, None) => ().into_any(),
                    (false, _) => view! {
                        <div class="approve-warn" role="note">{format!("This call will fail: {}", outcome.output)}</div>
                    }
                    .into_any(),
                };
                view! {
                    <div class="approve" role="group" aria-label="Approve tool call">
                        <div class="approve-q">
                            "Allow " <code class="ic">{name.clone()}</code> " to make this change?"
                        </div>
                        <div class="approve-diff">{proposal}</div>
                        <div class="approve-row">
                            <button class="approve-btn yes" on:click=move |_| agent::answer_approval(&ctx, &a, true, false)>"Allow once"</button>
                            <button class="approve-btn always" on:click=move |_| agent::answer_approval(&ctx, &b, true, true)>"Always allow"</button>
                            <button class="approve-btn no" on:click=move |_| agent::answer_approval(&ctx, &c, false, false)>"Deny"</button>
                        </div>
                    </div>
                }
            })
        }
    };

    view! {
        <div class="act" class:is-open=move || expanded.get() data-status=move || status.get().0>
            <span class="act-dot" aria-hidden="true"></span>
            <button class="act-head" on:click=toggle aria-expanded=move || expanded.get().to_string()>
                <span class=kind_class aria-hidden="true"></span>
                <span class="act-title">{title}</span>
                <span class="act-target">{target}</span>
                <span class="act-badge">{move || status.get().1}</span>
            </button>
            {approval}
            <Show when=move || expanded.get()>
                <div class="act-body">
                    <div class="act-label">{step_label.clone()} " · Action"</div>
                    <pre class="act-pre">{action_text.clone()}</pre>
                    {decided_by.clone().map(|d| view! { <div class="act-decision">{d}</div> })}
                    <div class="act-label">"Observation"</div>
                    <pre class="act-pre">{observation.clone().unwrap_or_else(|| "(pending)".to_string())}</pre>
                </div>
            </Show>
            {diff}
        </div>
    }
    .into_any()
}

pub fn diff_card(ctx: AppCtx, seq: u64, undone: bool, edit: FileEdit) -> AnyView {
    diff_view(ctx, edit, DiffMode::Applied { seq, undone })
}

fn diff_rows(hunks: &[Option<harness_core::workspace::DiffLine>], limit: usize) -> Vec<AnyView> {
    hunks
        .iter()
        .take(limit)
        .map(|h| match h {
            None => view! { <div class="dl gap">"⋯"</div> }.into_any(),
            Some(line) => {
                let (class, sign) = match line.tag {
                    DiffTag::Add => ("dl add", "+"),
                    DiffTag::Del => ("dl del", "−"),
                    DiffTag::Same => ("dl", " "),
                };
                view! { <div class=class><span class="dl-sign">{sign}</span><span class="dl-text">{line.text.clone()}</span></div> }.into_any()
            }
        })
        .collect()
}

fn diff_view(ctx: AppCtx, edit: FileEdit, mode: DiffMode) -> AnyView {
    let proposed = mode == DiffMode::Proposed;
    let (seq, undone) = match mode {
        DiffMode::Applied { seq, undone } => (seq, undone),
        DiffMode::Proposed => (0, false),
    };
    let before = edit.before.clone().unwrap_or_default();
    let after = edit.after.clone().unwrap_or_default();
    let lines = line_diff(&before, &after);
    let (add, del) = diff_stats(&lines);
    let is_delete = edit.before.is_some() && edit.after.is_none();
    let hunks = diff_hunks(&lines, 2);
    let total = hunks.len();
    let show_all = RwSignal::new(false);
    let rows = {
        let hunks = hunks.clone();
        move || {
            if proposed && is_delete {
                // delete_file: filename + line count, in red — not every line.
                let n = before.lines().count();
                return vec![view! {
                    <div class="dl del whole"><span class="dl-sign">"−"</span><span class="dl-text">{format!("Deletes the whole file · {n} line{}", if n == 1 { "" } else { "s" })}</span></div>
                }
                .into_any()];
            }
            let limit = match (proposed, show_all.get()) {
                (true, false) => PROPOSED_ROWS,
                (true, true) => PROPOSED_MAX_ROWS,
                (false, _) => MAX_DIFF_ROWS,
            };
            let mut out = diff_rows(&hunks, limit);
            if total > limit && !(proposed && !show_all.get()) {
                out.push(view! { <div class="dl gap">{format!("… {} more lines", total - limit)}</div> }.into_any());
            }
            out
        }
    };
    let verb = match (&edit.before, &edit.after, proposed) {
        (None, Some(_), false) => "Created",
        (Some(_), None, false) => "Deleted",
        (_, _, false) => "Edited",
        (None, Some(_), true) => "Will create",
        (Some(_), None, true) => "Will delete",
        (_, _, true) => "Will edit",
    };
    let undoable = Memo::new(move |_| {
        let busy = ctx.live.with(|l| l.is_some());
        !proposed && !busy && can_undo(&current_session(&ctx).events, seq)
    });
    let path = edit.path.clone();
    let is_html = path.ends_with(".html") || path.ends_with(".htm");
    let open_artifact = {
        let after = after.clone();
        let path = path.clone();
        move |_| {
            ctx.artifact.set(Some(if is_html {
                Artifact::Html { title: path.clone(), body: after.clone() }
            } else {
                let lang = path.rsplit('.').next().unwrap_or("text").to_string();
                Artifact::Code { lang, body: after.clone() }
            }))
        }
    };
    let has_after = edit.after.is_some() && !undone;
    view! {
        <div class="diff" class:is-undone=undone class:is-proposed=proposed class:is-delete=is_delete class:is-all=move || show_all.get()>
            <div class="diff-head">
                <span class="diff-verb">{verb}</span>
                <span class="diff-path">{path.clone()}</span>
                <span class="diff-stat"><span class="add" class:is-zero=add == 0>"+" {add}</span> <span class="del" class:is-zero=del == 0>"−" {del}</span></span>
            </div>
            <div class="diff-body">{rows}</div>
            {move || (proposed && !is_delete && !show_all.get() && total > PROPOSED_ROWS).then(|| view! {
                <button class="show-all" on:click=move |_| show_all.set(true)>{format!("Show all · {} more lines", total - PROPOSED_ROWS)}</button>
            })}
            <div class="diff-foot">
                {has_after.then(|| view! {
                    <button class="mini-btn" on:click=open_artifact.clone()>{if is_html { "Preview" } else { "Open" }}</button>
                })}
                {move || if proposed {
                    view! { <span class="diff-undone">"Not applied yet"</span> }.into_any()
                } else if undone {
                    view! { <span class="diff-undone">"Undone"</span> }.into_any()
                } else if undoable.get() {
                    view! { <button class="mini-btn undo-btn" on:click=move |_| agent::undo(&ctx, seq)>"↶ Undo edit"</button> }.into_any()
                } else {
                    ().into_any()
                }}
            </div>
        </div>
    }
    .into_any()
}
