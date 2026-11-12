//! Shared app state (one context provided at the root).

use crate::storage::{self, Boot, NEW_CHAT_TITLE};
use harness_core::{Message, Session, Settings};
use leptos::prelude::*;

/// One in-flight turn. Only ever zero or one of these.
#[derive(Clone)]
pub struct Turn {
    pub session_id: String,
    pub message_id: String,
    pub abort: web_sys::AbortController,
}

/// An artifact opened in the bottom sheet. Model HTML renders only inside a
/// sandboxed iframe in the sheet — never in the parent document.
#[derive(Clone, PartialEq)]
pub enum Artifact {
    Code { lang: String, body: String },
    Html { title: String, body: String },
}

#[derive(Clone)]
pub struct AppCtx {
    pub sessions: RwSignal<Vec<Session>>,
    pub current_id: RwSignal<String>,
    pub settings: RwSignal<Settings>,
    /// Locked model — what every request sends.
    pub model: RwSignal<String>,
    pub turn: RwSignal<Option<Turn>>,
    pub drawer_open: RwSignal<bool>,
    pub settings_open: RwSignal<bool>,
    pub artifact: RwSignal<Option<Artifact>>,
    pub storage_error: RwSignal<Option<String>>,
    /// Boot notes (migration notice, storage read failures).
    pub notes: RwSignal<Vec<String>>,
}

impl AppCtx {
    pub fn new(boot: Boot) -> Self {
        let mut sessions = boot.sessions;
        if sessions.is_empty() {
            sessions.push(Session {
                id: storage::uid("s"),
                title: NEW_CHAT_TITLE.to_string(),
                updated_at: storage::now_ms(),
                messages: Vec::new(),
            });
        }
        let current_id = sessions[0].id.clone();
        let ctx = Self {
            sessions: RwSignal::new(sessions),
            current_id: RwSignal::new(current_id),
            settings: RwSignal::new(boot.settings),
            model: RwSignal::new(boot.model),
            turn: RwSignal::new(None),
            drawer_open: RwSignal::new(false),
            settings_open: RwSignal::new(false),
            artifact: RwSignal::new(None),
            storage_error: RwSignal::new(None),
            notes: RwSignal::new(boot.notes),
        };
        ctx
    }

    /// Reactive: reads the settings signal so the banner clears as soon as
    /// the user connects.
    pub fn connected_hint_needed(&self) -> bool {
        let settings = self.settings.get();
        settings.base_url.trim().is_empty() && settings.token.is_empty()
    }
}

/// Find the live message for a view that re-renders on any session change.
pub fn find_message(ctx: &AppCtx, session_id: &str, message_id: &str) -> Option<Message> {
    ctx.sessions
        .get()
        .iter()
        .find(|s| s.id == session_id)
        .and_then(|s| s.messages.iter().find(|m| m.id == message_id))
        .cloned()
}

pub fn current_session(ctx: &AppCtx) -> Session {
    let sessions = ctx.sessions.get();
    let id = ctx.current_id.get();
    sessions
        .iter()
        .find(|s| s.id == id)
        .cloned()
        .unwrap_or_else(|| Session {
            id: id.clone(),
            title: NEW_CHAT_TITLE.to_string(),
            updated_at: storage::now_ms(),
            messages: Vec::new(),
        })
}

pub fn with_session(ctx: &AppCtx, session_id: &str, f: impl FnOnce(&mut Session)) {
    ctx.sessions.update(|sessions| {
        if let Some(session) = sessions.iter_mut().find(|s| s.id == session_id) {
            f(session);
            session.updated_at = storage::now_ms();
        }
    });
}
