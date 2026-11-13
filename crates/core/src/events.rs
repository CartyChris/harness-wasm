//! Append-only session event log (DeepSeek-Harness pattern, ported).
//!
//! The log is the single source of truth for a session. Nothing in it is
//! ever edited or removed: retry, undo, pause, and resume are *new events*.
//! Two read-only projections are derived from it:
//! - `derive_messages` — the exact model history sent to `/api/chat`
//!   (DeepSeek's `deriveMessages()`): failed and retried attempts stay in the
//!   log but add no model history.
//! - `derive_transcript` — what the thread renders, including the
//!   Action → Observation pairs of each turn (OpenHands' event stream view).

use crate::permissions::ApprovalMode;
use crate::tools::ToolCall;
use crate::types::Message;
use crate::workspace::{undone_set, FileEdit};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const RESUME_NOTE: &str =
    "Continue your previous reply exactly where it stopped. Do not repeat anything you already wrote.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub at: f64,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecisionBy {
    /// The approval mode / per-tool rule decided without asking.
    Policy,
    /// The user tapped Allow once / Deny.
    User,
    /// The user tapped "Always allow" (also writes a per-tool rule).
    UserAlways,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum Outcome {
    Done,
    Cancelled,
    Failed { reason: String },
    Interrupted { reason: String },
    StepLimit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum EventKind {
    #[serde(rename = "user/message")]
    UserMessage { id: String, text: String },
    #[serde(rename = "turn/start")]
    TurnStart {
        turn: String,
        model: String,
        #[serde(default)]
        mode: ApprovalMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_of: Option<String>,
    },
    /// A settled model step. `partial` = the stream stopped early (pause,
    /// cancel, drop); partial steps never carry tool calls.
    #[serde(rename = "assistant/message")]
    AssistantMessage {
        turn: String,
        step: u32,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
        #[serde(default)]
        partial: bool,
    },
    #[serde(rename = "tool/decision")]
    ToolDecision { turn: String, call_id: String, allow: bool, by: DecisionBy },
    #[serde(rename = "tool/result")]
    ToolResult {
        turn: String,
        call_id: String,
        name: String,
        ok: bool,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edit: Option<FileEdit>,
    },
    #[serde(rename = "turn/pause")]
    TurnPause { turn: String, reason: String },
    #[serde(rename = "turn/resume")]
    TurnResume { turn: String },
    #[serde(rename = "turn/end")]
    TurnEnd { turn: String, outcome: Outcome },
    #[serde(rename = "edit/undo")]
    EditUndo { edit_seq: u64 },
}

impl EventKind {
    /// The durable event name (DeepSeek-style `domain/verb`).
    pub fn name(&self) -> &'static str {
        match self {
            EventKind::UserMessage { .. } => "user/message",
            EventKind::TurnStart { .. } => "turn/start",
            EventKind::AssistantMessage { .. } => "assistant/message",
            EventKind::ToolDecision { .. } => "tool/decision",
            EventKind::ToolResult { .. } => "tool/result",
            EventKind::TurnPause { .. } => "turn/pause",
            EventKind::TurnResume { .. } => "turn/resume",
            EventKind::TurnEnd { .. } => "turn/end",
            EventKind::EditUndo { .. } => "edit/undo",
        }
    }

    pub fn turn(&self) -> Option<&str> {
        match self {
            EventKind::TurnStart { turn, .. }
            | EventKind::AssistantMessage { turn, .. }
            | EventKind::ToolDecision { turn, .. }
            | EventKind::ToolResult { turn, .. }
            | EventKind::TurnPause { turn, .. }
            | EventKind::TurnResume { turn }
            | EventKind::TurnEnd { turn, .. } => Some(turn),
            _ => None,
        }
    }

    /// One-line summary for the event-log inspector.
    pub fn summary(&self) -> String {
        use crate::tools::truncate;
        match self {
            EventKind::UserMessage { text, .. } => truncate(text.trim(), 70),
            EventKind::TurnStart { model, mode, retry_of, .. } => match retry_of {
                Some(old) => format!("{model} · {} · retry of {old}", mode.label()),
                None => format!("{model} · {}", mode.label()),
            },
            EventKind::AssistantMessage { step, text, tool_calls, partial, .. } => {
                let mut s = format!("step {step}");
                if !text.is_empty() {
                    s.push_str(&format!(" · {} chars", text.chars().count()));
                }
                if !tool_calls.is_empty() {
                    let names: Vec<&str> = tool_calls.iter().map(|c| c.name.as_str()).collect();
                    s.push_str(&format!(" · calls {}", names.join(", ")));
                }
                if *partial {
                    s.push_str(" · partial");
                }
                s
            }
            EventKind::ToolDecision { call_id, allow, by, .. } => format!(
                "{} {} ({})",
                if *allow { "allow" } else { "deny" },
                truncate(call_id, 18),
                match by {
                    DecisionBy::Policy => "policy",
                    DecisionBy::User => "you",
                    DecisionBy::UserAlways => "you · always",
                }
            ),
            EventKind::ToolResult { name, ok, output, edit, .. } => {
                let mut s = format!("{name} {}", if *ok { "ok" } else { "error" });
                if let Some(edit) = edit {
                    s.push_str(&format!(" · edit {}", edit.path));
                } else {
                    s.push_str(&format!(" · {}", truncate(output.lines().next().unwrap_or(""), 40)));
                }
                s
            }
            EventKind::TurnPause { reason, .. } => format!("paused ({reason})"),
            EventKind::TurnResume { .. } => "resumed".to_string(),
            EventKind::TurnEnd { outcome, .. } => match outcome {
                Outcome::Done => "done".into(),
                Outcome::Cancelled => "cancelled".into(),
                Outcome::Failed { reason } => format!("failed · {}", truncate(reason, 50)),
                Outcome::Interrupted { reason } => format!("interrupted · {}", truncate(reason, 50)),
                Outcome::StepLimit => "step limit reached".into(),
            },
            EventKind::EditUndo { edit_seq } => format!("undo edit #{edit_seq}"),
        }
    }
}

