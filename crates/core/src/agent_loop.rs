//! The tool loop as a pure function of the log (openai/codex pattern: a
//! turn is a loop of model step → tool calls → approvals → results → next
//! step, ending on a reply with no calls or the step cap).
//!
//! The client driver just asks `next_action` what to do, does it, appends
//! the resulting event, and asks again. Because the state lives in the
//! append-only log, a reload or a pause can pick the loop up exactly where
//! it stopped.

use crate::events::{Event, EventKind, Outcome};
use crate::tools::ToolCall;

pub const DEFAULT_MAX_STEPS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopConfig {
    pub max_steps: u32,
}

impl Default for LoopConfig {
    fn default() -> Self {
        LoopConfig { max_steps: DEFAULT_MAX_STEPS }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum NextAction {
    /// Stream a model step. `continuing` = resume a paused partial step.
    CallModel { step: u32, continuing: bool },
    /// A call has no decision yet: apply policy (may need to ask the user).
    Decide(ToolCall),
    /// Allowed and not yet run.
    Run(ToolCall),
    /// Denied and not yet answered: record the denial as the call's result.
    Refuse(ToolCall),
    /// Close the turn.
    Finish(Outcome),
    /// Paused, ended, or unknown turn: nothing to do until the user acts.
    Wait,
}

pub fn next_action(events: &[Event], turn: &str, cfg: &LoopConfig) -> NextAction {
    let in_turn: Vec<&Event> = events.iter().filter(|e| e.kind.turn() == Some(turn)).collect();
    if in_turn.is_empty() {
        return NextAction::Wait;
    }
    let mut paused = false;
    for e in &in_turn {
        match &e.kind {
            EventKind::TurnEnd { .. } => return NextAction::Wait,
            EventKind::TurnPause { .. } => paused = true,
            EventKind::TurnResume { .. } => paused = false,
            _ => {}
        }
    }
    if paused {
        return NextAction::Wait;
    }
    let last_step = in_turn.iter().rev().find_map(|e| match &e.kind {
        EventKind::AssistantMessage { step, tool_calls, partial, .. } => Some((*step, tool_calls.clone(), *partial)),
        _ => None,
    });
    let Some((step, calls, partial)) = last_step else {
        return NextAction::CallModel { step: 1, continuing: false };
    };
    if partial {
        // Only reachable after a resume (a pause would have returned Wait).
        return NextAction::CallModel { step, continuing: true };
    }
    if calls.is_empty() {
        return NextAction::Finish(Outcome::Done);
    }
    for call in &calls {
        let answered = in_turn.iter().any(|e| matches!(&e.kind, EventKind::ToolResult { call_id, .. } if *call_id == call.id));
        if answered {
            continue;
        }
        let decision = in_turn.iter().rev().find_map(|e| match &e.kind {
            EventKind::ToolDecision { call_id, allow, .. } if *call_id == call.id => Some(*allow),
            _ => None,
        });
        return match decision {
            None => NextAction::Decide(call.clone()),
            Some(true) => NextAction::Run(call.clone()),
            Some(false) => NextAction::Refuse(call.clone()),
        };
    }
    if step >= cfg.max_steps {
        NextAction::Finish(Outcome::StepLimit)
    } else {
        NextAction::CallModel { step: step + 1, continuing: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{append, DecisionBy, EventKind};
    use crate::permissions::ApprovalMode;

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall { id: id.into(), name: name.into(), arguments: "{}".into() }
    }

    fn base() -> Vec<Event> {
        let mut log = Vec::new();
        append(&mut log, EventKind::UserMessage { id: "u".into(), text: "go".into() }, 0.0);
        append(&mut log, EventKind::TurnStart { turn: "t".into(), model: "m".into(), mode: ApprovalMode::Ask, retry_of: None }, 0.0);
        log
    }

    fn step(log: &mut Vec<Event>, n: u32, calls: Vec<ToolCall>) {
        append(log, EventKind::AssistantMessage { turn: "t".into(), step: n, text: String::new(), tool_calls: calls, partial: false }, 0.0);
    }

    #[test]
    fn full_loop_walks_decide_run_next_step_finish() {
        let cfg = LoopConfig::default();
        let mut log = base();
        assert_eq!(next_action(&log, "t", &cfg), NextAction::CallModel { step: 1, continuing: false });
        step(&mut log, 1, vec![call("a", "write_file"), call("b", "list_files")]);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::Decide(call("a", "write_file")));
        append(&mut log, EventKind::ToolDecision { turn: "t".into(), call_id: "a".into(), allow: true, by: DecisionBy::User }, 0.0);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::Run(call("a", "write_file")));
        append(&mut log, EventKind::ToolResult { turn: "t".into(), call_id: "a".into(), name: "write_file".into(), ok: true, output: "ok".into(), edit: None }, 0.0);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::Decide(call("b", "list_files")));
        append(&mut log, EventKind::ToolDecision { turn: "t".into(), call_id: "b".into(), allow: false, by: DecisionBy::Policy }, 0.0);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::Refuse(call("b", "list_files")));
        append(&mut log, EventKind::ToolResult { turn: "t".into(), call_id: "b".into(), name: "list_files".into(), ok: false, output: "denied".into(), edit: None }, 0.0);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::CallModel { step: 2, continuing: false });
        step(&mut log, 2, vec![]);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::Finish(Outcome::Done));
        append(&mut log, EventKind::TurnEnd { turn: "t".into(), outcome: Outcome::Done }, 0.0);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::Wait);
    }

    #[test]
    fn step_cap_ends_the_loop() {
        let cfg = LoopConfig { max_steps: 2 };
        let mut log = base();
        for n in 1..=2 {
            let id = format!("c{n}");
            step(&mut log, n, vec![call(&id, "list_files")]);
            append(&mut log, EventKind::ToolDecision { turn: "t".into(), call_id: id.clone(), allow: true, by: DecisionBy::Policy }, 0.0);
            append(&mut log, EventKind::ToolResult { turn: "t".into(), call_id: id, name: "list_files".into(), ok: true, output: String::new(), edit: None }, 0.0);
        }
        assert_eq!(next_action(&log, "t", &cfg), NextAction::Finish(Outcome::StepLimit));
    }

    #[test]
    fn pause_waits_and_resume_continues_the_same_step() {
        let cfg = LoopConfig::default();
        let mut log = base();
        append(&mut log, EventKind::AssistantMessage { turn: "t".into(), step: 1, text: "half".into(), tool_calls: vec![], partial: true }, 0.0);
        append(&mut log, EventKind::TurnPause { turn: "t".into(), reason: "you paused".into() }, 0.0);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::Wait);
        append(&mut log, EventKind::TurnResume { turn: "t".into() }, 0.0);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::CallModel { step: 1, continuing: true });
    }

    #[test]
    fn reload_mid_approval_reasks() {
        // Paused (by reload) while a decision was pending → resume re-asks.
        let cfg = LoopConfig::default();
        let mut log = base();
        step(&mut log, 1, vec![call("a", "write_file")]);
        append(&mut log, EventKind::TurnPause { turn: "t".into(), reason: "reload".into() }, 0.0);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::Wait);
        append(&mut log, EventKind::TurnResume { turn: "t".into() }, 0.0);
        assert_eq!(next_action(&log, "t", &cfg), NextAction::Decide(call("a", "write_file")));
        assert_eq!(next_action(&log, "other", &cfg), NextAction::Wait);
    }
}
