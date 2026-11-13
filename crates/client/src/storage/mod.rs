//! Persistence (DESIGN.md §4). Sessions + event logs live in IndexedDB
//! (`idb`); localStorage keeps settings, mode, rules, the live checkpoint and
//! a text-only Stage 2 `messages` mirror of every session (rollback safety).
//! If IndexedDB is unavailable (some private modes) everything falls back to
//! the Stage 3 localStorage path and the storage banner says so. The
//! OpenRouter key NEVER lives here (it only ever exists in server env).

pub mod idb;

use crate::state::AppCtx;
use harness_core::events::{open_turn, EventKind};
use harness_core::permissions::{ApprovalMode, Rules};
use harness_core::{resolve_model, Session, Settings};
use leptos::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, Ordering};

thread_local! {
    /// session id → hash of the JSON last committed to IndexedDB, so a save
    /// only rewrites the sessions that changed.
    static WRITTEN: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

fn hash_of(raw: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    raw.hash(&mut h);
    h.finish()
}

/// What localStorage keeps for a session when IndexedDB holds the truth:
/// title + the Stage 2 text mirror, no event log.
fn mirror_only(s: &Session) -> Session {
    let mut m = s.with_legacy_mirror();
    m.events.clear();
    m
}

/// The IndexedDB record for a session (event log only, no mirror).
fn idb_record(s: &Session) -> Session {
    Session { messages: Vec::new(), ..s.clone() }
}

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

/// Unique across reloads: full ms timestamp + 32 random bits + a counter.
/// (Stage 2/3 cast the timestamp to u32, which saturates to `ffffffff`, so
/// ids restarted at `-1` on every page load and collided with old ones.)
pub fn uid(prefix: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let rand = (js_sys::Math::random() * 4_294_967_296.0) as u64;
    format!("{prefix}-{:x}-{rand:08x}-{n}", js_sys::Date::now() as u64)
}

/// Give colliding session ids (written by the old `uid`) a unique suffix so
/// no IndexedDB record overwrites another. The first occurrence keeps its id.
fn dedupe_session_ids(sessions: &mut [Session]) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut renamed = 0;
    for s in sessions.iter_mut() {
        if !seen.insert(s.id.clone()) {
            let mut i = 2;
            while seen.contains(&format!("{}-dup{i}", s.id)) {
                i += 1;
            }
            s.id = format!("{}-dup{i}", s.id);
            seen.insert(s.id.clone());
            renamed += 1;
        }
    }
    renamed
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
    /// Shown in the storage banner (e.g. IndexedDB unavailable).
    pub storage_error: Option<String>,
}

