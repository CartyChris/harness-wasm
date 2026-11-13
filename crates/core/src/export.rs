//! Chat export / import (DESIGN.md §4). The export carries sessions and
//! their event logs only: never settings, the proxy URL, or the access token;
//! key-shaped values are redacted. Import refuses any key-shaped file outright
//! and never writes anything from a refused file.

use crate::secrets::{find_key_shape, redact_key_shapes};
use crate::types::Session;
use serde::{Deserialize, Serialize};

pub const EXPORT_FORMAT: &str = "harness-chats";
pub const EXPORT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportFile {
    pub format: String,
    pub version: u32,
    pub exported_at: f64,
    pub sessions: Vec<Session>,
}

/// Serialize sessions for download. `strip` holds values that must never
/// appear in the file (access token, proxy URL); each is replaced wherever it
/// occurs, including inside chat text.
pub fn build_export(sessions: &[Session], at: f64, strip: &[&str]) -> String {
    let file = ExportFile {
        format: EXPORT_FORMAT.into(),
        version: EXPORT_VERSION,
        exported_at: at,
        sessions: sessions.iter().map(|s| Session { messages: Vec::new(), ..s.clone() }).collect(),
    };
    let mut raw = serde_json::to_string_pretty(&file).unwrap_or_default();
    for value in strip.iter().map(|v| v.trim()).filter(|v| v.len() >= 4) {
        // Match the JSON-escaped form, as it sits in the file.
        let escaped = serde_json::to_string(value).unwrap_or_default();
        let escaped = escaped.trim_matches('"');
        if !escaped.is_empty() {
            raw = raw.replace(escaped, "[stripped]");
        }
    }
    redact_key_shapes(&raw)
}

/// Parse an import file. Errors are plain sentences for the UI.
pub fn parse_import(raw: &str) -> Result<Vec<Session>, String> {
    if let Some(why) = find_key_shape(raw) {
        return Err(format!("Import refused: {why}. Nothing was saved."));
    }
    let file: ExportFile = serde_json::from_str(raw).map_err(|_| "Import failed: this is not a Harness chat export.".to_string())?;
    if file.format != EXPORT_FORMAT {
        return Err("Import failed: this is not a Harness chat export.".into());
    }
    if file.version > EXPORT_VERSION {
        return Err("Import failed: this export comes from a newer Harness.".into());
    }
    let mut sessions = file.sessions;
    for s in sessions.iter_mut() {
        s.migrate_legacy(s.updated_at);
    }
    Ok(sessions)
}

/// Merge imported sessions: new ids are added, a newer copy of a known id
/// replaces it, older or identical copies are skipped. Returns (added or
/// updated, skipped).
pub fn merge_import(existing: &mut Vec<Session>, incoming: Vec<Session>) -> (usize, usize) {
    let (mut added, mut skipped) = (0, 0);
    for s in incoming {
        match existing.iter_mut().find(|e| e.id == s.id) {
            Some(e) if s.updated_at > e.updated_at => {
                *e = s;
                added += 1;
            }
            Some(_) => skipped += 1,
            None => {
                existing.push(s);
                added += 1;
            }
        }
    }
    existing.sort_by(|a, b| b.updated_at.partial_cmp(&a.updated_at).unwrap_or(std::cmp::Ordering::Equal));
    (added, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventKind;

    fn session(id: &str, text: &str, at: f64) -> Session {
        let mut s = Session::new(id.into(), "T".into(), at);
        s.append(EventKind::UserMessage { id: format!("{id}-u"), text: text.into() }, at);
        s
    }

    #[test]
    fn export_strips_token_and_url_and_round_trips() {
        let token = "gate-token-0123456789abcdef";
        let url = "https://my-proxy.example.app";
        let s = session("s1", &format!("my token is {token} at {url}"), 5.0);
        let raw = build_export(&[s.clone()], 9.0, &[token, url]);
        assert!(!raw.contains(token) && !raw.contains(url));
        assert!(raw.contains("[stripped]"));
        let back = parse_import(&raw).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, "s1");
        assert_eq!(back[0].events.len(), s.events.len());
    }

    #[test]
    fn import_refuses_key_shaped_files_and_bad_json() {
        let poisoned = format!(r#"{{"format":"harness-chats","version":1,"exported_at":1,"sessions":[],"k":"{}"}}"#, ["sk-or-", "v1-", "0123456789abcdef0123"].concat());
        let err = parse_import(&poisoned).unwrap_err();
        assert!(err.starts_with("Import refused"), "{err}");
        let env = [r#"{"x":"OPENROUTER"#, r#"_API_KEY=abcdefgh12345678"}"#].concat();
        assert!(parse_import(&env).unwrap_err().starts_with("Import refused"));
        assert!(parse_import("not json").unwrap_err().contains("not a Harness chat export"));
        assert!(parse_import(r#"{"format":"other","version":1,"exported_at":1,"sessions":[]}"#).is_err());
    }

    #[test]
    fn merge_adds_new_replaces_newer_skips_rest() {
        let mut mine = vec![session("a", "old", 1.0), session("b", "keep", 5.0)];
        let (added, skipped) = merge_import(&mut mine, vec![session("a", "new", 2.0), session("b", "older", 4.0), session("c", "fresh", 3.0)]);
        assert_eq!((added, skipped), (2, 1));
        assert_eq!(mine.len(), 3);
        assert_eq!(mine.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["b", "c", "a"]);
    }
}
