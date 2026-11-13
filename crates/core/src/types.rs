//! Session/message model shared across the client. Kept free of any
//! framework so the pure logic stays host-testable.

use serde::{Deserialize, Serialize};

pub const KIND_USER: &str = "user";
pub const KIND_ASSISTANT: &str = "assistant";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub updated_at: f64,
    /// Stage 2 message list. Old saves migrate into `events` once
    /// (`Session::migrate_legacy`). On save it is re-filled with a
    /// Stage-2-shaped mirror (`with_legacy_mirror`) so a rollback to the
    /// Stage 2 build still parses and shows every chat; Stage 3 ignores it
    /// whenever `events` is non-empty. Always serialized: Stage 2 requires it.
    #[serde(default)]
    pub messages: Vec<Message>,
    /// Append-only event log — the source of truth (DESIGN.md §4).
    #[serde(default)]
    pub events: Vec<crate::events::Event>,
}

impl Session {
    pub fn new(id: String, title: String, at: f64) -> Self {
        Session { id, title, updated_at: at, messages: Vec::new(), events: Vec::new() }
    }

    /// Append one event (the only mutator of the log). Returns its seq.
    pub fn append(&mut self, kind: crate::events::EventKind, at: f64) -> u64 {
        self.updated_at = at;
        crate::events::append(&mut self.events, kind, at)
    }

    /// One-time Stage 2 → Stage 3 migration. Returns true if it ran.
    pub fn migrate_legacy(&mut self, at: f64) -> bool {
        if self.events.is_empty() && !self.messages.is_empty() {
            self.events = crate::events::from_legacy(&self.messages, at);
            self.messages.clear();
            true
        } else {
            if !self.events.is_empty() {
                // A save-time mirror; the event log is the truth.
                self.messages.clear();
            }
            false
        }
    }

    /// Fold in messages an older build appended to the text mirror after a
    /// rollback (anything past what this log's own mirror already covers).
    /// Returns how many messages were absorbed.
    pub fn absorb_legacy_tail(&mut self, legacy: &[Message], at: f64) -> usize {
        let known = legacy_mirror(&self.events).len();
        if legacy.len() <= known {
            return 0;
        }
        let tail = &legacy[known..];
        for e in crate::events::from_legacy(tail, at) {
            crate::events::append(&mut self.events, e.kind, at);
        }
        self.updated_at = self.updated_at.max(at);
        tail.len()
    }

    /// Copy of this session with `messages` set to the Stage 2 mirror of the
    /// event log. Used only when writing to storage.
    pub fn with_legacy_mirror(&self) -> Session {
        let mut out = self.clone();
        if !self.events.is_empty() {
            out.messages = legacy_mirror(&self.events);
        }
        out
    }
}

/// Combine the IndexedDB records (full logs) with the localStorage list.
/// localStorage order wins (it is rewritten on every save). Per session:
/// - a localStorage copy WITH events (fallback mode / Stage 3 build) that is
///   newer replaces the stored one;
/// - a mirror-only copy (Stage 3.1 mirror, or a Stage 2 build after a
///   rollback) never replaces a full log; it can only add a message tail;
/// - IndexedDB-only sessions are kept.
pub fn merge_local_and_stored(
    local: Vec<Session>,
    mut stored: std::collections::HashMap<String, Session>,
    at: f64,
) -> Vec<Session> {
    let mut out = Vec::with_capacity(local.len() + stored.len());
    for mut l in local {
        match stored.remove(&l.id) {
            None => {
                l.migrate_legacy(at);
                out.push(l);
            }
            Some(s) if !l.events.is_empty() => out.push(if l.updated_at > s.updated_at { l } else { s }),
            Some(mut s) => {
                s.absorb_legacy_tail(&l.messages, at);
                out.push(s);
            }
        }
    }
    if !stored.is_empty() {
        out.extend(stored.into_values());
        out.sort_by(|a, b| b.updated_at.partial_cmp(&a.updated_at).unwrap_or(std::cmp::Ordering::Equal));
    }
    out
}

