//! Browser-local virtual workspace the agent's tools act on.
//!
//! State is never stored on its own: it is *derived* by folding the
//! `tool/result` edits in the append-only log, skipping edits that an
//! `edit/undo` event reverted (Aider's "undo is a first-class action",
//! ported as a log event instead of a git commit).

use crate::events::{Event, EventKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_FILE_BYTES: usize = 200 * 1024;
pub const MAX_FILES: usize = 64;

/// One file change. `before`/`after` of `None` mean "absent" (create /
/// delete).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEdit {
    pub path: String,
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Workspace {
    pub files: BTreeMap<String, String>,
}

impl Workspace {
    /// Fold the log's edits (in order), skipping undone ones.
    pub fn derive(events: &[Event]) -> Self {
        let undone = undone_set(events);
        let mut files = BTreeMap::new();
        for event in events {
            if let EventKind::ToolResult { edit: Some(edit), .. } = &event.kind {
                if undone.contains(&event.seq) {
                    continue;
                }
                match &edit.after {
                    Some(content) => {
                        files.insert(edit.path.clone(), content.clone());
                    }
                    None => {
                        files.remove(&edit.path);
                    }
                }
            }
        }
        Workspace { files }
    }

    pub fn get(&self, path: &str) -> Option<&String> {
        self.files.get(path)
    }

    pub fn total_lines(&self) -> usize {
        self.files.values().map(|c| c.lines().count()).sum()
    }
}

pub fn undone_set(events: &[Event]) -> BTreeSet<u64> {
    events
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::EditUndo { edit_seq } => Some(edit_seq),
            _ => None,
        })
        .collect()
}

/// An edit can be undone when it is not already undone and it is the latest
/// live edit of its path (Aider only undoes its own last change — undoing an
/// older edit under a newer one would silently resurrect stale text).
pub fn can_undo(events: &[Event], edit_seq: u64) -> bool {
    let undone = undone_set(events);
    if undone.contains(&edit_seq) {
        return false;
    }
    let Some(path) = events.iter().find_map(|e| match &e.kind {
        EventKind::ToolResult { edit: Some(edit), .. } if e.seq == edit_seq => Some(edit.path.clone()),
        _ => None,
    }) else {
        return false;
    };
    let latest = events
        .iter()
        .filter(|e| !undone.contains(&e.seq))
        .filter_map(|e| match &e.kind {
            EventKind::ToolResult { edit: Some(edit), .. } if edit.path == path => Some(e.seq),
            _ => None,
        })
        .max();
    latest == Some(edit_seq)
}

/// Normalise a model-supplied path: relative, no `..`, no empty segments,
/// conservative charset. Returns `None` for anything suspicious.
pub fn clean_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches("./").trim_start_matches('/');
    if trimmed.is_empty() || trimmed.len() > 120 {
        return None;
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        if !part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return None;
        }
        parts.push(part);
    }
    Some(parts.join("/"))
}

/// Aider-style SEARCH/REPLACE: the search text must match exactly once.
pub fn apply_search_replace(content: &str, search: &str, replace: &str) -> Result<String, String> {
    if search.is_empty() {
        return Err("search text is empty".into());
    }
    let count = content.matches(search).count();
    match count {
        0 => Err("search text not found — read_file first and copy it exactly".into()),
        1 => Ok(content.replacen(search, replace, 1)),
        n => Err(format!("search text matches {n} places — include more context")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTag {
    Same,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub tag: DiffTag,
    pub text: String,
}

/// Line diff (LCS). Large inputs fall back to "all removed / all added" so
/// the phone never does quadratic work on a big file.
pub fn line_diff(before: &str, after: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    if a.len() * b.len() > 250_000 {
        let mut out: Vec<DiffLine> = a.iter().map(|l| DiffLine { tag: DiffTag::Del, text: l.to_string() }).collect();
        out.extend(b.iter().map(|l| DiffLine { tag: DiffTag::Add, text: l.to_string() }));
        return out;
    }
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffLine { tag: DiffTag::Same, text: a[i].to_string() });
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(DiffLine { tag: DiffTag::Del, text: a[i].to_string() });
            i += 1;
        } else {
            out.push(DiffLine { tag: DiffTag::Add, text: b[j].to_string() });
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| DiffLine { tag: DiffTag::Del, text: l.to_string() }));
    out.extend(b[j..].iter().map(|l| DiffLine { tag: DiffTag::Add, text: l.to_string() }));
    out
}

/// `(+added, -removed)` for a diff card header.
pub fn diff_stats(lines: &[DiffLine]) -> (usize, usize) {
    let add = lines.iter().filter(|l| l.tag == DiffTag::Add).count();
    let del = lines.iter().filter(|l| l.tag == DiffTag::Del).count();
    (add, del)
}

