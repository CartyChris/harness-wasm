//! Markdown AST → DOM. Every string lands in a text node; links only when
//! `safe_href` accepted them. Fenced code becomes a code card that opens the
//! artifact sheet (HTML previews only ever run in the sandboxed iframe).

use crate::state::{AppCtx, Artifact};
use harness_core::markdown::{parse, Block, Span};
use leptos::prelude::*;

pub fn render_markdown(ctx: AppCtx, text: &str) -> AnyView {
    let blocks: Vec<AnyView> = parse(text).into_iter().map(|b| block_view(ctx, b)).collect();
    view! { <div class="md">{blocks}</div> }.into_any()
}

fn spans_view(spans: Vec<Span>) -> Vec<AnyView> {
    spans
        .into_iter()
        .map(|s| {
            if s.code {
                return view! { <code class="ic">{s.text}</code> }.into_any();
            }
            let mut class = String::new();
            if s.bold {
                class.push_str("b ");
            }
            if s.italic {
                class.push_str("i");
            }
            match s.link {
                Some(href) => view! {
                    <a class=class href=href target="_blank" rel="noopener noreferrer">{s.text}</a>
                }
                .into_any(),
                None if class.is_empty() => s.text.into_any(),
                None => view! { <span class=class>{s.text}</span> }.into_any(),
            }
        })
        .collect()
}

fn block_view(ctx: AppCtx, block: Block) -> AnyView {
    match block {
        Block::Para(spans) => view! { <p>{spans_view(spans)}</p> }.into_any(),
        Block::Heading(level, spans) => match level {
            1 => view! { <h3 class="md-h1">{spans_view(spans)}</h3> }.into_any(),
            2 => view! { <h4 class="md-h2">{spans_view(spans)}</h4> }.into_any(),
            _ => view! { <h5 class="md-h3">{spans_view(spans)}</h5> }.into_any(),
        },
        Block::List { ordered, start, items } => {
            let lis: Vec<AnyView> = items
                .into_iter()
                .map(|spans| view! { <li>{spans_view(spans)}</li> }.into_any())
                .collect();
            if ordered {
                view! { <ol start=start.to_string()>{lis}</ol> }.into_any()
            } else {
                view! { <ul>{lis}</ul> }.into_any()
            }
        }
        Block::Quote(spans) => view! { <blockquote>{spans_view(spans)}</blockquote> }.into_any(),
        Block::Rule => view! { <hr /> }.into_any(),
        Block::Table { head, rows } => {
            let ths: Vec<AnyView> = head.into_iter().map(|c| view! { <th>{spans_view(c)}</th> }.into_any()).collect();
            let trs: Vec<AnyView> = rows
                .into_iter()
                .map(|row| {
                    let tds: Vec<AnyView> = row.into_iter().map(|c| view! { <td>{spans_view(c)}</td> }.into_any()).collect();
                    view! { <tr>{tds}</tr> }.into_any()
                })
                .collect();
            view! {
                <div class="md-table-wrap">
                    <table><thead><tr>{ths}</tr></thead><tbody>{trs}</tbody></table>
                </div>
            }
            .into_any()
        }
        Block::Code { lang, body, closed } => code_card(ctx, lang, body, closed),
    }
}

pub fn code_card(ctx: AppCtx, lang: String, body: String, closed: bool) -> AnyView {
    let is_html = lang.eq_ignore_ascii_case("html");
    let preview: String = body.chars().take(1200).collect();
    let line_count = body.lines().count();
    let artifact = if is_html {
        Artifact::Html { title: "HTML preview".to_string(), body: body.clone() }
    } else {
        Artifact::Code { lang: lang.clone(), body: body.clone() }
    };
    let open_label = if is_html { "Preview" } else { "Open" };
    let meta = if closed { format!("{line_count} lines") } else { "writing…".to_string() };
    view! {
        <div class="codecard">
            <div class="codecard-head">
                <span class="codecard-lang">{lang}</span>
                <span class="codecard-meta">{meta}</span>
                <button class="mini-btn" on:click=move |_| ctx.artifact.set(Some(artifact.clone()))>
                    {open_label}
                </button>
            </div>
            <pre class="codecard-pre"><code>{preview}</code></pre>
        </div>
    }
    .into_any()
}
