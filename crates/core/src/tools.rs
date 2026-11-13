//! Browser-side tools the model may call. They act only on the derived
//! virtual workspace — no host shell, no network (DESIGN.md §1, §6).

use crate::workspace::{apply_search_replace, clean_path, FileEdit, Workspace, MAX_FILES, MAX_FILE_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const MAX_OUTPUT_CHARS: usize = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Write,
}

#[derive(Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub kind: ToolKind,
    pub title: &'static str,
    pub description: &'static str,
    /// JSON Schema for the arguments.
    pub params: &'static str,
}

pub static REGISTRY: [ToolSpec; 6] = [
    ToolSpec {
        name: "list_files",
        kind: ToolKind::Read,
        title: "List files",
        description: "List every file in the workspace with its line count.",
        params: r#"{"type":"object","properties":{},"additionalProperties":false}"#,
    },
    ToolSpec {
        name: "read_file",
        kind: ToolKind::Read,
        title: "Read file",
        description: "Read a workspace file. Returns numbered lines.",
        params: r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
    },
    ToolSpec {
        name: "search_files",
        kind: ToolKind::Read,
        title: "Search",
        description: "Case-insensitive substring search across workspace files. Returns path:line: text.",
        params: r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}"#,
    },
    ToolSpec {
        name: "write_file",
        kind: ToolKind::Write,
        title: "Write file",
        description: "Create or overwrite a workspace file with the full content.",
        params: r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}"#,
    },
    ToolSpec {
        name: "edit_file",
        kind: ToolKind::Write,
        title: "Edit file",
        description: "Replace one exact occurrence of `search` with `replace` in a file (SEARCH/REPLACE edit). `search` must match exactly once.",
        params: r#"{"type":"object","properties":{"path":{"type":"string"},"search":{"type":"string"},"replace":{"type":"string"}},"required":["path","search","replace"],"additionalProperties":false}"#,
    },
    ToolSpec {
        name: "delete_file",
        kind: ToolKind::Write,
        title: "Delete file",
        description: "Delete a workspace file.",
        params: r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
    },
];

pub fn spec(name: &str) -> Option<&'static ToolSpec> {
    REGISTRY.iter().find(|s| s.name == name)
}

/// OpenAI-compatible `tools` array for the chat request.
pub fn tools_json() -> Value {
    Value::Array(
        REGISTRY
            .iter()
            .map(|s| {
                json!({
                    "type": "function",
                    "function": {
                        "name": s.name,
                        "description": s.description,
                        "parameters": serde_json::from_str::<Value>(s.params).unwrap_or(json!({"type":"object"})),
                    }
                })
            })
            .collect(),
    )
}

/// One complete tool call from the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON argument string, exactly as the model produced it.
    pub arguments: String,
}

impl ToolCall {
    pub fn args(&self) -> Value {
        serde_json::from_str(&self.arguments).unwrap_or(Value::Null)
    }

