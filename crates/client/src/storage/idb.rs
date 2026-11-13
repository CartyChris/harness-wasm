//! IndexedDB store for sessions + event logs (DESIGN.md §4). One object
//! store, `kv`, out-of-line string keys, JSON string values:
//!   `s:<session id>` → one Session (full event log)
//!   `meta:migrated`  → "1" once localStorage sessions were moved in
//! Every call is a Promise wrapper over the raw request API; opening has a
//! timeout so a wedged IndexedDB (seen on some iOS builds) falls back fast.

use js_sys::{Array, Promise};
use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{IdbDatabase, IdbRequest, IdbTransactionMode};

const DB_NAME: &str = "harness";
const DB_VERSION: u32 = 1;
const STORE: &str = "kv";
pub const META_MIGRATED: &str = "meta:migrated";
const OPEN_TIMEOUT_MS: i32 = 2500;

thread_local! {
    static DB: RefCell<Option<IdbDatabase>> = const { RefCell::new(None) };
}

pub fn session_key(id: &str) -> String {
    format!("s:{id}")
}

pub fn forget() {
    DB.with(|d| *d.borrow_mut() = None);
}

pub fn handle() -> Option<IdbDatabase> {
    DB.with(|d| d.borrow().clone())
}

/// Resolve when `req` succeeds (with its result) or reject on error.
fn request_promise(req: &IdbRequest) -> Promise {
    let req = req.clone();
    Promise::new(&mut |resolve, reject| {
        let ok_req = req.clone();
        let on_ok = Closure::once_into_js(move || {
            let _ = resolve.call1(&JsValue::NULL, &ok_req.result().unwrap_or(JsValue::UNDEFINED));
        });
        let on_err = Closure::once_into_js(move || {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("indexeddb request failed"));
        });
        req.set_onsuccess(Some(on_ok.unchecked_ref()));
        req.set_onerror(Some(on_err.unchecked_ref()));
    })
}

fn timeout(ms: i32) -> Promise {
    Promise::new(&mut |_, reject| {
        let cb = Closure::once_into_js(move || {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("indexeddb open timed out"));
        });
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), ms);
        }
    })
}

/// Open (and on first run create) the database. Err → caller falls back to
/// the localStorage path.
pub async fn open() -> Result<IdbDatabase, String> {
    let factory = web_sys::window()
        .and_then(|w| w.indexed_db().ok().flatten())
        .ok_or_else(|| "IndexedDB is unavailable in this browser context".to_string())?;
    let req = factory.open_with_u32(DB_NAME, DB_VERSION).map_err(|_| "IndexedDB refused to open".to_string())?;
    let upgrade_req = req.clone();
    let on_upgrade = Closure::once_into_js(move || {
        if let Ok(result) = upgrade_req.result() {
            let db: IdbDatabase = result.unchecked_into();
            if !db.object_store_names().contains(STORE) {
                let _ = db.create_object_store(STORE);
            }
        }
    });
    req.set_onupgradeneeded(Some(on_upgrade.unchecked_ref()));
    let race = Promise::race(&Array::of2(&request_promise(&req), &timeout(OPEN_TIMEOUT_MS)));
    let db: IdbDatabase = JsFuture::from(race).await.map_err(|e| e.as_string().unwrap_or_else(|| "IndexedDB open failed".into()))?.unchecked_into();
    DB.with(|d| *d.borrow_mut() = Some(db.clone()));
    Ok(db)
}

/// Every (key, value) pair in the store.
pub async fn get_all(db: &IdbDatabase) -> Result<Vec<(String, String)>, String> {
    let tx = db.transaction_with_str(STORE).map_err(|_| "IndexedDB read failed".to_string())?;
    let store = tx.object_store(STORE).map_err(|_| "IndexedDB read failed".to_string())?;
    let keys = JsFuture::from(request_promise(&store.get_all_keys().map_err(|_| "IndexedDB read failed".to_string())?))
        .await
        .map_err(|_| "IndexedDB read failed".to_string())?;
    let values = JsFuture::from(request_promise(&store.get_all().map_err(|_| "IndexedDB read failed".to_string())?))
        .await
        .map_err(|_| "IndexedDB read failed".to_string())?;
    let keys: Array = keys.unchecked_into();
    let values: Array = values.unchecked_into();
    Ok(keys
        .iter()
        .zip(values.iter())
        .filter_map(|(k, v)| Some((k.as_string()?, v.as_string()?)))
        .collect())
}

/// One readwrite transaction: puts then deletes. Resolves when committed.
pub async fn write(db: &IdbDatabase, puts: &[(String, String)], deletes: &[String]) -> Result<(), String> {
    if puts.is_empty() && deletes.is_empty() {
        return Ok(());
    }
    let tx = db
        .transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite)
        .map_err(|_| "IndexedDB write failed".to_string())?;
    let store = tx.object_store(STORE).map_err(|_| "IndexedDB write failed".to_string())?;
    for (k, v) in puts {
        store
            .put_with_key(&JsValue::from_str(v), &JsValue::from_str(k))
            .map_err(|_| "IndexedDB write failed".to_string())?;
    }
    for k in deletes {
        store.delete(&JsValue::from_str(k)).map_err(|_| "IndexedDB write failed".to_string())?;
    }
    let done = Promise::new(&mut |resolve, reject| {
        let ok = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });
        let reject2 = reject.clone();
        let err = Closure::once_into_js(move || {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("Storage is full or blocked — changes may not persist. The live session stays in memory."));
        });
        let abort = Closure::once_into_js(move || {
            let _ = reject2.call1(&JsValue::NULL, &JsValue::from_str("Storage is full or blocked — changes may not persist. The live session stays in memory."));
        });
        tx.set_oncomplete(Some(ok.unchecked_ref()));
        tx.set_onerror(Some(err.unchecked_ref()));
        tx.set_onabort(Some(abort.unchecked_ref()));
    });
    JsFuture::from(done).await.map(|_| ()).map_err(|e| e.as_string().unwrap_or_else(|| "IndexedDB write failed".into()))
}
