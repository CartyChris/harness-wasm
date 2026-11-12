//! localStorage persistence. Sessions, settings, and the locked model live
//! here; the OpenRouter key NEVER does (it only ever exists in server env).

use crate::state::AppCtx;
use harness_core::{resolve_model, Session, Settings};
use leptos::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

pub const KEY_SESSIONS: &str = "harness.sessions";
pub const KEY_SETTINGS: &str = "harness.settings";
pub const KEY_MODEL: &str = "harness.model";
pub const KEY_MODEL_MIGRATED: &str = "harness.model_migrated";

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
    pub notes: Vec<String>,
}

pub fn boot() -> Boot {
    let mut notes = Vec::new();

    let sessions: Vec<Session> = read_string(KEY_SESSIONS)
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

    Boot {
        sessions,
        settings,
        model: resolution.model,
        notes,
    }
}

/// Persist sessions; failures surface through the app's storage banner.
pub fn save_sessions(ctx: &AppCtx) {
    let sessions = ctx.sessions.get_untracked();
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
