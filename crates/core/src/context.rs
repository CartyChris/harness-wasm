//! Deterministic context assembly (Aider pattern, ported: a "repo map" of
//! the workspace built from file names + top-level symbol lines — no
//! embeddings, no retrieval, no randomness — plus whole-turn trimming to a
//! token budget). The same log always yields the same request.

use crate::events::{derive_messages, Event, WireMessage};
use crate::workspace::Workspace;

pub const DEFAULT_BUDGET_TOKENS: usize = 24_000;
pub const REPO_MAP_BUDGET_TOKENS: usize = 1_500;

const SYSTEM_BASE: &str = "You are Harness, a concise coding and writing agent running in the user's phone browser. \
You have a small virtual workspace of text files and tools to list, read, search, write, edit (exact SEARCH/REPLACE), and delete them. \
Use tools when the user asks you to build, change, or inspect files; otherwise just answer. \
After tool work, reply with a short summary. Use fenced code blocks with a language tag for code you show. \
HTML in an ```html block can be previewed by the user.";

/// Cheap, deterministic token estimate (~4 chars/token).
pub fn estimate_tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

fn is_symbol_line(line: &str) -> bool {
    const PREFIXES: [&str; 22] = [
        "fn ", "pub fn ", "async fn ", "pub async fn ", "struct ", "pub struct ", "enum ", "pub enum ",
        "impl ", "trait ", "pub trait ", "mod ", "class ", "def ", "async def ", "function ", "export ",
        "interface ", "type ", "const ", "# ", "## ",
    ];
    let trimmed = line.trim_start();
    // Only top-level-ish lines (≤ 4 columns of indent) count as structure.
    let indent = line.len() - trimmed.len();
    indent <= 4 && PREFIXES.iter().any(|p| trimmed.starts_with(p)) || trimmed.starts_with("<title>")
}

/// Repo map: sorted paths, line counts, and up to 8 symbol lines per file,
/// cut to `budget_tokens`.
pub fn repo_map(ws: &Workspace, budget_tokens: usize) -> String {
    if ws.files.is_empty() {
        return String::new();
    }
    let budget_chars = budget_tokens * 4;
    let mut out = String::new();
    let total = ws.files.len();
    for (shown, (path, content)) in ws.files.iter().enumerate() {
        let mut block = format!("{path} ({} lines)\n", content.lines().count());
        for line in content.lines().filter(|l| is_symbol_line(l)).take(8) {
            let line = line.trim_end();
            let line: String = line.chars().take(96).collect();
            block.push_str(&format!("  │ {line}\n"));
        }
        if out.len() + block.len() > budget_chars {
            out.push_str(&format!("… {} more file(s) not shown\n", total - shown));
            break;
        }
        out.push_str(&block);
    }
    out.trim_end().to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextPlan {
    pub system: String,
    pub repo_map: String,
    /// The full request history: system first, then the kept turns.
    pub messages: Vec<WireMessage>,
    pub kept_groups: usize,
    pub dropped_groups: usize,
    pub est_tokens: usize,
    pub budget_tokens: usize,
}

/// Split history into groups that each start at a user message, so trimming
/// never orphans a tool result from its call.
fn groups(history: Vec<WireMessage>) -> Vec<Vec<WireMessage>> {
    let mut out: Vec<Vec<WireMessage>> = Vec::new();
    for m in history {
        if m.role == "user" || out.is_empty() {
            out.push(vec![m]);
        } else if let Some(last) = out.last_mut() {
            last.push(m);
        }
    }
    out
}

pub fn assemble(events: &[Event], ws: &Workspace, budget_tokens: usize) -> ContextPlan {
    let map = repo_map(ws, REPO_MAP_BUDGET_TOKENS);
    let system = if map.is_empty() {
        format!("{SYSTEM_BASE}\n\nWorkspace: (empty)")
    } else {
        format!("{SYSTEM_BASE}\n\nWorkspace repo map:\n{map}")
    };
    let system_tokens = estimate_tokens(system.len());
    let all = groups(derive_messages(events));
    let total_groups = all.len();
    let mut kept: Vec<Vec<WireMessage>> = Vec::new();
    let mut used = system_tokens;
    for (i, group) in all.into_iter().rev().enumerate() {
        let cost: usize = estimate_tokens(group.iter().map(WireMessage::chars).sum());
        // The newest group is always kept, even over budget.
        if i > 0 && used + cost > budget_tokens {
            break;
        }
        used += cost;
        kept.push(group);
    }
    kept.reverse();
    let kept_groups = kept.len();
    let mut messages = vec![WireMessage::text("system", system.clone())];
    messages.extend(kept.into_iter().flatten());
    ContextPlan {
        system,
        repo_map: map,
        messages,
        kept_groups,
        dropped_groups: total_groups - kept_groups,
        est_tokens: used,
        budget_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{append, EventKind};

    fn ws() -> Workspace {
        let mut ws = Workspace::default();
        ws.files.insert("src/main.rs".into(), "use x;\n\npub fn main() {\n    run();\n}\nstruct App;\n".into());
        ws.files.insert("index.html".into(), "<html>\n<title>Tip</title>\n</html>\n".into());
        ws
    }

    #[test]
    fn repo_map_is_deterministic_and_structural() {
        let a = repo_map(&ws(), 500);
        let b = repo_map(&ws(), 500);
        assert_eq!(a, b);
        assert!(a.starts_with("index.html (3 lines)"), "sorted by path: {a}");
        assert!(a.contains("│ pub fn main() {"));
        assert!(a.contains("│ struct App;"));
        assert!(!a.contains("run();"), "bodies are not in the map");
        assert_eq!(repo_map(&Workspace::default(), 500), "");
    }

    #[test]
    fn repo_map_respects_budget() {
        let mut big = Workspace::default();
        for i in 0..50 {
            big.files.insert(format!("f{i:02}.rs"), "fn a() {}\n".repeat(3));
        }
        let map = repo_map(&big, 50);
        assert!(map.len() <= 50 * 4 + 40);
        assert!(map.contains("more file(s) not shown"));
    }

    #[test]
    fn assemble_trims_oldest_whole_turns() {
        let mut log = Vec::new();
        for i in 0..6 {
            append(&mut log, EventKind::UserMessage { id: format!("u{i}"), text: "x".repeat(2000) }, 0.0);
        }
        let plan = assemble(&log, &ws(), 1500);
        assert_eq!(plan.messages[0].role, "system");
        assert!(plan.messages[0].content.as_deref().unwrap().contains("repo map"));
        assert!(plan.dropped_groups > 0);
        assert_eq!(plan.kept_groups + plan.dropped_groups, 6);
        assert!(plan.kept_groups >= 1);
        let again = assemble(&log, &ws(), 1500);
        assert_eq!(plan, again, "same log → same request");
    }
}