    /// Short human label: `write_file · index.html`.
    pub fn headline(&self) -> String {
        let args = self.args();
        let target = args
            .get("path")
            .or_else(|| args.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if target.is_empty() {
            self.name.clone()
        } else {
            format!("{} · {}", self.name, truncate(target, 48))
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub ok: bool,
    pub output: String,
    pub edit: Option<FileEdit>,
}

fn fail(msg: impl Into<String>) -> ToolOutcome {
    ToolOutcome { ok: false, output: msg.into(), edit: None }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Execute a call against the workspace. Pure: returns the edit to log; the
/// workspace itself only changes when the log is re-derived.
pub fn execute(call: &ToolCall, ws: &Workspace) -> ToolOutcome {
    let Some(spec) = spec(&call.name) else {
        return fail(format!("unknown tool `{}`", truncate(&call.name, 40)));
    };
    let args: Value = match serde_json::from_str(if call.arguments.trim().is_empty() { "{}" } else { &call.arguments }) {
        Ok(v) => v,
        Err(_) => return fail("arguments are not valid JSON"),
    };
    let path = || -> Result<String, ToolOutcome> {
        let raw = arg_str(&args, "path").ok_or_else(|| fail("missing `path`"))?;
        clean_path(raw).ok_or_else(|| fail("invalid path (relative, a-z 0-9 . _ - / only, no ..)"))
    };
    let outcome = match spec.name {
        "list_files" => {
            if ws.files.is_empty() {
                ToolOutcome { ok: true, output: "(workspace is empty)".into(), edit: None }
            } else {
                let listing: Vec<String> = ws
                    .files
                    .iter()
                    .map(|(p, c)| format!("{p} ({} lines)", c.lines().count()))
                    .collect();
                ToolOutcome { ok: true, output: listing.join("\n"), edit: None }
            }
        }
        "read_file" => match path() {
            Err(e) => e,
            Ok(p) => match ws.get(&p) {
                None => fail(format!("{p}: no such file")),
                Some(content) => {
                    let numbered: Vec<String> = content
                        .lines()
                        .take(400)
                        .enumerate()
                        .map(|(i, l)| format!("{:>4}  {l}", i + 1))
                        .collect();
                    ToolOutcome { ok: true, output: numbered.join("\n"), edit: None }
                }
            },
        },
        "search_files" => {
            let Some(query) = arg_str(&args, "query").filter(|q| !q.trim().is_empty()) else {
                return fail("missing `query`");
            };
            let needle = query.to_lowercase();
            let mut hits = Vec::new();
            for (p, content) in &ws.files {
                for (i, line) in content.lines().enumerate() {
                    if line.to_lowercase().contains(&needle) {
                        hits.push(format!("{p}:{}: {}", i + 1, truncate(line.trim(), 120)));
                    }
                }
            }
            if hits.is_empty() {
                ToolOutcome { ok: true, output: format!("no matches for `{}`", truncate(query, 60)), edit: None }
            } else {
                ToolOutcome { ok: true, output: hits.into_iter().take(80).collect::<Vec<_>>().join("\n"), edit: None }
            }
        }
        "write_file" => match path() {
            Err(e) => e,
            Ok(p) => {
                let Some(content) = arg_str(&args, "content") else {
                    return fail("missing `content`");
                };
                if content.len() > MAX_FILE_BYTES {
                    return fail("file too large for the phone workspace (200 KB max)");
                }
                let before = ws.get(&p).cloned();
                if before.is_none() && ws.files.len() >= MAX_FILES {
                    return fail("workspace is full (64 files)");
                }
                let verb = if before.is_some() { "updated" } else { "created" };
                ToolOutcome {
                    ok: true,
                    output: format!("{verb} {p} ({} lines)", content.lines().count()),
                    edit: Some(FileEdit { path: p, before, after: Some(content.to_string()) }),
                }
            }
        },
        "edit_file" => match path() {
            Err(e) => e,
            Ok(p) => {
                let (Some(search), Some(replace)) = (arg_str(&args, "search"), arg_str(&args, "replace")) else {
                    return fail("missing `search` or `replace`");
                };
                let Some(before) = ws.get(&p).cloned() else {
                    return fail(format!("{p}: no such file"));
                };
                match apply_search_replace(&before, search, replace) {
                    Err(e) => fail(format!("{p}: {e}")),
                    Ok(after) => {
                        if after.len() > MAX_FILE_BYTES {
                            return fail("file too large for the phone workspace (200 KB max)");
                        }
                        ToolOutcome {
                            ok: true,
                            output: format!("edited {p}"),
                            edit: Some(FileEdit { path: p, before: Some(before), after: Some(after) }),
                        }
                    }
                }
            }
        },
        "delete_file" => match path() {
            Err(e) => e,
            Ok(p) => match ws.get(&p).cloned() {
                None => fail(format!("{p}: no such file")),
                Some(before) => ToolOutcome {
                    ok: true,
                    output: format!("deleted {p}"),
                    edit: Some(FileEdit { path: p, before: Some(before), after: None }),
                },
            },
        },
        _ => fail("unhandled tool"),
    };
    ToolOutcome { output: truncate(&outcome.output, MAX_OUTPUT_CHARS), ..outcome }
}

/// One streamed fragment of `choices[0].delta.tool_calls[i]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCallFragment {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// Accumulates streamed tool-call fragments into complete calls (OpenAI /
/// OpenRouter streaming shape: id + name arrive first, arguments in pieces).
#[derive(Debug, Clone, Default)]
pub struct ToolCallAccumulator {
    slots: Vec<(Option<String>, String, String)>,
}

impl ToolCallAccumulator {
    pub fn push(&mut self, frag: &ToolCallFragment) {
        if frag.index >= 32 {
            return;
        }
        while self.slots.len() <= frag.index {
            self.slots.push((None, String::new(), String::new()));
        }
        let slot = &mut self.slots[frag.index];
        if let Some(id) = &frag.id {
            if !id.is_empty() {
                slot.0 = Some(id.clone());
            }
        }
        if let Some(name) = &frag.name {
            slot.1.push_str(name);
        }
        if let Some(args) = &frag.arguments {
            slot.2.push_str(args);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.1.is_empty())
    }

    /// Name of the call currently being streamed (for the live status).
    pub fn current_name(&self) -> Option<String> {
        self.slots.iter().rev().find(|s| !s.1.is_empty()).map(|s| s.1.clone())
    }

    pub fn finish(&self, turn_salt: &str) -> Vec<ToolCall> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.1.is_empty())
            .map(|(i, s)| ToolCall {
                id: s.0.clone().unwrap_or_else(|| format!("call_{turn_salt}_{i}")),
                name: s.1.clone(),
                arguments: if s.2.trim().is_empty() { "{}".into() } else { s.2.clone() },
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall { id: "c1".into(), name: name.into(), arguments: args.to_string() }
    }

    #[test]
    fn registry_json_is_valid_openai_tools() {
        let tools = tools_json();
        let arr = tools.as_array().unwrap();
        assert_eq!(arr.len(), REGISTRY.len());
        for t in arr {
            assert_eq!(t["type"], "function");
            assert_eq!(t["function"]["parameters"]["type"], "object");
        }
    }

    #[test]
    fn write_then_read_via_edits() {
        let ws = Workspace::default();
        let out = execute(&call("write_file", json!({"path":"app.py","content":"print(1)\n"})), &ws);
        assert!(out.ok);
        let edit = out.edit.unwrap();
        assert_eq!(edit.before, None);
        let mut ws2 = Workspace::default();
        ws2.files.insert(edit.path.clone(), edit.after.clone().unwrap());
        let read = execute(&call("read_file", json!({"path":"app.py"})), &ws2);
        assert!(read.output.contains("1  print(1)"));
        let edited = execute(&call("edit_file", json!({"path":"app.py","search":"1","replace":"2"})), &ws2);
        assert_eq!(edited.edit.unwrap().after.as_deref(), Some("print(2)\n"));
    }

    #[test]
    fn bad_calls_fail_softly() {
        let ws = Workspace::default();
        assert!(!execute(&call("nope", json!({})), &ws).ok);
        assert!(!execute(&ToolCall { id: "x".into(), name: "read_file".into(), arguments: "{bad".into() }, &ws).ok);
        assert!(!execute(&call("read_file", json!({"path":"../x"})), &ws).ok);
        assert!(!execute(&call("delete_file", json!({"path":"missing.txt"})), &ws).ok);
        assert!(execute(&call("list_files", json!({})), &ws).output.contains("empty"));
    }

    #[test]
    fn accumulator_joins_fragments() {
        let mut acc = ToolCallAccumulator::default();
        acc.push(&ToolCallFragment { index: 0, id: Some("call_a".into()), name: Some("write_file".into()), arguments: Some("{\"pa".into()) });
        acc.push(&ToolCallFragment { index: 0, arguments: Some("th\":\"a\",\"content\":\"x\"}".into()), ..Default::default() });
        acc.push(&ToolCallFragment { index: 1, name: Some("list_files".into()), ..Default::default() });
        assert_eq!(acc.current_name().as_deref(), Some("list_files"));
        let calls = acc.finish("t9");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].args()["path"], "a");
        assert_eq!(calls[1].id, "call_t9_1");
        assert_eq!(calls[1].arguments, "{}");
        assert_eq!(calls[0].headline(), "write_file · a");
    }
}