/// Next sequence number for an append.
pub fn next_seq(events: &[Event]) -> u64 {
    events.last().map(|e| e.seq + 1).unwrap_or(1)
}

/// The only way to grow a log.
pub fn append(events: &mut Vec<Event>, kind: EventKind, at: f64) -> u64 {
    let seq = next_seq(events);
    events.push(Event { seq, at, kind });
    seq
}

// --- model history ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireFunction,
}

/// OpenAI-compatible chat message (what `/api/chat` forwards upstream).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<WireToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl WireMessage {
    pub fn text(role: &str, content: impl Into<String>) -> Self {
        WireMessage { role: role.into(), content: Some(content.into()), tool_calls: Vec::new(), tool_call_id: None }
    }

    /// Rough size for context budgeting (chars of content + call args).
    pub fn chars(&self) -> usize {
        self.content.as_deref().map(str::len).unwrap_or(0)
            + self.tool_calls.iter().map(|c| c.function.arguments.len() + c.function.name.len()).sum::<usize>()
            + 8
    }
}

/// Turns whose attempts add no model history: failed turns and turns a
/// later `turn/start` retried.
fn excluded_turns(events: &[Event]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for e in events {
        match &e.kind {
            EventKind::TurnStart { retry_of: Some(old), .. } => {
                out.insert(old.clone());
            }
            EventKind::TurnEnd { turn, outcome: Outcome::Failed { .. } } => {
                out.insert(turn.clone());
            }
            _ => {}
        }
    }
    out
}

fn edit_path(events: &[Event], seq: u64) -> Option<String> {
    events.iter().find_map(|e| match &e.kind {
        EventKind::ToolResult { edit: Some(edit), .. } if e.seq == seq => Some(edit.path.clone()),
        _ => None,
    })
}

