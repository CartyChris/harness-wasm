//! Thread transcript. Assistant text renders ONLY as escaped text nodes and
//! created elements (DESIGN.md §6): no `innerHTML`, ever. Fenced code becomes
//! a code card; tapping it opens the artifact sheet.

use crate::agent;
use crate::state::{current_session, find_message, AppCtx, Artifact};
use harness_core::Message;
use leptos::prelude::*;

#[component]
pub fn Thread(ctx: AppCtx) -> impl IntoView {
    let each_ctx = ctx.clone();
    let empty_ctx = ctx.clone();

    view! {
        <main class="thread" id="thread">
            <For
                each=move || current_session(&each_ctx).messages
                key=|message| message.id.clone()
                let(message)
            >
                <MessageView ctx=ctx.clone() message_id=message.id.clone() />
            </For>
            <Show when=move || current_session(&empty_ctx).messages.is_empty()>
                <div class="empty-state">
                    <div class="empty-mark" aria-hidden="true">"◇"</div>
                    <p class="empty-title">"Harness"</p>
                    <p class="empty-hint">"Streaming chat through your proxy. Locked model, session memory, artifacts."</p>
                </div>
            </Show>
        </main>
    }
}

#[component]
fn MessageView(ctx: AppCtx, message_id: String) -> impl IntoView {
    let kind = {
        let ctx = ctx.clone();
        let message_id = message_id.clone();
        move || {
            find_message(&ctx, &ctx.current_id.get(), &message_id)
                .map(|m| if m.is_user() { "user" } else { "assistant" })
                .unwrap_or("assistant")
                .to_string()
        }
    };
    let body = {
        let ctx = ctx.clone();
        let message_id = message_id.clone();
        move || {
            find_message(&ctx, &ctx.current_id.get(), &message_id)
                .map(|m| render_body(ctx.clone(), &m))
                .unwrap_or_default()
        }
    };
    let meta = {
        let ctx = ctx.clone();
        let message_id = message_id.clone();
        move || {
            find_message(&ctx, &ctx.current_id.get(), &message_id)
                .map(|m| render_meta(ctx.clone(), &m))
                .unwrap_or_default()
        }
    };

    view! {
        <article class=move || format!("msg msg-{}", kind())>
            <div class="bubble">
                <div class="msg-body">{body}</div>
                {meta}
            </div>
        </article>
    }
}

/// Status line under a message: streaming pulse, failure reason + Retry.
fn render_meta(ctx: AppCtx, message: &Message) -> Vec<AnyView> {
    let mut out = Vec::new();
    match message.status.as_str() {
        "Streaming" => out.push(
            view! {
                <div class="msg-status"><span class="pulse" aria-hidden="true"></span>"streaming…"</div>
            }
            .into_any(),
        ),
        "Interrupted" | "Cancelled" => {
            let reason = message.error.clone().unwrap_or_else(|| {
                "Turn interrupted — partial text kept.".to_string()
            });
            out.push(view! { <div class="msg-status msg-status-warn">{reason}</div> }.into_any());
            out.push(retry_button(ctx));
        }
        "Failed" => {
            let reason = message
                .error
                .clone()
                .unwrap_or_else(|| "The turn failed.".to_string());
            out.push(view! { <div class="msg-status msg-status-err">{reason}</div> }.into_any());
            out.push(retry_button(ctx));
        }
        _ => {}
    }
    out
}

fn retry_button(ctx: AppCtx) -> AnyView {
    view! {
        <button class="mini-btn retry-btn" on:click=move |_| agent::retry(&ctx)>"Retry"</button>
    }
    .into_any()
}

/// A code segment as parsed from fenced blocks.
pub enum Segment {
    Text(String),
    Code { lang: String, body: String },
}

/// Split assistant text on ``` fences. An unterminated fence (mid-stream)
/// still renders as code.
pub fn split_fences(content: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    for (index, part) in content.split("```").enumerate() {
        if index % 2 == 0 {
            if !part.trim().is_empty() {
                segments.push(Segment::Text(part.to_string()));
            }
        } else {
            let (lang, body) = match part.split_once('\n') {
                Some((first, rest)) => (first.trim().to_string(), rest.to_string()),
                None => (part.trim().to_string(), String::new()),
            };
            let lang = if lang.is_empty() || lang.len() > 24 {
                "text".to_string()
            } else {
                lang
            };
            segments.push(Segment::Code { lang, body });
        }
    }
    segments
}

/// Render message content: paragraphs as escaped text, code as cards. All
/// text lands in text nodes — raw model HTML can never execute here.
fn render_body(ctx: AppCtx, message: &Message) -> Vec<AnyView> {
    let mut out = Vec::new();
    for segment in split_fences(&message.content) {
        match segment {
            Segment::Text(text) => {
                out.push(view! { <p class="msg-text">{text}</p> }.into_any());
            }
            Segment::Code { lang, body } => {
                let is_html = lang.eq_ignore_ascii_case("html");
                let preview: String = body.chars().take(600).collect();
                let artifact = if is_html {
                    Artifact::Html {
                        title: "HTML preview".to_string(),
                        body: body.clone(),
                    }
                } else {
                    Artifact::Code {
                        lang: lang.clone(),
                        body: body.clone(),
                    }
                };
                let open_label = if is_html { "Preview" } else { "Open" };
                out.push(
                    view! {
                        <div class="codecard">
                            <div class="codecard-head">
                                <span class="codecard-lang">{lang}</span>
                                <button
                                    class="mini-btn"
                                    on:click=move |_| ctx.artifact.set(Some(artifact.clone()))
                                >
                                    {open_label}
                                </button>
                            </div>
                            <pre class="codecard-pre"><code>{preview}</code></pre>
                        </div>
                    }
                    .into_any(),
                );
            }
        }
    }
    if out.is_empty() {
        out.push(view! { <p class="msg-text msg-placeholder">"…"</p> }.into_any());
    }
    out
}
