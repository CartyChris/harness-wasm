//! Shared app state (one Copy context passed down from the root).

use crate::storage::{self, Boot, NEW_CHAT_TITLE};
use harness_core::events::{derive_transcript, EventKind, TranscriptItem};
use harness_core::permissions::{ApprovalMode, Rules};
use harness_core::tools::ToolCall;
use harness_core::{Session, Settings};
use leptos::prelude::*;
use std::collections::BTreeSet;

/// What the live (in-memory) part of the open turn is doing right now.
#[derive(Clone, PartialEq)]
pub enum Phase {
    /// Streaming a model step.
    Streaming,
    /// Waiting for the user to Allow / Deny this call.
    Approval(ToolCall),
    /// Executing a browser-side tool.
    Running(String),
}

/// The in-flight part of a turn. Settled data lives in the session log; this
/// holds only what is not durable yet (stream text, abort handle).
#[derive(Clone)]
pub struct LiveTurn {
    pub session_id: String,
    pub turn: String,
    pub step: u32,
    pub phase: Phase,
    pub text: String,
    pub reasoning_chars: usize,
    pub tool_name: Option<String>,
    pub abort: Option<web_sys::AbortController>,
    /// Set by Pause before aborting so the driver settles as Paused, not
    /// Cancelled.
    pub pause_requested: bool,
}

/// An artifact opened in the bottom sheet. Model HTML renders only inside a
/// sandboxed iframe in the sheet — never in the parent document.
#[derive(Clone, PartialEq)]
pub enum Artifact {
    Code { lang: String, body: String },
    Html { title: String, body: String },
}

#[derive(Clone, Copy)]
pub struct AppCtx {
    pub sessions: RwSignal<Vec<Session>>,
    pub current_id: RwSignal<String>,
    pub settings: RwSignal<Settings>,
    /// Locked model — what every request sends.
    pub model: RwSignal<String>,
    /// codex-style approval mode.
    pub mode: RwSignal<ApprovalMode>,
    /// opencode-style per-tool rules.
    pub rules: RwSignal<Rules>,
    pub tools_enabled: RwSignal<bool>,
    pub live: RwSignal<Option<LiveTurn>>,
    pub drawer_open: RwSignal<bool>,
    pub settings_open: RwSignal<bool>,
    pub context_open: RwSignal<bool>,
    pub log_open: RwSignal<bool>,
    pub mode_menu_open: RwSignal<bool>,
    pub artifact: RwSignal<Option<Artifact>>,
    pub storage_error: RwSignal<Option<String>>,
    /// Boot notes (migration notice, reload recovery, storage read failures).
    pub notes: RwSignal<Vec<String>>,
    pub draft: RwSignal<String>,
    /// Expanded timeline cards, by tool-call id.
    pub expanded: RwSignal<BTreeSet<String>>,
    /// Thread is scrolled to (near) the bottom: new tokens keep it there.
    pub pinned: RwSignal<bool>,
    /// Short confirmation ("Copied") shown over the composer.
    pub toast: RwSignal<Option<String>>,
    pub transcript: Memo<Vec<TranscriptItem>>,
}

impl AppCtx {
    pub fn new(boot: Boot) -> Self {
        let mut sessions = boot.sessions;
        if sessions.is_empty() {
            sessions.push(Session::new(storage::uid("s"), NEW_CHAT_TITLE.to_string(), storage::now_ms()));
        }
        let current_id = sessions[0].id.clone();
        let sessions = RwSignal::new(sessions);
        let current_id = RwSignal::new(current_id);
        let transcript = Memo::new(move |_| {
            let id = current_id.get();
            sessions.with(|all| {
                all.iter()
                    .find(|s| s.id == id)
                    .map(|s| derive_transcript(&s.events))
                    .unwrap_or_default()
            })
        });
        Self {
            sessions,
            current_id,
            settings: RwSignal::new(boot.settings),
            model: RwSignal::new(boot.model),
            mode: RwSignal::new(boot.mode),
            rules: RwSignal::new(boot.rules),
            tools_enabled: RwSignal::new(boot.tools_enabled),
            live: RwSignal::new(None),
            drawer_open: RwSignal::new(false),
            settings_open: RwSignal::new(false),
            context_open: RwSignal::new(false),
            log_open: RwSignal::new(false),
            mode_menu_open: RwSignal::new(false),
            artifact: RwSignal::new(None),
            storage_error: RwSignal::new(boot.storage_error),
            notes: RwSignal::new(boot.notes),
            draft: RwSignal::new(String::new()),
            expanded: RwSignal::new(BTreeSet::new()),
            pinned: RwSignal::new(true),
            toast: RwSignal::new(None),
            transcript,
        }
    }

    /// Reactive: reads the settings signal so the banner clears as soon as
    /// the user connects.
    pub fn connected_hint_needed(&self) -> bool {
        let settings = self.settings.get();
        settings.base_url.trim().is_empty() && settings.token.is_empty()
    }

    /// True while a turn is live in *this* session (streaming, approving, or
    /// running a tool).
    pub fn busy(&self) -> bool {
        self.live.with(|l| l.is_some())
    }

    pub fn live_in_current(&self) -> Option<LiveTurn> {
        let id = self.current_id.get();
        self.live.get().filter(|l| l.session_id == id)
    }
}

pub fn current_session(ctx: &AppCtx) -> Session {
    let id = ctx.current_id.get();
    ctx.sessions.with(|sessions| {
        sessions
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .unwrap_or_else(|| Session::new(id.clone(), NEW_CHAT_TITLE.to_string(), storage::now_ms()))
    })
}

pub fn session_events(ctx: &AppCtx, session_id: &str) -> Vec<harness_core::events::Event> {
    ctx.sessions.with_untracked(|sessions| {
        sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.events.clone())
            .unwrap_or_default()
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

/// Append one durable event and persist. The only write path into a log.
pub fn append_event(ctx: &AppCtx, session_id: &str, kind: EventKind) -> u64 {
    let mut seq = 0;
    with_session(ctx, session_id, |session| {
        seq = session.append(kind, storage::now_ms());
    });
    storage::save_sessions(ctx);
    seq
}

/// Show a toast for ~1.6 s (a newer toast replaces an older one).
pub fn show_toast(ctx: &AppCtx, msg: &str) {
    ctx.toast.set(Some(msg.to_string()));
    let toast = ctx.toast;
    let msg = msg.to_string();
    let cb = wasm_bindgen::closure::Closure::once_into_js(move || {
        if toast.get_untracked().as_deref() == Some(msg.as_str()) {
            toast.set(None);
        }
    });
    let _ = crate::js::window().set_timeout_with_callback_and_timeout_and_arguments_0(wasm_bindgen::JsCast::unchecked_ref(&cb), 1600);
}