/// Project the model history from the log (DeepSeek `deriveMessages()`).
pub fn derive_messages(events: &[Event]) -> Vec<WireMessage> {
    let excluded = excluded_turns(events);
    let answered: BTreeSet<&str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    let mut included_calls: BTreeSet<String> = BTreeSet::new();
    let mut last_partial: BTreeMap<String, bool> = BTreeMap::new();
    let mut out = Vec::new();
    for e in events {
        match &e.kind {
            EventKind::UserMessage { text, .. } => out.push(WireMessage::text("user", text.clone())),
            EventKind::AssistantMessage { turn, text, tool_calls, partial, .. } => {
                if excluded.contains(turn) {
                    continue;
                }
                last_partial.insert(turn.clone(), *partial);
                // Calls without a result (turn cancelled mid-approval) would
                // make the upstream reject the history: drop them.
                let calls: Vec<WireToolCall> = tool_calls
                    .iter()
                    .filter(|c| !*partial && answered.contains(c.id.as_str()))
                    .map(|c| WireToolCall {
                        id: c.id.clone(),
                        kind: "function".into(),
                        function: WireFunction { name: c.name.clone(), arguments: c.arguments.clone() },
                    })
                    .collect();
                if text.is_empty() && calls.is_empty() {
                    continue;
                }
                for c in &calls {
                    included_calls.insert(c.id.clone());
                }
                out.push(WireMessage {
                    role: "assistant".into(),
                    content: if text.is_empty() { None } else { Some(text.clone()) },
                    tool_calls: calls,
                    tool_call_id: None,
                });
            }
            EventKind::ToolResult { turn, call_id, ok, output, .. } => {
                if excluded.contains(turn) || !included_calls.contains(call_id) {
                    continue;
                }
                let content = if *ok { output.clone() } else { format!("error: {output}") };
                out.push(WireMessage {
                    role: "tool".into(),
                    content: Some(content),
                    tool_calls: Vec::new(),
                    tool_call_id: Some(call_id.clone()),
                });
            }
            EventKind::TurnResume { turn } => {
                if !excluded.contains(turn) && last_partial.get(turn).copied().unwrap_or(false) {
                    out.push(WireMessage::text("user", RESUME_NOTE));
                }
            }
            EventKind::EditUndo { edit_seq } => {
                if let Some(path) = edit_path(events, *edit_seq) {
                    out.push(WireMessage::text(
                        "user",
                        format!("(Harness note: I undid your last edit to {path}; that file is back to its previous content.)"),
                    ));
                }
            }
            _ => {}
        }
    }
    out
}

// --- transcript (what the thread renders) -----------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum TurnStatus {
    Running,
    Paused { reason: String },
    Done,
    Cancelled,
    Failed { reason: String },
    Interrupted { reason: String },
    StepLimit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResultView {
    pub seq: u64,
    pub ok: bool,
    pub output: String,
    pub edit: Option<FileEdit>,
    pub undone: bool,
}

