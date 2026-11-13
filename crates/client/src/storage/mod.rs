//! localStorage persistence. Sessions, settings, and the locked model live
//! here; the OpenRouter key NEVER does (it only ever exists in server env).

use crate::state::AppCtx;
use harness_core::events::{open_turn, EventKind};
use harness_core::permissions::{ApprovalMode, Rules};
use harness_core::{resolve_model, Session, Settings};
use leptos::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

pub const KEY_SESSIONS: &str = "harness.sessions";
pub const KEY_SETTINGS: &str = "harness.settings";
pub const KEY_MODEL: &str = "harness.model";
pub const KEY_MODEL_MIGRATED: &str = "harness.model_migrated";
pub const KEY_MODE: &str = "harness.mode";
pub const KEY_RULES: &str = "harness.rules";
pub const KEY_TOOLS: &str = "harness.tools";
/// Checkpoint of the streaming text so a reload never loses a turn.
pub const KEY_LIVE: &str = "harness.live";

pub const NEW_CHAT_TITLE: &str = "New chat";

static COUNTER: AtomicU32 = AtomicU32::new(1);

pub fn uid(prefix: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{:x}-{n}", js_sys::Date::now() as u32)
}

pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

fn read_string(key: &str) -> Option<String> {
    local_storage()
        .and_then(|storage| storage.get_item(key).ok().flatten())
        .filter(|v| !v.is_empty())
}

fn write_string(key: &str, value: &str) -> Result<(), String> {
    let Some(storage) = local_storage() else {
        return Err("Storage is unavailable in this browser context.".into());
    };
    storage
        .set_item(key, value)
        .map_err(|_| "Storage is full or blocked — changes may not persist. The live session stays in memory.".to_string())
}

/// Everything loaded at boot.
pub struct Boot {
    pub sessions: Vec<Session>,
    pub settings: Settings,
    pub model: String,
    pub mode: ApprovalMode,
    pub rules: Rules,
    pub tools_enabled: bool,
    pub notes: Vec<String>,
}

pub fn boot() -> Boot {
    let mut notes = Vec::new();

    let mut sessions: Vec<Session> = read_string(KEY_SESSIONS)
        .map(|raw| {
            serde_json::from_str(&raw).unwrap_or_else(|err| {
                notes.push(format!(
                    "Saved sessions could not be read ({err}); starting fresh."
                ));
                Vec::new()
            })
        })
        .unwrap_or_default();

    let settings: Settings = read_string(KEY_SETTINGS)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();

    let saved_model = read_string(KEY_MODEL);
    let migrated = read_string(KEY_MODEL_MIGRATED).as_deref() == Some("1");
    let resolution = resolve_model(saved_model.as_deref(), migrated);
    if resolution.migrated_now {
        notes.push(
            "Default model moved from openai/gpt-4o-mini to x-ai/grok-4.7 (one-time migration)."
                .to_string(),
        );
        let _ = write_string(KEY_MODEL, harness_core::DEFAULT_MODEL);
        let _ = write_string(KEY_MODEL_MIGRATED, "1");
    }

    // Stage 2 saves: migrate message lists into event logs (once).
    let now = now_ms();
    let mut migrated_any = false;
    for session in sessions.iter_mut() {
        migrated_any |= session.migrate_legacy(now);
    }
    if migrated_any {
        notes.push("Sessions upgraded to the append-only event log.".to_string());
    }

    // goose-style recovery: a turn that was live when the page went away is
    // settled as Paused (keeping any checkpointed text) so Resume continues it.
    let checkpoint = read_string(KEY_LIVE).and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
    let mut recovered = false;
    for session in sessions.iter_mut() {
        let Some(open) = open_turn(&session.events) else { continue };
        if open.paused {
            continue;
        }
        if let Some(cp) = &checkpoint {
            let matches = cp.get("session").and_then(|v| v.as_str()) == Some(session.id.as_str())
                && cp.get("turn").and_then(|v| v.as_str()) == Some(open.turn.as_str());
            let text = cp.get("text").and_then(|v| v.as_str()).unwrap_or_default();
            if matches && !text.is_empty() {
                let step = cp.get("step").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
                session.append(
                    EventKind::AssistantMessage { turn: open.turn.clone(), step, text: text.to_string(), tool_calls: Vec::new(), partial: true },
                    now,
                );
            }
        }
        session.append(EventKind::TurnPause { turn: open.turn.clone(), reason: "page reloaded".to_string() }, now);
        recovered = true;
    }
    if recovered {
        notes.push("A turn was interrupted by a reload. Its text is kept; tap Resume to continue it.".to_string());
    }
    if let Some(storage) = local_storage() {
        let _ = storage.remove_item(KEY_LIVE);
    }
    if migrated_any || recovered {
        if let Ok(raw) = serde_json::to_string(&sessions) {
            let _ = write_string(KEY_SESSIONS, &raw);
        }
    }

    let mode = read_string(KEY_MODE).and_then(|m| ApprovalMode::parse(&m)).unwrap_or_default();
    let rules: Rules = read_string(KEY_RULES)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let tools_enabled = read_string(KEY_TOOLS).as_deref() != Some("off");

    Boot {
        sessions,
        settings,
        model: resolution.model,
        mode,
        rules,
        tools_enabled,
        notes,
    }
}

