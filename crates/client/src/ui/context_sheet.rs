//! Context sheet (Aider pattern): exactly what the next request will carry —
//! token estimate vs budget, the deterministic repo map, kept/trimmed turns,
//! the workspace files, and the system prompt. Unmounts on close.

use crate::state::{current_session, AppCtx, Artifact};
use crate::ui::composer::short_tokens;
use harness_core::context::{assemble, DEFAULT_BUDGET_TOKENS};
use harness_core::workspace::Workspace;
use leptos::prelude::*;

#[component]
pub fn ContextSheet(ctx: AppCtx) -> impl IntoView {
    let session = current_session(&ctx);
    let ws = Workspace::derive(&session.events);
    let plan = assemble(&session.events, &ws, DEFAULT_BUDGET_TOKENS);
    let pct = ((plan.est_tokens as f64 / plan.budget_tokens as f64) * 100.0).clamp(1.0, 100.0);
    let bar_style = format!("width:{pct:.1}%");
    let close = move |_| ctx.context_open.set(false);
    let summary = format!(
        "≈{} of {} tokens · {} turn{} kept{}",
        short_tokens(plan.est_tokens),
        short_tokens(plan.budget_tokens),
        plan.kept_groups,
        if plan.kept_groups == 1 { "" } else { "s" },
        if plan.dropped_groups > 0 { format!(" · {} trimmed", plan.dropped_groups) } else { String::new() }
    );
    let files: Vec<AnyView> = ws
        .files
        .iter()
        .map(|(path, body)| {
            let lines = body.lines().count();
            let is_html = path.ends_with(".html") || path.ends_with(".htm");
            let artifact = if is_html {
                Artifact::Html { title: path.clone(), body: body.clone() }
            } else {
                Artifact::Code { lang: path.rsplit('.').next().unwrap_or("text").to_string(), body: body.clone() }
            };
            view! {
                <div class="ctx-file">
                    <span class="ctx-file-path">{path.clone()}</span>
                    <span class="ctx-file-meta">{format!("{lines} lines")}</span>
                    <button class="mini-btn" on:click=move |_| {
                        ctx.context_open.set(false);
                        ctx.artifact.set(Some(artifact.clone()));
                    }>{if is_html { "Preview" } else { "Open" }}</button>
                </div>
            }
            .into_any()
        })
        .collect();
    let has_files = !files.is_empty();
    let repo_map = if plan.repo_map.is_empty() {
        "(empty — files the agent writes appear here, mapped by name and top-level symbols)".to_string()
    } else {
        plan.repo_map.clone()
    };
    let roles: Vec<AnyView> = plan
        .messages
        .iter()
        .map(|m| {
            let role = m.role.clone();
            let size = m.chars();
            let label = if m.tool_calls.is_empty() { role.clone() } else { format!("{role} · {} call(s)", m.tool_calls.len()) };
            view! { <span class=format!("ctx-msg role-{role}")>{label}<em>{short_tokens(size.div_ceil(4))}</em></span> }.into_any()
        })
        .collect();
    let system = plan.system.clone();

    view! {
        <div class="overlay" role="dialog" aria-modal="true" aria-label="Context">
            <button class="overlay-backdrop" aria-label="Close context" on:click=close></button>
            <section class="sheet ctx-sheet">
                <div class="sheet-grab" aria-hidden="true"></div>
                <header class="sheet-head">
                    <h2>"Context"</h2>
                    <button class="icon-btn" aria-label="Close" on:click=close>
                        <span class="icon-close" aria-hidden="true"></span>
                    </button>
                </header>
                <div class="sheet-body">
                    <div class="ctx-meter">
                        <div class="ctx-summary">{summary}</div>
                        <div class="ctx-bar"><span style=bar_style></span></div>
                        <div class="field-hint">"Assembled deterministically from the event log: no embeddings, no retrieval. Oldest whole turns are trimmed first."</div>
                    </div>
                    <div class="sec">
                        <div class="sec-title">"Repo map"</div>
                        <pre class="ctx-pre">{repo_map}</pre>
                    </div>
                    {has_files.then(|| view! {
                        <div class="sec">
                            <div class="sec-title">"Workspace files"</div>
                            <div class="ctx-files">{files}</div>
                        </div>
                    })}
                    <div class="sec">
                        <div class="sec-title">"Next request"</div>
                        <div class="ctx-msgs">{roles}</div>
                    </div>
                    <details class="sec ctx-details">
                        <summary>"System prompt"</summary>
                        <pre class="ctx-pre">{system}</pre>
                    </details>
                </div>
            </section>
        </div>
    }
}
