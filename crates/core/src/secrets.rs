//! Key-shape detection and redaction for anything that leaves or enters the
//! phone as a file (export / import, DESIGN.md §4–§5). The client never holds
//! an OpenRouter key; this keeps one from riding in on an imported file or out
//! in an export someone pasted into a chat.

const KEY_CHARS: fn(char) -> bool = |c| c.is_ascii_alphanumeric() || c == '-' || c == '_';
/// Env-var names that only ever precede a real secret.
const ENV_NAMES: [&str; 3] = ["openrouter_api_key", "zai_api_key", "harness_access_token"];

fn run_len(text: &str, from: usize) -> usize {
    text[from..].chars().take_while(|c| KEY_CHARS(*c) || *c == '.').map(char::len_utf8).sum()
}

/// Byte ranges of key-shaped VALUES (what gets redacted / refused):
/// `sk-or-` + 8 or more key chars, `Bearer ` + 20 or more token chars, and
/// the value after `OPENROUTER_API_KEY=` / `ZAI_API_KEY: "…"` (8+ chars).
/// Merely mentioning a prefix or an env-var name is fine.
fn key_spans(text: &str) -> Vec<(usize, usize, &'static str)> {
    let lower = text.to_ascii_lowercase();
    let mut spans = Vec::new();
    for (prefix, min, why) in [
        ("sk-or-", 8usize, "it contains an OpenRouter-style key (sk-or-…)"),
        ("bearer ", 20usize, "it contains a bearer token"),
    ] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(prefix) {
            let start = from + i;
            let body = start + prefix.len();
            let run = run_len(text, body);
            if run >= min {
                spans.push((start, body + run, why));
            }
            from = body;
        }
    }
    for name in ENV_NAMES {
        let mut from = 0;
        while let Some(i) = lower[from..].find(name) {
            let mut j = from + i + name.len();
            // optional closing quote, spaces, then = or :, spaces, opening quote
            let bytes = text.as_bytes();
            while j < bytes.len() && matches!(bytes[j], b'"' | b'\'' | b' ' | b'\\') {
                j += 1;
            }
            if j < bytes.len() && matches!(bytes[j], b'=' | b':') {
                j += 1;
                while j < bytes.len() && matches!(bytes[j], b'"' | b'\'' | b' ' | b'\\') {
                    j += 1;
                }
                let run = run_len(text, j);
                if run >= 8 {
                    spans.push((j, j + run, "it contains a secret env-var value (OPENROUTER_API_KEY / ZAI_API_KEY / HARNESS_ACCESS_TOKEN)"));
                }
            }
            from = from + i + name.len();
        }
    }
    spans.sort();
    spans
}

/// Why a text must be refused, or None when it is clean.
pub fn find_key_shape(text: &str) -> Option<&'static str> {
    key_spans(text).first().map(|s| s.2)
}

/// Replace every key-shaped value with `[redacted]`. Output is JSON-safe when
/// the input is JSON (key chars never include quotes or backslashes).
pub fn redact_key_shapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (a, b, _) in key_spans(text) {
        if a < last {
            continue;
        }
        out.push_str(&text[last..a]);
        out.push_str("[redacted]");
        last = b;
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_key() -> String {
        ["sk-", "or-", "v1-", "abcdef0123456789abcdef"].concat()
    }

    #[test]
    fn detects_and_redacts_key_shapes() {
        let k = fake_key();
        assert!(find_key_shape(&format!("my key is {k} ok")).is_some());
        assert!(find_key_shape(&["OPENROUTER", "_API_KEY=", "abcdefgh12345678"].concat()).is_some());
        assert!(find_key_shape(&[r#"{"ZAI"#, r#"_API_KEY": "abcdefgh12345678"}"#].concat()).is_some());
        // Mentions are fine: a chat about setting the env var must re-import.
        assert!(find_key_shape("set OPENROUTER_API_KEY in env; keys start with sk-or-").is_none());
        assert!(find_key_shape("ZAI_API_KEY=").is_none());
        assert!(find_key_shape("a normal chat about keys and tokens").is_none());
        assert!(find_key_shape("risk-orange").is_none());
        let bearer = ["Bear", "er ", "abcdefghijklmnopqrstuvwxyz0123"].concat();
        let red = redact_key_shapes(&format!(r#"{{"t":"use {k} and {bearer}"}}"#));
        assert_eq!(red, r#"{"t":"use [redacted] and [redacted]"}"#);
        assert!(find_key_shape(&red).is_none());
        // Invariant: anything redacted is importable.
        let env = ["OPENROUTER", "_API_KEY=", "abcdefgh12345678 rest"].concat();
        assert_eq!(redact_key_shapes(&env), ["OPENROUTER", "_API_KEY=[redacted] rest"].concat());
        assert!(find_key_shape(&redact_key_shapes(&env)).is_none());
        let _: serde_json::Value = serde_json::from_str(&red).unwrap();
    }
}