/// Persist sessions; failures surface through the app's storage banner.
pub fn save_sessions(ctx: &AppCtx) {
    // Write a Stage 2 `messages` mirror alongside `events` so rolling the
    // deploy back to the Stage 2 build never wipes history.
    let sessions: Vec<_> = ctx
        .sessions
        .get_untracked()
        .iter()
        .map(|s| s.with_legacy_mirror())
        .collect();
    match serde_json::to_string(&sessions) {
        Ok(raw) => match write_string(KEY_SESSIONS, &raw) {
            Ok(()) => ctx.storage_error.set(None),
            Err(err) => ctx.storage_error.set(Some(err)),
        },
        Err(_) => ctx
            .storage_error
            .set(Some("Could not serialize sessions.".into())),
    }
}

pub fn save_settings(settings: &Settings, ctx: &AppCtx) {
    match serde_json::to_string(settings) {
        Ok(raw) => {
            if let Err(err) = write_string(KEY_SETTINGS, &raw) {
                ctx.storage_error.set(Some(err));
            }
        }
        Err(_) => ctx.storage_error.set(Some("Could not save settings.".into())),
    }
}

pub fn save_model(model: &str, ctx: &AppCtx) {
    if let Err(err) = write_string(KEY_MODEL, model) {
        ctx.storage_error.set(Some(err));
    } else {
        let _ = write_string(KEY_MODEL_MIGRATED, "1"); // any explicit save is a user lock
    }
}

pub fn save_mode(mode: ApprovalMode, ctx: &AppCtx) {
    if let Err(err) = write_string(KEY_MODE, mode.as_str()) {
        ctx.storage_error.set(Some(err));
    }
}

pub fn save_rules(rules: &Rules, ctx: &AppCtx) {
    match serde_json::to_string(rules) {
        Ok(raw) => {
            if let Err(err) = write_string(KEY_RULES, &raw) {
                ctx.storage_error.set(Some(err));
            }
        }
        Err(_) => ctx.storage_error.set(Some("Could not save tool permissions.".into())),
    }
}

pub fn save_tools_enabled(on: bool, ctx: &AppCtx) {
    if let Err(err) = write_string(KEY_TOOLS, if on { "on" } else { "off" }) {
        ctx.storage_error.set(Some(err));
    }
}

/// Throttled by the caller. Failure is silent: it is a best-effort net.
pub fn save_live_checkpoint(session: &str, turn: &str, step: u32, text: &str) {
    let raw = serde_json::json!({ "session": session, "turn": turn, "step": step, "text": text }).to_string();
    let _ = write_string(KEY_LIVE, &raw);
}

pub fn clear_live_checkpoint() {
    if let Some(storage) = local_storage() {
        let _ = storage.remove_item(KEY_LIVE);
    }
}

pub fn title_from(text: &str) -> String {
    let mut title: String = text
        .trim()
        .chars()
        .take(42)
        .collect();
    if text.trim().chars().count() > 42 {
        title.push('…');
    }
    if title.is_empty() {
        NEW_CHAT_TITLE.to_string()
    } else {
        title
    }
}