/// Collapse long unchanged runs to keep a diff card short: keep `context`
/// lines around each change; a `None` marks an elided gap.
pub fn diff_hunks(lines: &[DiffLine], context: usize) -> Vec<Option<DiffLine>> {
    let changed: Vec<bool> = lines.iter().map(|l| l.tag != DiffTag::Same).collect();
    let mut keep = vec![false; lines.len()];
    for (i, c) in changed.iter().enumerate() {
        if *c {
            let lo = i.saturating_sub(context);
            let hi = (i + context + 1).min(lines.len());
            for k in keep.iter_mut().take(hi).skip(lo) {
                *k = true;
            }
        }
    }
    let mut out = Vec::new();
    let mut gap = false;
    for (i, line) in lines.iter().enumerate() {
        if keep[i] {
            out.push(Some(line.clone()));
            gap = false;
        } else if !gap {
            out.push(None);
            gap = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event, EventKind};

    fn result(seq: u64, path: &str, before: Option<&str>, after: Option<&str>) -> Event {
        Event {
            seq,
            at: 0.0,
            kind: EventKind::ToolResult {
                turn: "t1".into(),
                call_id: format!("c{seq}"),
                name: "write_file".into(),
                ok: true,
                output: String::new(),
                edit: Some(FileEdit {
                    path: path.into(),
                    before: before.map(str::to_string),
                    after: after.map(str::to_string),
                }),
            },
        }
    }

    #[test]
    fn workspace_folds_edits_and_undo_restores() {
        let mut events = vec![
            result(1, "a.txt", None, Some("one")),
            result(2, "a.txt", Some("one"), Some("two")),
            result(3, "b.txt", None, Some("bee")),
        ];
        let ws = Workspace::derive(&events);
        assert_eq!(ws.get("a.txt").map(String::as_str), Some("two"));
        assert!(can_undo(&events, 2));
        assert!(!can_undo(&events, 1), "older edit under a newer one is not undoable");
        events.push(Event { seq: 4, at: 0.0, kind: EventKind::EditUndo { edit_seq: 2 } });
        let ws = Workspace::derive(&events);
        assert_eq!(ws.get("a.txt").map(String::as_str), Some("one"));
        assert!(!can_undo(&events, 2), "already undone");
        assert!(can_undo(&events, 1), "now the latest live edit");
        events.push(Event { seq: 5, at: 0.0, kind: EventKind::EditUndo { edit_seq: 1 } });
        assert!(Workspace::derive(&events).get("a.txt").is_none(), "create undone = gone");
        assert_eq!(events.len(), 5, "undo appends, never removes");
    }

    #[test]
    fn delete_edit_removes_file() {
        let events = vec![result(1, "x.rs", None, Some("fn x() {}")), result(2, "x.rs", Some("fn x() {}"), None)];
        assert!(Workspace::derive(&events).files.is_empty());
    }

    #[test]
    fn path_cleaning_blocks_escapes() {
        assert_eq!(clean_path("./src/main.rs"), Some("src/main.rs".into()));
        assert_eq!(clean_path("/abs.txt"), Some("abs.txt".into()));
        assert_eq!(clean_path("../etc/passwd"), None);
        assert_eq!(clean_path("a//b"), None);
        assert_eq!(clean_path("a/<script>.js"), None);
        assert_eq!(clean_path(""), None);
    }

    #[test]
    fn search_replace_exactly_once() {
        assert_eq!(apply_search_replace("a b c", "b", "B").unwrap(), "a B c");
        assert!(apply_search_replace("a b c", "z", "B").is_err());
        assert!(apply_search_replace("b b", "b", "B").unwrap_err().contains("2 places"));
    }

    #[test]
    fn line_diff_marks_changes() {
        let d = line_diff("a\nb\nc\n", "a\nB\nc\nd\n");
        let tags: Vec<DiffTag> = d.iter().map(|l| l.tag).collect();
        assert_eq!(tags, vec![DiffTag::Same, DiffTag::Del, DiffTag::Add, DiffTag::Same, DiffTag::Add]);
        assert_eq!(diff_stats(&d), (2, 1));
        let long_before: String = (0..40).map(|i| format!("l{i}\n")).collect();
        let long_after = long_before.replace("l20\n", "L20\n");
        let hunks = diff_hunks(&line_diff(&long_before, &long_after), 2);
        assert!(hunks.len() < 12, "unchanged runs are elided");
        assert!(hunks.iter().any(|h| h.is_none()));
    }
}
