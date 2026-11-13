//! Small, safe Markdown → AST for assistant text. The renderer builds DOM
//! nodes from this AST with text nodes only; raw HTML in model output is
//! never interpreted (DESIGN.md §6). Links survive only as `https:`/`http:`.

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Para(Vec<Span>),
    Heading(u8, Vec<Span>),
    List { ordered: bool, start: u32, items: Vec<Vec<Span>> },
    Quote(Vec<Span>),
    Code { lang: String, body: String, closed: bool },
    Table { head: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>> },
    Rule,
}

pub fn safe_href(url: &str) -> Option<String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    if (lower.starts_with("https://") || lower.starts_with("http://"))
        && !u.chars().any(|c| c.is_whitespace() || c == '"' || c == '<' || c == '>')
    {
        Some(u.to_string())
    } else {
        None
    }
}

pub fn inline(src: &str) -> Vec<Span> {
    let chars: Vec<char> = src.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    let mut cur = String::new();
    let (mut bold, mut italic) = (false, false);
    let flush = |cur: &mut String, spans: &mut Vec<Span>, bold: bool, italic: bool| {
        if !cur.is_empty() {
            spans.push(Span { text: std::mem::take(cur), bold, italic, ..Default::default() });
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&x| x == '`') {
                flush(&mut cur, &mut spans, bold, italic);
                let code: String = chars[i + 1..i + 1 + end].iter().collect();
                spans.push(Span { text: code, code: true, ..Default::default() });
                i += end + 2;
                continue;
            }
        }
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            flush(&mut cur, &mut spans, bold, italic);
            bold = !bold;
            i += 2;
            continue;
        }
        if c == '*' && (italic || chars.get(i + 1).is_some_and(|n| !n.is_whitespace())) {
            flush(&mut cur, &mut spans, bold, italic);
            italic = !italic;
            i += 1;
            continue;
        }
        if c == '[' {
            if let Some(close) = chars[i + 1..].iter().position(|&x| x == ']') {
                let after = i + 1 + close + 1;
                if chars.get(after) == Some(&'(') {
                    if let Some(paren) = chars[after + 1..].iter().position(|&x| x == ')') {
                        let label: String = chars[i + 1..i + 1 + close].iter().collect();
                        let url: String = chars[after + 1..after + 1 + paren].iter().collect();
                        flush(&mut cur, &mut spans, bold, italic);
                        match safe_href(&url) {
                            Some(href) => spans.push(Span { text: label, bold, italic, link: Some(href), ..Default::default() }),
                            None => spans.push(Span { text: label, bold, italic, ..Default::default() }),
                        }
                        i = after + 1 + paren + 1;
                        continue;
                    }
                }
            }
        }
        cur.push(c);
        i += 1;
    }
    flush(&mut cur, &mut spans, bold, italic);
    spans
}

