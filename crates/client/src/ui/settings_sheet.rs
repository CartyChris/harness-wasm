//! Settings / Connect card: proxy base URL + access token only (never an
//! OpenRouter key field), plus the locked model picker with a best-effort
//! live list. Unmounts fully on close.

use crate::net::{self, NetError};
use crate::state::AppCtx;
use crate::storage;
use harness_core::permissions::{decide, mode_default, ApprovalMode, Permission};
use harness_core::tools::{ToolKind, REGISTRY};
use harness_core::{normalize_model_input, Settings};
use leptos::prelude::*;

#[derive(Clone, PartialEq)]
enum ListState {
    Idle,
    Loading,
    Loaded(Vec<String>),
    Failed(String),
}

#[component]
pub fn SettingsSheet(ctx: AppCtx) -> impl IntoView {
    let ctx = ctx;
    let initial = ctx.settings.get_untracked();
    let base_url = RwSignal::new(initial.base_url.clone());
    let token = RwSignal::new(initial.token.clone());
    let model_input = RwSignal::new(ctx.model.get_untracked());
    let list_state = RwSignal::new(ListState::Idle);
    let health_note = RwSignal::new(None::<String>);

    let close = {
        let ctx = ctx.clone();
        move |_| ctx.settings_open.set(false)
    };

    let save = {
        let ctx = ctx.clone();
        move |_: _| {
            let settings = Settings {
                base_url: base_url.get_untracked().trim().trim_end_matches('/').to_string(),
                token: token.get_untracked().trim().to_string(),
            };
            if let Some(model) = normalize_model_input(&model_input.get_untracked()) {
                ctx.model.set(model.clone());
                storage::save_model(&model, &ctx);
            }
            ctx.settings.set(settings.clone());
            storage::save_settings(&settings, &ctx);
            ctx.settings_open.set(false);
        }
    };

    let test_health = {
        let ctx = ctx.clone();
        let base_url = base_url.clone();
        let token = token.clone();
        let health_note = health_note.clone();
        move |_: _| {
            health_note.set(Some("Checking…".to_string()));
            let settings = Settings {
                base_url: base_url.get_untracked().trim().trim_end_matches('/').to_string(),
                token: token.get_untracked().trim().to_string(),
            };
            let health_note = health_note.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match net::health(&settings).await {
                    Ok(true) => health_note.set(Some("Proxy is up.".to_string())),
                    Ok(false) => health_note.set(Some("Proxy responded but not healthy.".to_string())),
                    Err(err) => health_note.set(Some(describe_net_error(&err))),
                }
                let _ = &ctx;
            });
        }
    };

    let load_list = {
        let base_url = base_url.clone();
        let token = token.clone();
        let list_state = list_state.clone();
        move |_: _| {
            list_state.set(ListState::Loading);
            let settings = Settings {
                base_url: base_url.get_untracked().trim().trim_end_matches('/').to_string(),
                token: token.get_untracked().trim().to_string(),
            };
            let list_state = list_state.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match net::fetch_models(&settings).await {
                    Ok(ids) => list_state.set(ListState::Loaded(ids)),
                    Err(err) => list_state.set(ListState::Failed(describe_net_error(&err))),
                }
            });
        }
    };

    let list_status = move || match list_state.get() {
        ListState::Idle => String::new(),
        ListState::Loading => "Loading model list…".to_string(),
        ListState::Loaded(ref ids) => format!("{} models available. Locked model unchanged.", ids.len()),
        ListState::Failed(ref reason) => format!("List unavailable — {reason}. Keeping locked model."),
    };

    let options = move || match list_state.get() {
        ListState::Loaded(ids) => ids,
        _ => Vec::new(),
    };

    view! {
        <div class="overlay" role="dialog" aria-modal="true" aria-label="Settings">
            <button class="overlay-backdrop" aria-label="Close settings" on:click=close></button>
            <section class="sheet">
                <div class="sheet-grab" aria-hidden="true"></div>
                <header class="sheet-head">
                    <h2>"Settings"</h2>
                    <button class="icon-btn" aria-label="Close" on:click=close>
                        <span class="icon-close" aria-hidden="true"></span>
                    </button>
                </header>

                <div class="sheet-body">
                    <div class="sec-title">"Connect"</div>
                    <label class="field">
                        <span class="field-label">"Proxy URL"</span>
                        <input
                            class="field-input"
                            type="url"
                            inputmode="url"
                            autocomplete="off"
                            spellcheck="false"
                            placeholder="https://your-harness.vercel.app (blank = same host)"
                            bind:value=base_url
                        />
                        <span class="field-hint">"The Harness proxy that holds the OpenRouter key. Never put an OpenRouter key on this phone."</span>
                    </label>

                    <label class="field">
                        <span class="field-label">"Access token"</span>
                        <input
                            class="field-input"
                            type="text"
                            autocomplete="off"
                            spellcheck="false"
                            placeholder="Bearer token for your proxy"
                            bind:value=token
                        />
                        <span class="field-hint">"This is your proxy's shared secret — not an OpenRouter key."</span>
                    </label>

                    <button class="secondary-btn" on:click=test_health>"Test proxy"</button>
                    <Show when=move || health_note.get().is_some()>
                        <div class="health-note" role="status">{move || health_note.get().unwrap_or_default()}</div>
                    </Show>

                    <div class="field">
                        <div class="field-row">
                            <span class="field-label">"Model (locked)"</span>
                            <button class="mini-btn" on:click=load_list>"Load list"</button>
                        </div>
                        <input
                            class="field-input"
                            type="text"
                            autocomplete="off"
                            spellcheck="false"
                            list="model-ids"
                            placeholder="x-ai/grok-4.7"
                            bind:value=model_input
                        />
                        <datalist id="model-ids">
                            {move || options().into_iter().map(|id| view! { <option value=id /> }).collect::<Vec<_>>()}
                        </datalist>
                        <span class="field-hint">{list_status}</span>
                    </div>

                    <AgentSettings ctx=ctx />
                </div>

                <footer class="sheet-foot">
                    <button class="primary-btn" on:click=save>"Save"</button>
                </footer>
            </section>
        </div>
    }
}