/// Stage-2-shaped view of an event log: one user message per prompt and one
/// assistant message per (non-retried) turn, text only. Lossy by design; it
/// exists so an older build can still read storage, never to be read back.
pub fn legacy_mirror(events: &[crate::events::Event]) -> Vec<Message> {
    use crate::events::{EventKind, Outcome};
    let retried: std::collections::BTreeSet<&str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::TurnStart { retry_of: Some(old), .. } => Some(old.as_str()),
            _ => None,
        })
        .collect();
    let mut out: Vec<Message> = Vec::new();
    // (turn id, index in `out`, tools used)
    let mut open: Option<(String, usize, usize)> = None;
    for e in events {
        match &e.kind {
            EventKind::UserMessage { id, text } => out.push(Message::user(id.clone(), text.clone())),
            EventKind::TurnStart { turn, model, .. } => {
                if retried.contains(turn.as_str()) {
                    open = None;
                    continue;
                }
                let mut m = Message::assistant(format!("{turn}-m"), model.clone());
                m.status = "Interrupted".to_string();
                m.error = Some("paused".to_string());
                out.push(m);
                open = Some((turn.clone(), out.len() - 1, 0));
            }
            EventKind::AssistantMessage { turn, text, .. } => {
                if let Some((t, i, _)) = &open {
                    if t == turn && !text.is_empty() {
                        let c = &mut out[*i].content;
                        if !c.is_empty() && !c.ends_with('\n') {
                            c.push_str("\n\n");
                        }
                        c.push_str(text);
                    }
                }
            }
            EventKind::ToolResult { turn, .. } => {
                if let Some((t, _, n)) = &mut open {
                    if t == turn {
                        *n += 1;
                    }
                }
            }
            EventKind::TurnEnd { turn, outcome } => {
                if let Some((t, i, n)) = open.take() {
                    if &t != turn {
                        open = Some((t, i, n));
                        continue;
                    }
                    let m = &mut out[i];
                    let (status, err) = match outcome {
                        Outcome::Done | Outcome::StepLimit => ("Done", None),
                        Outcome::Cancelled => ("Cancelled", None),
                        Outcome::Failed { reason } => ("Failed", Some(reason.clone())),
                        Outcome::Interrupted { reason } => ("Interrupted", Some(reason.clone())),
                    };
                    m.status = status.to_string();
                    m.error = err;
                    if n > 0 {
                        let note = format!("({n} tool call{} in this turn)", if n == 1 { "" } else { "s" });
                        if m.content.is_empty() {
                            m.content = note;
                        } else {
                            m.content.push_str("\n\n");
                            m.content.push_str(&note);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    #[serde(rename = "type")]
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Done | Streaming | Interrupted | Cancelled | Failed
    #[serde(default = "default_status")]
    pub status: String,
    /// Short user-facing reason when `status` is Failed/Interrupted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn default_status() -> String {
    "Done".to_string()
}

impl Message {
    pub fn user(id: String, content: String) -> Self {
        Message {
            id,
            role: KIND_USER.to_string(),
            content,
            model: None,
            status: "Done".to_string(),
            error: None,
        }
    }

    pub fn assistant(id: String, model: String) -> Self {
        Message {
            id,
            role: KIND_ASSISTANT.to_string(),
            content: String::new(),
            model: Some(model),
            status: "Streaming".to_string(),
            error: None,
        }
    }

    pub fn is_user(&self) -> bool {
        self.role == KIND_USER
    }
}

/// Wire message sent to `/api/chat`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Proxy base URL, e.g. `https://harness-proxy.vercel.app`. Empty string
    /// means same origin as the page.
    #[serde(default)]
    pub base_url: String,
    /// Harness access token (the bearer gate) — NEVER an OpenRouter key.
    #[serde(default)]
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_serde_roundtrip() {
        let m = Message::user("m1".into(), "hello".into());
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"type\":\"user\""));
        let back: Message = serde_json::from_str(&json).unwrap();
        assert!(back.is_user());

        // Old exports without a status field still load.
        let legacy = r#"{"id":"m2","type":"assistant","content":"hi","model":"x-ai/grok-4.7"}"#;
        let m2: Message = serde_json::from_str(legacy).unwrap();
        assert_eq!(m2.status, "Done");
    }

    #[test]
    fn stage2_session_json_loads_and_migrates_once() {
        let raw = r#"{"id":"s1","title":"Hi","updated_at":1.0,"messages":[{"id":"m1","type":"user","content":"hi"},{"id":"m2","type":"assistant","content":"yo","model":"x-ai/grok-4.7","status":"Done"}]}"#;
        let mut s: Session = serde_json::from_str(raw).unwrap();
        assert!(s.events.is_empty());
        assert!(s.migrate_legacy(2.0));
        assert!(s.messages.is_empty());
        assert_eq!(s.events.len(), 4);
        assert!(!s.migrate_legacy(3.0), "runs once");
        // The mirror round-trips: a reload re-reads events, not the mirror.
        let out = serde_json::to_string(&s.with_legacy_mirror()).unwrap();
        let mut back: Session = serde_json::from_str(&out).unwrap();
        assert!(!back.migrate_legacy(4.0), "mirror never re-migrates");
        assert!(back.messages.is_empty(), "mirror dropped on load");
        assert_eq!(back.events, s.events);
    }

    /// Rollback safety: storage written by Stage 3 must parse with the exact
    /// Stage 2 struct (messages required, no events field) and keep the chat.
    #[test]
    fn stage3_save_parses_as_stage2_with_text_kept() {
        use crate::events::{EventKind, Outcome};
        use crate::permissions::ApprovalMode;
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct Stage2Session { id: String, title: String, updated_at: f64, messages: Vec<Message> }

        let mut s = Session::new("s1".into(), "Tip".into(), 1.0);
        s.append(EventKind::UserMessage { id: "u1".into(), text: "build it".into() }, 1.0);
        s.append(EventKind::TurnStart { turn: "t1".into(), model: "x-ai/grok-4.7".into(), mode: ApprovalMode::Ask, retry_of: None }, 1.0);
        s.append(EventKind::TurnEnd { turn: "t1".into(), outcome: Outcome::Failed { reason: "offline".into() } }, 1.0);
        s.append(EventKind::TurnStart { turn: "t2".into(), model: "x-ai/grok-4.7".into(), mode: ApprovalMode::Ask, retry_of: Some("t1".into()) }, 1.0);
        s.append(EventKind::AssistantMessage { turn: "t2".into(), step: 0, text: "On it.".into(), tool_calls: vec![], partial: false }, 1.0);
        s.append(EventKind::AssistantMessage { turn: "t2".into(), step: 1, text: "Done.".into(), tool_calls: vec![], partial: false }, 1.0);
        s.append(EventKind::TurnEnd { turn: "t2".into(), outcome: Outcome::Done }, 1.0);

        let raw = serde_json::to_string(&vec![s.with_legacy_mirror()]).unwrap();
        let old: Vec<Stage2Session> = serde_json::from_str(&raw).expect("Stage 2 struct parses Stage 3 storage");
        let m = &old[0].messages;
        assert_eq!(m.len(), 2, "retried turn hidden: {m:?}");
        assert!(m[0].is_user() && m[0].content == "build it");
        assert_eq!(m[1].content, "On it.\n\nDone.");
        assert_eq!(m[1].status, "Done");
        // Storage without the mirror (pre-fix Stage 3 shape) would not parse.
        let bare = r#"[{"id":"s1","title":"Tip","updated_at":1.0,"events":[]}]"#;
        assert!(serde_json::from_str::<Vec<Stage2Session>>(bare).is_err());
    }

    #[test]
    fn merge_keeps_full_logs_and_absorbs_rollback_tails() {
        use crate::events::{EventKind, Outcome};
        use crate::permissions::ApprovalMode;
        use std::collections::HashMap;
        let mut full = Session::new("a".into(), "A".into(), 10.0);
        full.append(EventKind::UserMessage { id: "u1".into(), text: "hi".into() }, 10.0);
        full.append(EventKind::TurnStart { turn: "t1".into(), model: "m".into(), mode: ApprovalMode::Ask, retry_of: None }, 10.0);
        full.append(EventKind::AssistantMessage { turn: "t1".into(), step: 1, text: "yo".into(), tool_calls: vec![], partial: false }, 10.0);
        full.append(EventKind::TurnEnd { turn: "t1".into(), outcome: Outcome::Done }, 10.0);
        let stored: HashMap<String, Session> = [("a".to_string(), full.clone())].into();

        // 1. Mirror-only copy written LATER (async IDB commit lost on reload):
        //    the full log must win, nothing absorbed.
        let mut mirror = full.with_legacy_mirror();
        mirror.events.clear();
        mirror.updated_at = 99.0;
        let out = merge_local_and_stored(vec![mirror.clone()], stored.clone(), 100.0);
        assert_eq!(out[0].events, full.events);

        // 2. Stage 2 build after a rollback appended a Q+A: absorbed as a tail.
        let mut rolled = mirror.clone();
        rolled.messages.push(Message::user("u2".into(), "again".into()));
        let mut a2 = Message::assistant("a2".into(), "m".into());
        a2.content = "sure".into();
        a2.status = "Done".into();
        rolled.messages.push(a2);
        let out = merge_local_and_stored(vec![rolled], stored.clone(), 100.0);
        assert_eq!(out[0].events.len(), full.events.len() + 4);
        assert_eq!(legacy_mirror(&out[0].events).len(), 4);

        // 3. A newer FULL local copy (fallback mode) replaces the stored one.
        let mut newer = full.clone();
        newer.append(EventKind::UserMessage { id: "u3".into(), text: "new".into() }, 50.0);
        let out = merge_local_and_stored(vec![newer.clone()], stored.clone(), 100.0);
        assert_eq!(out[0].events, newer.events);

        // 4. Unknown local sessions (Stage 2 shape) migrate; IDB-only kept.
        let s2: Session = serde_json::from_str(r#"{"id":"b","title":"B","updated_at":5.0,"messages":[{"id":"m","type":"user","content":"x"}]}"#).unwrap();
        let out = merge_local_and_stored(vec![s2], stored, 100.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out.iter().find(|s| s.id == "b").unwrap().events.len(), 1);
    }
}