/// One Action (the call) paired with its Observation (the result).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolView {
    pub step: u32,
    pub call: ToolCall,
    pub decision: Option<(bool, DecisionBy)>,
    pub result: Option<ToolResultView>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StepItem {
    Text { seq: u64, text: String },
    Tool(ToolView),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnView {
    pub turn: String,
    pub start_seq: u64,
    pub model: String,
    pub mode: ApprovalMode,
    pub items: Vec<StepItem>,
    pub steps: u32,
    pub status: TurnStatus,
    pub retry_of: Option<String>,
}

impl TurnView {
    pub fn tool_count(&self) -> usize {
        self.items.iter().filter(|i| matches!(i, StepItem::Tool(_))).count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptItem {
    User { seq: u64, id: String, text: String },
    Turn(TurnView),
    Note { seq: u64, text: String },
}

impl TranscriptItem {
    pub fn key(&self) -> String {
        match self {
            TranscriptItem::User { seq, .. } => format!("u{seq}"),
            TranscriptItem::Turn(t) => format!("t{}", t.start_seq),
            TranscriptItem::Note { seq, .. } => format!("n{seq}"),
        }
    }
}

pub fn derive_transcript(events: &[Event]) -> Vec<TranscriptItem> {
    let retried: BTreeSet<String> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::TurnStart { retry_of: Some(old), .. } => Some(old.clone()),
            _ => None,
        })
        .collect();
    let undone = undone_set(events);
    let mut items: Vec<TranscriptItem> = Vec::new();
    let mut turn_index: BTreeMap<String, usize> = BTreeMap::new();

    fn turn_mut<'a>(items: &'a mut [TranscriptItem], index: &BTreeMap<String, usize>, turn: &str) -> Option<&'a mut TurnView> {
        let i = *index.get(turn)?;
        match items.get_mut(i) {
            Some(TranscriptItem::Turn(t)) => Some(t),
            _ => None,
        }
    }
    fn tool_mut<'a>(t: &'a mut TurnView, call_id: &str) -> Option<&'a mut ToolView> {
        t.items.iter_mut().rev().find_map(|i| match i {
            StepItem::Tool(v) if v.call.id == call_id => Some(v),
            _ => None,
        })
    }

    for e in events {
        match &e.kind {
            EventKind::UserMessage { id, text } => {
                items.push(TranscriptItem::User { seq: e.seq, id: id.clone(), text: text.clone() })
            }
            EventKind::TurnStart { turn, model, mode, retry_of } => {
                if retried.contains(turn) {
                    continue;
                }
                turn_index.insert(turn.clone(), items.len());
                items.push(TranscriptItem::Turn(TurnView {
                    turn: turn.clone(),
                    start_seq: e.seq,
                    model: model.clone(),
                    mode: *mode,
                    items: Vec::new(),
                    steps: 0,
                    status: TurnStatus::Running,
                    retry_of: retry_of.clone(),
                }));
            }
            EventKind::AssistantMessage { turn, step, text, tool_calls, .. } => {
                let Some(t) = turn_mut(&mut items, &turn_index, turn) else { continue };
                t.steps = t.steps.max(*step);
                if !text.is_empty() {
                    // A resumed continuation flows on from the paused text.
                    if let Some(StepItem::Text { text: prev, .. }) = t.items.last_mut() {
                        prev.push_str(text);
                    } else {
                        t.items.push(StepItem::Text { seq: e.seq, text: text.clone() });
                    }
                }
                for call in tool_calls {
                    t.items.push(StepItem::Tool(ToolView { step: *step, call: call.clone(), decision: None, result: None }));
                }
            }
            EventKind::ToolDecision { turn, call_id, allow, by } => {
                if let Some(v) = turn_mut(&mut items, &turn_index, turn).and_then(|t| tool_mut(t, call_id)) {
                    v.decision = Some((*allow, *by));
                }
            }
            EventKind::ToolResult { turn, call_id, ok, output, edit, .. } => {
                if let Some(v) = turn_mut(&mut items, &turn_index, turn).and_then(|t| tool_mut(t, call_id)) {
                    v.result = Some(ToolResultView {
                        seq: e.seq,
                        ok: *ok,
                        output: output.clone(),
                        edit: edit.clone(),
                        undone: undone.contains(&e.seq),
                    });
                }
            }
            EventKind::TurnPause { turn, reason } => {
                if let Some(t) = turn_mut(&mut items, &turn_index, turn) {
                    t.status = TurnStatus::Paused { reason: reason.clone() };
                }
            }
            EventKind::TurnResume { turn } => {
                if let Some(t) = turn_mut(&mut items, &turn_index, turn) {
                    t.status = TurnStatus::Running;
                }
            }
            EventKind::TurnEnd { turn, outcome } => {
                if let Some(t) = turn_mut(&mut items, &turn_index, turn) {
                    t.status = match outcome {
                        Outcome::Done => TurnStatus::Done,
                        Outcome::Cancelled => TurnStatus::Cancelled,
                        Outcome::Failed { reason } => TurnStatus::Failed { reason: reason.clone() },
                        Outcome::Interrupted { reason } => TurnStatus::Interrupted { reason: reason.clone() },
                        Outcome::StepLimit => TurnStatus::StepLimit,
                    };
                }
            }
            EventKind::EditUndo { edit_seq } => {
                let path = edit_path(events, *edit_seq).unwrap_or_else(|| format!("#{edit_seq}"));
                items.push(TranscriptItem::Note { seq: e.seq, text: format!("Undid edit to {path}") });
            }
        }
    }
    items
}

// --- turn queries ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct OpenTurn {
    pub turn: String,
    pub model: String,
    pub mode: ApprovalMode,
    pub paused: bool,
}

/// The turn that has started but not ended, if any (at most one).
pub fn open_turn(events: &[Event]) -> Option<OpenTurn> {
    let (start_index, turn, model, mode) = events.iter().enumerate().rev().find_map(|(i, e)| match &e.kind {
        EventKind::TurnStart { turn, model, mode, .. } => Some((i, turn.clone(), model.clone(), *mode)),
        _ => None,
    })?;
    let mut paused = false;
    for e in &events[start_index..] {
        match &e.kind {
            EventKind::TurnEnd { turn: t, .. } if *t == turn => return None,
            EventKind::TurnPause { turn: t, .. } if *t == turn => paused = true,
            EventKind::TurnResume { turn: t } if *t == turn => paused = false,
            _ => {}
        }
    }
    Some(OpenTurn { turn, model, mode, paused })
}

