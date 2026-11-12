//! Turn state machine: send / cancel / retry (DESIGN.md §1, §6).
//!
//! - At most one in-flight turn; the composer's Send becomes Cancel.
//! - Cancel aborts the fetch and keeps partial text, marked Cancelled.
//! - A dropped stream keeps partial text, marked Interrupted, Retry available.
//! - Retry removes only the failed assistant placeholder — the user message
//!   is never duplicated in the log.

use crate::net::{self, NetError, Outcome};
use crate::state::{with_session, AppCtx, Turn};
use crate::storage;
use harness_core::{ChatMessage, Message};
use leptos::prelude::*;
use std::cell::Cell;

pub fn send(ctx: &AppCtx, text: String) {
    if ctx.turn.get_untracked().is_some() {
        return;
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    let session_id = ctx.current_id.get_untracked();
    let user = Message::user(storage::uid("m"), text.clone());
    with_session(ctx, &session_id, |session| {
        if session.title.is_empty() || session.title == storage::NEW_CHAT_TITLE {
            session.title = storage::title_from(&text);
        }
        session.messages.push(user);
    });
    storage::save_sessions(ctx);
    spawn_turn(ctx.clone(), session_id);
}

/// Cancel the in-flight turn: abort the fetch, keep the partial, close the
/// turn as cancelled.
pub fn cancel(ctx: &AppCtx) {
    if let Some(turn) = ctx.turn.get_untracked() {
        turn.abort.abort();
    }
}

/// Retry the last turn if its assistant message failed/was interrupted or
/// cancelled. The user event is not duplicated.
pub fn retry(ctx: &AppCtx) {
    if ctx.turn.get_untracked().is_some() {
        return;
    }
    let session_id = ctx.current_id.get_untracked();
    let mut removed = false;
    with_session(ctx, &session_id, |session| {
        let retryable = session.messages.last().is_some_and(|m| {
            !m.is_user()
                && matches!(m.status.as_str(), "Failed" | "Interrupted" | "Cancelled")
        });
        if retryable {
            session.messages.pop();
            removed = true;
        }
    });
    if removed {
        storage::save_sessions(ctx);
        spawn_turn(ctx.clone(), session_id);
    }
}

fn spawn_turn(ctx: AppCtx, session_id: String) {
    wasm_bindgen_futures::spawn_local(async move {
        run_turn(&ctx, &session_id).await;
    });
}

async fn run_turn(ctx: &AppCtx, session_id: &str) {
    // Snapshot the conversation history before adding the placeholder.
    let (history, model, settings) = {
        let sessions = ctx.sessions.get_untracked();
        let session = sessions.iter().find(|s| s.id == session_id);
        let history: Vec<ChatMessage> = session
            .map(|s| {
                s.messages
                    .iter()
                    .map(|m| ChatMessage {
                        role: if m.is_user() { "user" } else { "assistant" }.to_string(),
                        content: m.content.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let model = ctx.model.get_untracked();
        let settings = ctx.settings.get_untracked();
        (history, model, settings)
    };

    let assistant_id = storage::uid("m");
    with_session(ctx, session_id, |session| {
        session
            .messages
            .push(Message::assistant(assistant_id.clone(), model.clone()));
    });

    let abort = web_sys::AbortController::new().expect("AbortController");
    ctx.turn.set(Some(Turn {
        session_id: session_id.to_string(),
        message_id: assistant_id.clone(),
        abort: abort.clone(),
    }));
    crate::js::schedule_scroll_to_bottom();

    let received_any = Cell::new(false);
    let received_in_stream = received_any.clone();
    let sessions_signal = ctx.sessions;
    let target_session = session_id.to_string();
    let target_message = assistant_id.clone();
    let on_delta = move |delta: &str| {
        received_in_stream.set(true);
        sessions_signal.update(|sessions| {
            if let Some(session) = sessions.iter_mut().find(|s| s.id == target_session) {
                if let Some(message) = session
                    .messages
                    .iter_mut()
                    .find(|m| m.id == target_message)
                {
                    message.content.push_str(delta);
                }
            }
        });
    };

    let result = net::stream_chat(&settings, &model, &history, &on_delta, &abort).await;

    match result {
        Ok(Outcome::Done) => set_status(ctx, session_id, &assistant_id, "Done", None),
        Ok(Outcome::Cancelled) => {
            set_status(ctx, session_id, &assistant_id, "Cancelled", None)
        }
        Err(NetError::Unauthorized) => {
            set_status(
                ctx,
                session_id,
                &assistant_id,
                "Failed",
                Some("Connect required — missing or wrong access token.".to_string()),
            );
            ctx.settings_open.set(true);
        }
        Err(NetError::Unreachable(reason)) => {
            if received_any.get() {
                // Stream drop mid-token: keep partial text, interrupted.
                set_status(
                    ctx,
                    session_id,
                    &assistant_id,
                    "Interrupted",
                    Some("The stream dropped. Retry continues from your last message.".to_string()),
                );
            } else {
                set_status(
                    ctx,
                    session_id,
                    &assistant_id,
                    "Failed",
                    Some(format!("Proxy unreachable — {reason}")),
                );
            }
        }
        Err(NetError::Api { message, .. }) => {
            if received_any.get() {
                set_status(ctx, session_id, &assistant_id, "Interrupted", Some(message));
            } else {
                set_status(ctx, session_id, &assistant_id, "Failed", Some(message));
            }
        }
    }

    ctx.turn.set(None);
    storage::save_sessions(ctx);
    crate::js::schedule_scroll_to_bottom();
}

fn set_status(
    ctx: &AppCtx,
    session_id: &str,
    message_id: &str,
    status: &str,
    error: Option<String>,
) {
    with_session(ctx, session_id, |session| {
        if let Some(message) = session.messages.iter_mut().find(|m| m.id == message_id) {
            message.status = status.to_string();
            message.error = error;
        }
    });
}