/// Approval mode (codex) + per-tool allow / ask / deny (opencode). These
/// apply immediately — they gate the very next tool call.
#[component]
fn AgentSettings(ctx: AppCtx) -> impl IntoView {
    let modes: Vec<AnyView> = ApprovalMode::ALL
        .iter()
        .map(|&m| {
            view! {
                <button
                    class="seg-btn"
                    class:is-active=move || ctx.mode.get() == m
                    aria-pressed=move || (ctx.mode.get() == m).to_string()
                    on:click=move |_| {
                        ctx.mode.set(m);
                        storage::save_mode(m, &ctx);
                    }
                >
                    {m.label()}
                </button>
            }
            .into_any()
        })
        .collect();
    let tool_rows: Vec<AnyView> = REGISTRY
        .iter()
        .map(|spec| {
            let name = spec.name;
            let kind = if spec.kind == ToolKind::Write { "write" } else { "read" };
            let effective = move || {
                let p = decide(spec, ctx.mode.get(), &ctx.rules.get());
                let source = if ctx.rules.with(|r| r.get(name).is_some()) && !(ctx.mode.get() == ApprovalMode::ReadOnly && spec.kind == ToolKind::Write) {
                    "rule"
                } else {
                    "mode"
                };
                format!("→ {} ({source})", p.label())
            };
            let choices: Vec<AnyView> = [None, Some(Permission::Allow), Some(Permission::Ask), Some(Permission::Deny)]
                .into_iter()
                .map(|choice| {
                    let label = match choice {
                        None => "Default",
                        Some(p) => p.label(),
                    };
                    let active = move || ctx.rules.with(|r| r.get(name)) == choice;
                    let class = match choice {
                        Some(Permission::Allow) => "seg-btn sm allow",
                        Some(Permission::Ask) => "seg-btn sm ask",
                        Some(Permission::Deny) => "seg-btn sm deny",
                        None => "seg-btn sm",
                    };
                    view! {
                        <button
                            class=class
                            class:is-active=active
                            aria-pressed=move || active().to_string()
                            aria-label=format!("{name}: {label}")
                            on:click=move |_| {
                                ctx.rules.update(|r| r.set(name, choice));
                                storage::save_rules(&ctx.rules.get_untracked(), &ctx);
                            }
                        >
                            {label}
                        </button>
                    }
                    .into_any()
                })
                .collect();
            let default_hint = move || format!("default: {}", mode_default(spec, ctx.mode.get()).label());
            view! {
                <div class="perm-row" data-tool=name>
                    <div class="perm-head">
                        <code class="perm-name">{name}</code>
                        <span class=format!("perm-kind {kind}")>{kind}</span>
                        <span class="perm-eff">{effective}</span>
                    </div>
                    <div class="seg perm-seg">{choices}</div>
                    <div class="field-hint">{default_hint}</div>
                </div>
            }
            .into_any()
        })
        .collect();
    let toggle_tools = move |_| {
        let on = !ctx.tools_enabled.get_untracked();
        ctx.tools_enabled.set(on);
        storage::save_tools_enabled(on, &ctx);
    };
    view! {
        <div class="sec-title">"Agent"</div>
        <div class="field">
            <span class="field-label">"Approval mode"</span>
            <div class="seg mode-seg">{modes}</div>
            <span class="field-hint">{move || ctx.mode.get().blurb()}</span>
        </div>
        <div class="field">
            <div class="field-row">
                <span class="field-label">"Workspace tools"</span>
                <button class="switch" class:is-on=move || ctx.tools_enabled.get() role="switch"
                    aria-checked=move || ctx.tools_enabled.get().to_string() aria-label="Workspace tools" on:click=toggle_tools>
                    <span class="switch-knob" aria-hidden="true"></span>
                </button>
            </div>
            <span class="field-hint">"Tools act only on a phone-local virtual workspace — no shell, no network. Turn off for models without tool support."</span>
        </div>
        <div class="sec-title">"Tool permissions"</div>
        <div class="perm-list">{tool_rows}</div>
    }
}

pub fn describe_net_error(err: &NetError) -> String {
    match err {
        NetError::Unauthorized => "access token missing or wrong".to_string(),
        NetError::Unreachable(reason) => format!("proxy unreachable ({reason})"),
        NetError::Api { message, .. } => message.clone(),
    }
}