fn list_marker(line: &str) -> Option<(bool, u32, &str)> {
    let t = line.trim_start();
    for m in ["- ", "* ", "+ ", "• "] {
        if let Some(rest) = t.strip_prefix(m) {
            return Some((false, 1, rest));
        }
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() && digits.len() <= 4 {
        let rest = &t[digits.len()..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((true, digits.parse().unwrap_or(1), rest));
        }
    }
    None
}

fn table_cells(line: &str) -> Vec<String> {
    let t = line.trim().trim_start_matches('|').trim_end_matches('|');
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_table_sep(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

pub fn parse(src: &str) -> Vec<Block> {
    let lines: Vec<&str> = src.split('\n').collect();
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush_para = |para: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            let text = para.join("\n");
            para.clear();
            if !text.trim().is_empty() {
                blocks.push(Block::Para(inline(text.trim())));
            }
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end_matches('\r');
        let trimmed = line.trim_start();
        if let Some(fence) = trimmed.strip_prefix("```") {
            flush_para(&mut para, &mut blocks);
            let lang = fence.trim();
            let lang = if lang.is_empty() || lang.len() > 24 { "text".to_string() } else { lang.to_string() };
            let mut body = Vec::new();
            let mut closed = false;
            i += 1;
            while i < lines.len() {
                let l = lines[i].trim_end_matches('\r');
                if l.trim_start().starts_with("```") {
                    closed = true;
                    break;
                }
                body.push(l);
                i += 1;
            }
            blocks.push(Block::Code { lang, body: body.join("\n"), closed });
            i += 1;
            continue;
        }
        if trimmed.is_empty() {
            flush_para(&mut para, &mut blocks);
            i += 1;
            continue;
        }
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=4).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush_para(&mut para, &mut blocks);
            blocks.push(Block::Heading(hashes as u8, inline(trimmed[hashes..].trim())));
            i += 1;
            continue;
        }
        if matches!(trimmed.trim_end(), "---" | "***" | "___") {
            flush_para(&mut para, &mut blocks);
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if trimmed.starts_with('|') && lines.get(i + 1).is_some_and(|n| is_table_sep(n)) {
            flush_para(&mut para, &mut blocks);
            let head = table_cells(trimmed).iter().map(|c| inline(c)).collect();
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                rows.push(table_cells(lines[i]).iter().map(|c| inline(c)).collect());
                i += 1;
            }
            blocks.push(Block::Table { head, rows });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            flush_para(&mut para, &mut blocks);
            let mut quote = vec![rest.trim_start().to_string()];
            i += 1;
            while i < lines.len() {
                match lines[i].trim_start().strip_prefix('>') {
                    Some(r) => quote.push(r.trim_start().to_string()),
                    None => break,
                }
                i += 1;
            }
            blocks.push(Block::Quote(inline(&quote.join("\n"))));
            continue;
        }
        if let Some((ordered, start, first)) = list_marker(line) {
            flush_para(&mut para, &mut blocks);
            let mut items = vec![first.to_string()];
            i += 1;
            while i < lines.len() {
                let l = lines[i].trim_end_matches('\r');
                match list_marker(l) {
                    Some((o, _, rest)) if o == ordered => items.push(rest.to_string()),
                    _ if !l.trim().is_empty() && l.starts_with("  ") => {
                        if let Some(last) = items.last_mut() {
                            last.push(' ');
                            last.push_str(l.trim());
                        }
                    }
                    _ => break,
                }
                i += 1;
            }
            blocks.push(Block::List { ordered, start, items: items.iter().map(|s| inline(s)).collect() });
            continue;
        }
        para.push(line);
        i += 1;
    }
    flush_para(&mut para, &mut blocks);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn blocks_parse() {
        let md = "# Title\n\nHello **bold** and `code`.\n\n- one\n- two\n\n1. a\n2. b\n\n> quote\n\n```rust\nfn x() {}\n```\n---\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let b = parse(md);
        assert!(matches!(&b[0], Block::Heading(1, s) if text(s) == "Title"));
        let Block::Para(p) = &b[1] else { panic!("{b:?}") };
        assert!(p.iter().any(|s| s.bold && s.text == "bold"));
        assert!(p.iter().any(|s| s.code && s.text == "code"));
        assert!(matches!(&b[2], Block::List { ordered: false, items, .. } if items.len() == 2));
        assert!(matches!(&b[3], Block::List { ordered: true, items, .. } if items.len() == 2));
        assert!(matches!(&b[4], Block::Quote(_)));
        assert!(matches!(&b[5], Block::Code { lang, closed: true, .. } if lang == "rust"));
        assert!(matches!(&b[6], Block::Rule));
        assert!(matches!(&b[7], Block::Table { rows, .. } if rows.len() == 1));
    }

    #[test]
    fn unterminated_fence_mid_stream_is_code() {
        let b = parse("Here:\n```html\n<div>");
        assert!(matches!(&b[1], Block::Code { closed: false, body, .. } if body == "<div>"));
    }

    #[test]
    fn raw_html_is_just_text_and_links_are_filtered() {
        let b = parse("<img src=x onerror=alert(1)> [ok](https://example.com) [bad](javascript:alert(1))");
        let Block::Para(spans) = &b[0] else { panic!() };
        assert!(spans[0].text.contains("<img src=x onerror=alert(1)>"));
        assert_eq!(spans.iter().filter(|s| s.link.is_some()).count(), 1);
        assert_eq!(spans.iter().find(|s| s.text == "bad").unwrap().link, None);
        assert_eq!(safe_href("https://a.b/c"), Some("https://a.b/c".into()));
        assert_eq!(safe_href("data:text/html,x"), None);
        assert_eq!(safe_href("https://a.b/\"onmouseover"), None);
    }

    #[test]
    fn snake_case_and_lone_stars_survive() {
        let spans = inline("use my_var_name and 2 * 3");
        assert_eq!(text(&spans), "use my_var_name and 2 * 3");
    }
}