/// The last turn, when it ended in a state Retry can replace.
pub fn retryable_turn(events: &[Event]) -> Option<String> {
    let last_turn = events.iter().rev().find_map(|e| match &e.kind {
        EventKind::TurnStart { turn, .. } => Some(turn.clone()),
        _ => None,
    })?;
    // Only when nothing the user said came after it.
    let after_user = events.iter().rev().take_while(|e| !matches!(&e.kind, EventKind::TurnStart { turn, .. } if *turn == last_turn)).any(|e| matches!(e.kind, EventKind::UserMessage { .. }));
    if after_user {
        return None;
    }
    events.iter().rev().find_map(|e| match &e.kind {
        EventKind::TurnEnd { turn, outcome } if *turn == last_turn => match outcome {
            Outcome::Failed { .. } | Outcome::Interrupted { .. } | Outcome::Cancelled => Some(turn.clone()),
            _ => None,
        },
        _ => None,
    })
}

/// Convert a Stage 2 message list into an equivalent log (one-time
/// migration on load). Each assistant message becomes its own settled turn.
pub fn from_legacy(messages: &[Message], at: f64) -> Vec<Event> {
    let mut events = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        if m.is_user() {
            append(&mut events, EventKind::UserMessage { id: m.id.clone(), text: m.content.clone() }, at);
        } else {
            let turn = format!("t-legacy-{i}");
            append(
                &mut events,
                EventKind::TurnStart {
                    turn: turn.clone(),
                    model: m.model.clone().unwrap_or_else(|| crate::DEFAULT_MODEL.to_string()),
                    mode: ApprovalMode::Ask,
                    retry_of: None,
                },
                at,
            );
            let partial = m.status != "Done";
            append(
                &mut events,
                EventKind::AssistantMessage { turn: turn.clone(), step: 1, text: m.content.clone(), tool_calls: Vec::new(), partial },
                at,
            );
            let reason = m.error.clone().unwrap_or_else(|| "turn did not finish".to_string());
            let outcome = match m.status.as_str() {
                "Done" => Outcome::Done,
                "Cancelled" => Outcome::Cancelled,
                "Failed" => Outcome::Failed { reason },
                _ => Outcome::Interrupted { reason },
            };
            append(&mut events, EventKind::TurnEnd { turn, outcome }, at);
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolCall;

    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall { id: id.into(), name: name.into(), arguments: args.into() }
    }

    fn start(log: &mut Vec<Event>, turn: &str, retry_of: Option<&str>) {
        append(
            log,
            EventKind::TurnStart { turn: turn.into(), model: "x-ai/grok-4.7".into(), mode: ApprovalMode::Ask, retry_of: retry_of.map(Into::into) },
            0.0,
        );
    }

    /// A 3-step turn: text + two tool cards + final text (DESIGN §8 bar 3).
    fn tool_turn() -> Vec<Event> {
        let mut log = Vec::new();
        append(&mut log, EventKind::UserMessage { id: "u1".into(), text: "make app".into() }, 0.0);
        start(&mut log, "t1", None);
        append(&mut log, EventKind::AssistantMessage { turn: "t1".into(), step: 1, text: "Writing.".into(), tool_calls: vec![call("c1", "write_file", r#"{"path":"a.txt","content":"x"}"#)], partial: false }, 0.0);
        append(&mut log, EventKind::ToolDecision { turn: "t1".into(), call_id: "c1".into(), allow: true, by: DecisionBy::User }, 0.0);
        append(&mut log, EventKind::ToolResult { turn: "t1".into(), call_id: "c1".into(), name: "write_file".into(), ok: true, output: "created a.txt".into(), edit: Some(FileEdit { path: "a.txt".into(), before: None, after: Some("x".into()) }) }, 0.0);
        append(&mut log, EventKind::AssistantMessage { turn: "t1".into(), step: 2, text: String::new(), tool_calls: vec![call("c2", "read_file", r#"{"path":"a.txt"}"#)], partial: false }, 0.0);
        append(&mut log, EventKind::ToolDecision { turn: "t1".into(), call_id: "c2".into(), allow: true, by: DecisionBy::Policy }, 0.0);
        append(&mut log, EventKind::ToolResult { turn: "t1".into(), call_id: "c2".into(), name: "read_file".into(), ok: true, output: "1  x".into(), edit: None }, 0.0);
        append(&mut log, EventKind::AssistantMessage { turn: "t1".into(), step: 3, text: "Done.".into(), tool_calls: vec![], partial: false }, 0.0);
        append(&mut log, EventKind::TurnEnd { turn: "t1".into(), outcome: Outcome::Done }, 0.0);
        log
    }

    #[test]
    fn seq_is_monotonic_and_serde_names_are_durable() {
        let log = tool_turn();
        for (i, e) in log.iter().enumerate() {
            assert_eq!(e.seq, i as u64 + 1);
        }
        let raw = serde_json::to_string(&log).unwrap();
        assert!(raw.contains("\"kind\":\"user/message\""));
        assert!(raw.contains("\"kind\":\"tool/result\""));
        let back: Vec<Event> = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, log);
    }

    #[test]
    fn derive_messages_builds_openai_tool_history() {
        let msgs = derive_messages(&tool_turn());
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "tool", "assistant", "tool", "assistant"]);
        assert_eq!(msgs[1].tool_calls[0].id, "c1");
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(msgs[3].content, None, "tool-only step has null content");
        let json = serde_json::to_value(&msgs[3]).unwrap();
        assert!(json.get("content").unwrap().is_null());
        assert_eq!(json["tool_calls"][0]["type"], "function");
    }

    #[test]
    fn transcript_pairs_actions_with_observations() {
        let items = derive_transcript(&tool_turn());
        assert_eq!(items.len(), 2);
        let TranscriptItem::Turn(t) = &items[1] else { panic!() };
        assert_eq!(t.status, TurnStatus::Done);
        assert_eq!(t.steps, 3);
        assert_eq!(t.tool_count(), 2);
        let tools: Vec<&ToolView> = t.items.iter().filter_map(|i| if let StepItem::Tool(v) = i { Some(v) } else { None }).collect();
        assert_eq!(tools[0].call.name, "write_file");
        assert_eq!(tools[0].decision, Some((true, DecisionBy::User)));
        assert!(tools[0].result.as_ref().unwrap().edit.is_some());
        assert_eq!(tools[1].result.as_ref().unwrap().output, "1  x");
    }

    #[test]
    fn retry_appends_and_hides_the_failed_attempt() {
        let mut log = Vec::new();
        append(&mut log, EventKind::UserMessage { id: "u1".into(), text: "hi".into() }, 0.0);
        start(&mut log, "t1", None);
        append(&mut log, EventKind::AssistantMessage { turn: "t1".into(), step: 1, text: "par".into(), tool_calls: vec![], partial: true }, 0.0);
        append(&mut log, EventKind::TurnEnd { turn: "t1".into(), outcome: Outcome::Interrupted { reason: "drop".into() } }, 0.0);
        assert_eq!(retryable_turn(&log).as_deref(), Some("t1"));
        let before = log.len();
        start(&mut log, "t2", Some("t1"));
        append(&mut log, EventKind::AssistantMessage { turn: "t2".into(), step: 1, text: "hello".into(), tool_calls: vec![], partial: false }, 0.0);
        append(&mut log, EventKind::TurnEnd { turn: "t2".into(), outcome: Outcome::Done }, 0.0);
        assert_eq!(log.len(), before + 3, "append-only: nothing removed");
        let msgs = derive_messages(&log);
        assert_eq!(msgs.iter().filter(|m| m.role == "user").count(), 1, "user event not duplicated");
        assert_eq!(msgs.last().unwrap().content.as_deref(), Some("hello"));
        assert!(!msgs.iter().any(|m| m.content.as_deref() == Some("par")), "retried attempt adds no history");
        let items = derive_transcript(&log);
        assert_eq!(items.len(), 2, "user + the retry turn only");
        assert_eq!(retryable_turn(&log), None);
    }

    #[test]
    fn failed_turns_add_no_history_and_cancelled_partial_is_kept() {
        let mut log = Vec::new();
        append(&mut log, EventKind::UserMessage { id: "u1".into(), text: "a".into() }, 0.0);
        start(&mut log, "t1", None);
        append(&mut log, EventKind::TurnEnd { turn: "t1".into(), outcome: Outcome::Failed { reason: "401".into() } }, 0.0);
        append(&mut log, EventKind::UserMessage { id: "u2".into(), text: "b".into() }, 0.0);
        start(&mut log, "t2", None);
        append(&mut log, EventKind::AssistantMessage { turn: "t2".into(), step: 1, text: "half".into(), tool_calls: vec![call("cx", "write_file", "{}")], partial: true }, 0.0);
        append(&mut log, EventKind::TurnEnd { turn: "t2".into(), outcome: Outcome::Cancelled }, 0.0);
        let msgs = derive_messages(&log);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2].content.as_deref(), Some("half"));
        assert!(msgs[2].tool_calls.is_empty(), "partial steps never carry calls");
    }

    #[test]
    fn unanswered_tool_calls_are_dropped_from_history() {
        let mut log = Vec::new();
        append(&mut log, EventKind::UserMessage { id: "u1".into(), text: "a".into() }, 0.0);
        start(&mut log, "t1", None);
        append(&mut log, EventKind::AssistantMessage { turn: "t1".into(), step: 1, text: String::new(), tool_calls: vec![call("c1", "write_file", "{}")], partial: false }, 0.0);
        append(&mut log, EventKind::TurnEnd { turn: "t1".into(), outcome: Outcome::Cancelled }, 0.0);
        assert_eq!(derive_messages(&log).len(), 1);
    }

    #[test]
    fn pause_resume_keeps_the_turn() {
        let mut log = Vec::new();
        append(&mut log, EventKind::UserMessage { id: "u1".into(), text: "story".into() }, 0.0);
        start(&mut log, "t1", None);
        append(&mut log, EventKind::AssistantMessage { turn: "t1".into(), step: 1, text: "Once upon".into(), tool_calls: vec![], partial: true }, 0.0);
        append(&mut log, EventKind::TurnPause { turn: "t1".into(), reason: "you paused".into() }, 0.0);
        let open = open_turn(&log).unwrap();
        assert!(open.paused);
        let TranscriptItem::Turn(t) = &derive_transcript(&log)[1] else { panic!() };
        assert!(matches!(t.status, TurnStatus::Paused { .. }));
        append(&mut log, EventKind::TurnResume { turn: "t1".into() }, 0.0);
        assert!(!open_turn(&log).unwrap().paused);
        let msgs = derive_messages(&log);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1].content.as_deref(), Some("Once upon"));
        assert_eq!(msgs[2].content.as_deref(), Some(RESUME_NOTE));
        append(&mut log, EventKind::AssistantMessage { turn: "t1".into(), step: 1, text: " a time.".into(), tool_calls: vec![], partial: false }, 0.0);
        append(&mut log, EventKind::TurnEnd { turn: "t1".into(), outcome: Outcome::Done }, 0.0);
        let items = derive_transcript(&log);
        let TranscriptItem::Turn(t) = &items[1] else { panic!() };
        assert_eq!(t.items, vec![StepItem::Text { seq: 3, text: "Once upon a time.".into() }], "one flowing reply");
        assert_eq!(open_turn(&log), None);
        assert_eq!(derive_messages(&log).iter().filter(|m| m.role == "user" && m.content.as_deref() == Some("story")).count(), 1);
    }

    #[test]
    fn undo_is_an_event_and_tells_the_model() {
        let mut log = tool_turn();
        let edit_seq = log.iter().find(|e| matches!(&e.kind, EventKind::ToolResult { edit: Some(_), .. })).unwrap().seq;
        append(&mut log, EventKind::EditUndo { edit_seq }, 0.0);
        let msgs = derive_messages(&log);
        assert!(msgs.last().unwrap().content.as_deref().unwrap().contains("undid your last edit to a.txt"));
        let items = derive_transcript(&log);
        assert!(matches!(items.last(), Some(TranscriptItem::Note { .. })));
        let TranscriptItem::Turn(t) = &items[1] else { panic!() };
        let StepItem::Tool(v) = &t.items[1] else { panic!() };
        assert!(v.result.as_ref().unwrap().undone);
    }

    #[test]
    fn legacy_messages_migrate_to_events() {
        let mut a = Message::assistant("m2".into(), "x-ai/grok-4.7".into());
        a.content = "hello".into();
        a.status = "Done".into();
        let mut b = Message::assistant("m4".into(), "x-ai/grok-4.7".into());
        b.content = "part".into();
        b.status = "Streaming".into();
        let msgs = vec![Message::user("m1".into(), "hi".into()), a, Message::user("m3".into(), "more".into()), b];
        let log = from_legacy(&msgs, 1.0);
        let derived = derive_messages(&log);
        assert_eq!(derived.iter().map(|m| m.content.clone().unwrap()).collect::<Vec<_>>(), vec!["hi", "hello", "more", "part"]);
        let items = derive_transcript(&log);
        let TranscriptItem::Turn(t) = &items[3] else { panic!() };
        assert!(matches!(t.status, TurnStatus::Interrupted { .. }));
        assert_eq!(open_turn(&log), None);
    }
}
