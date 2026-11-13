//! Approval policy + per-tool permissions.
//!
//! Two patterns, ported (not vendored):
//! - openai/codex: an approval *mode* for the whole agent — `ReadOnly`
//!   (sandbox: no writes), `Ask` (on-request: writes need a human yes), and
//!   `Auto` (full-auto). The sandbox wins over everything else.
//! - anomalyco/opencode: a per-tool `allow | ask | deny` table that overrides
//!   the mode's default for that tool.

use crate::tools::{ToolKind, ToolSpec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    /// Read tools only; every write is denied (codex read-only sandbox).
    ReadOnly,
    /// Reads run, writes ask first (codex on-request). The default.
    #[default]
    Ask,
    /// Everything runs without asking (codex full-auto).
    Auto,
}

impl ApprovalMode {
    pub const ALL: [ApprovalMode; 3] = [ApprovalMode::ReadOnly, ApprovalMode::Ask, ApprovalMode::Auto];

    pub fn label(self) -> &'static str {
        match self {
            ApprovalMode::ReadOnly => "Read-only",
            ApprovalMode::Ask => "Ask",
            ApprovalMode::Auto => "Auto",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            ApprovalMode::ReadOnly => "Reads run. Every write is blocked.",
            ApprovalMode::Ask => "Reads run. Writes wait for your yes.",
            ApprovalMode::Auto => "Everything runs without asking.",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalMode::ReadOnly => "read-only",
            ApprovalMode::Ask => "ask",
            ApprovalMode::Auto => "auto",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read-only" => Some(ApprovalMode::ReadOnly),
            "ask" => Some(ApprovalMode::Ask),
            "auto" => Some(ApprovalMode::Auto),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Allow,
    Ask,
    Deny,
}

impl Permission {
    pub const ALL: [Permission; 3] = [Permission::Allow, Permission::Ask, Permission::Deny];

    pub fn label(self) -> &'static str {
        match self {
            Permission::Allow => "Allow",
            Permission::Ask => "Ask",
            Permission::Deny => "Deny",
        }
    }
}

/// Per-tool overrides (opencode's `permission` table). Missing = use the
/// mode's default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    #[serde(default)]
    pub per_tool: BTreeMap<String, Permission>,
}

impl Rules {
    pub fn get(&self, tool: &str) -> Option<Permission> {
        self.per_tool.get(tool).copied()
    }

    pub fn set(&mut self, tool: &str, permission: Option<Permission>) {
        match permission {
            Some(p) => {
                self.per_tool.insert(tool.to_string(), p);
            }
            None => {
                self.per_tool.remove(tool);
            }
        }
    }
}

/// What the mode alone would say for this tool.
pub fn mode_default(spec: &ToolSpec, mode: ApprovalMode) -> Permission {
    match (mode, spec.kind) {
        (_, ToolKind::Read) => Permission::Allow,
        (ApprovalMode::ReadOnly, ToolKind::Write) => Permission::Deny,
        (ApprovalMode::Ask, ToolKind::Write) => Permission::Ask,
        (ApprovalMode::Auto, ToolKind::Write) => Permission::Allow,
    }
}

/// The effective decision for one tool call. Order: read-only sandbox
/// (writes always denied) → per-tool rule → mode default.
pub fn decide(spec: &ToolSpec, mode: ApprovalMode, rules: &Rules) -> Permission {
    if mode == ApprovalMode::ReadOnly && spec.kind == ToolKind::Write {
        return Permission::Deny;
    }
    rules.get(spec.name).unwrap_or_else(|| mode_default(spec, mode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::spec;

    #[test]
    fn codex_modes_set_defaults() {
        let read = spec("read_file").unwrap();
        let write = spec("write_file").unwrap();
        let rules = Rules::default();
        assert_eq!(decide(read, ApprovalMode::ReadOnly, &rules), Permission::Allow);
        assert_eq!(decide(write, ApprovalMode::ReadOnly, &rules), Permission::Deny);
        assert_eq!(decide(read, ApprovalMode::Ask, &rules), Permission::Allow);
        assert_eq!(decide(write, ApprovalMode::Ask, &rules), Permission::Ask);
        assert_eq!(decide(write, ApprovalMode::Auto, &rules), Permission::Allow);
        assert_eq!(ApprovalMode::default(), ApprovalMode::Ask);
    }

    #[test]
    fn opencode_rules_override_mode() {
        let write = spec("write_file").unwrap();
        let read = spec("search_files").unwrap();
        let mut rules = Rules::default();
        rules.set("write_file", Some(Permission::Allow));
        rules.set("search_files", Some(Permission::Deny));
        assert_eq!(decide(write, ApprovalMode::Ask, &rules), Permission::Allow);
        assert_eq!(decide(read, ApprovalMode::Auto, &rules), Permission::Deny);
        rules.set("write_file", Some(Permission::Ask));
        assert_eq!(decide(write, ApprovalMode::Auto, &rules), Permission::Ask);
        rules.set("write_file", None);
        assert_eq!(decide(write, ApprovalMode::Auto, &rules), Permission::Allow);
    }

    #[test]
    fn read_only_sandbox_beats_allow_rule() {
        let write = spec("edit_file").unwrap();
        let mut rules = Rules::default();
        rules.set("edit_file", Some(Permission::Allow));
        assert_eq!(decide(write, ApprovalMode::ReadOnly, &rules), Permission::Deny);
    }

    #[test]
    fn mode_and_rules_roundtrip_json() {
        let mut rules = Rules::default();
        rules.set("delete_file", Some(Permission::Deny));
        let raw = serde_json::to_string(&rules).unwrap();
        assert!(raw.contains("\"deny\""));
        let back: Rules = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, rules);
        for m in ApprovalMode::ALL {
            assert_eq!(ApprovalMode::parse(m.as_str()), Some(m));
        }
    }
}