pub async fn boot() -> Boot {
    let mut notes = Vec::new();
    let mut storage_error = None;

    let mut local_sessions: Vec<Session> = read_string(KEY_SESSIONS)
        .map(|raw| {
            serde_json::from_str(&raw).unwrap_or_else(|err| {
                notes.push(format!(
                    "Saved sessions could not be read ({err}); starting fresh."
                ));
                Vec::new()
            })
        })
        .unwrap_or_default();

    if dedupe_session_ids(&mut local_sessions) > 0 {
        notes.push("Some chats shared an id (an older build's bug); they were kept apart.".to_string());
    }

    // IndexedDB holds the event logs. First run on this build: move the
    // localStorage sessions in (once, nothing dropped).
    let mut first_migration = false;
    let mut sessions: Vec<Session> = match idb::open().await {
        Ok(db) => match idb::get_all(&db).await {
            Ok(rows) => {
                let migrated = rows.iter().any(|(k, _)| k == idb::META_MIGRATED);
                let mut stored = HashMap::new();
                let mut corrupt = 0;
                for (k, v) in rows.iter().filter(|(k, _)| k.starts_with("s:")) {
                    match serde_json::from_str::<Session>(v) {
                        Ok(s) => {
                            WRITTEN.with(|w| w.borrow_mut().insert(s.id.clone(), hash_of(v)));
                            stored.insert(s.id.clone(), s);
                        }
                        Err(_) => {
                            corrupt += 1;
                            let _ = k;
                        }
                    }
                }
                if corrupt > 0 {
                    notes.push(format!("{corrupt} saved chat(s) could not be read and were skipped."));
                }
                if !migrated {
                    first_migration = true;
                }
                harness_core::types::merge_local_and_stored(local_sessions, stored, now_ms())
            }
            Err(err) => {
                storage_error = Some(format!("Chats could not be read from IndexedDB ({err}); using this browser's small localStorage instead."));
                idb::forget();
                local_sessions
            }
        },
        Err(err) => {
            storage_error = Some(format!("{err}; chats are kept in this browser's small localStorage (about 5 MB)."));
            local_sessions
        }
    };

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
            // The partial may already be settled (an async save committed
            // before teardown, the pause did not): never add it twice.
            let already = session.events.iter().any(|e| {
                matches!(&e.kind, EventKind::AssistantMessage { turn, text: t, partial: true, .. } if *turn == open.turn && t == text)
            });
            if matches && !text.is_empty() && !already {
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
    if migrated_any || recovered || first_migration || idb::handle().is_some() {
        if let Err(err) = persist(&sessions, first_migration).await {
            storage_error = Some(err);
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
        storage_error,
    }
}

/// The IndexedDB puts/deletes a save needs (changed sessions only).
fn idb_delta(sessions: &[Session], force_all: bool) -> (Vec<(String, String)>, Vec<String>) {
    let mut puts = Vec::new();
    let live: std::collections::HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    WRITTEN.with(|w| {
        let mut w = w.borrow_mut();
        for s in sessions {
            let Ok(raw) = serde_json::to_string(&idb_record(s)) else { continue };
            let h = hash_of(&raw);
            if force_all || w.get(&s.id) != Some(&h) {
                w.insert(s.id.clone(), h);
                puts.push((idb::session_key(&s.id), raw));
            }
        }
        let gone: Vec<String> = w.keys().filter(|id| !live.contains(id.as_str())).cloned().collect();
        for id in &gone {
            w.remove(id);
        }
        (puts, gone.iter().map(|id| idb::session_key(id)).collect())
    })
}

type IdbJob = (web_sys::IdbDatabase, Vec<(String, String)>, Vec<String>);

/// Synchronous half of a save: the localStorage write (mirror, or the full
/// Stage 3 shape when IndexedDB is unavailable) and the IndexedDB delta. It
/// runs at the moment of the save, so a teardown right after still keeps the
/// mirror, and save order is fixed before any await.
fn stage(sessions: &[Session], first_migration: bool) -> (Result<(), String>, Option<IdbJob>) {
    let Some(db) = idb::handle() else {
        let all: Vec<Session> = sessions.iter().map(|s| s.with_legacy_mirror()).collect();
        let res = serde_json::to_string(&all)
            .map_err(|_| "Could not serialize sessions.".to_string())
            .and_then(|raw| write_string(KEY_SESSIONS, &raw));
        return (res, None);
    };
    let (mut puts, deletes) = idb_delta(sessions, first_migration);
    if first_migration {
        puts.push((idb::META_MIGRATED.to_string(), "1".to_string()));
    }
    let mirror: Vec<Session> = sessions.iter().map(mirror_only).collect();
    let res = serde_json::to_string(&mirror)
        .map_err(|_| "Could not serialize sessions.".to_string())
        .and_then(|raw| write_string(KEY_SESSIONS, &raw));
    (res, Some((db, puts, deletes)))
}

/// Async half: commit the IndexedDB delta.
async fn commit(job: Option<IdbJob>) -> Result<(), String> {
    let Some((db, puts, deletes)) = job else { return Ok(()) };
    if let Err(err) = idb::write(&db, &puts, &deletes).await {
        // Forget the hashes so the next save retries these sessions.
        WRITTEN.with(|w| {
            let mut w = w.borrow_mut();
            for (k, _) in &puts {
                if let Some(id) = k.strip_prefix("s:") {
                    w.remove(id);
                }
            }
        });
        return Err(err);
    }
    Ok(())
}

async fn persist(sessions: &[Session], first_migration: bool) -> Result<(), String> {
    let (local, job) = stage(sessions, first_migration);
    commit(job).await?;
    local
}

/// Persist sessions; failures surface through the app's storage banner.
pub fn save_sessions(ctx: &AppCtx) {
    let sessions = ctx.sessions.get_untracked();
    let (local, job) = stage(&sessions, false);
    let ctx = *ctx;
    let fallback = job.is_none();
    wasm_bindgen_futures::spawn_local(async move {
        match commit(job).await.and(local) {
            Ok(()) => {
                // Keep the "IndexedDB unavailable" notice up in fallback mode.
                if !fallback || ctx.storage_error.get_untracked().as_deref().is_some_and(|e| e.starts_with("Storage is full")) {
                    ctx.storage_error.set(None);
                }
            }
            Err(err) => ctx.storage_error.set(Some(err)),
        }
    });
}

/// Local storage footprint (chars) of everything Harness keeps there.
pub fn local_storage_chars() -> usize {
    let Some(storage) = local_storage() else { return 0 };
    let len = storage.length().unwrap_or(0);
    (0..len)
        .filter_map(|i| storage.key(i).ok().flatten())
        .filter(|k| k.starts_with("harness."))
        .map(|k| k.len() + storage.get_item(&k).ok().flatten().map(|v| v.len()).unwrap_or(0))
        .sum()
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
