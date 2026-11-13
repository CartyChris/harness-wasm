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
use wasm_bindgen::JsCast;

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
    let show_token = RwSignal::new(false);

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

                    <div class="field">
                        <label class="field-label" for="access-token">"Access token"</label>
                        // Masked by default: it is a shared secret and shows up
                        // in screenshots / over a shoulder otherwise.
                        <div class="pw-row">
                            <input
                                id="access-token"
                                class="field-input"
                                type=move || if show_token.get() { "text" } else { "password" }
                                autocomplete="off"
                                autocapitalize="off"
                                spellcheck="false"
                                placeholder="Bearer token for your proxy"
                                bind:value=token
                            />
                            <button
                                class="pw-toggle"
                                type="button"
                                aria-label=move || if show_token.get() { "Hide access token" } else { "Show access token" }
                                aria-pressed=move || show_token.get().to_string()
                                on:click=move |_| show_token.update(|v| *v = !*v)
                            >
                                {move || if show_token.get() { "Hide" } else { "Show" }}
                            </button>
                        </div>
                        <span class="field-hint">"This is your proxy's shared secret — not an OpenRouter key."</span>
                    </div>

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
                    <ChatData ctx=ctx />
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

/// Export / import of chats (DESIGN.md §4). Export strips the proxy URL and
/// access token and redacts key shapes; import refuses key-shaped files and
/// never writes anything from a refused file.
#[component]
fn ChatData(ctx: AppCtx) -> impl IntoView {
    use harness_core::export::{build_export, merge_import, parse_import};
    let status = RwSignal::new(None::<(bool, String)>);
    let file_ref = NodeRef::<leptos::html::Input>::new();

    let export = move |_| {
        let settings = ctx.settings.get_untracked();
        let sessions: Vec<_> = ctx.sessions.get_untracked().into_iter().filter(|s| !s.events.is_empty()).collect();
        let raw = build_export(&sessions, storage::now_ms(), &[settings.token.as_str(), settings.base_url.as_str()]);
        match download_json("harness-chats.json", &raw) {
            Ok(()) => status.set(Some((true, format!(
                "Exported {} chat{} · proxy URL and access token stripped.",
                sessions.len(),
                if sessions.len() == 1 { "" } else { "s" }
            )))),
            Err(e) => status.set(Some((false, format!("Export failed: {e}")))),
        }
    };

    let on_file = move |_| {
        let Some(input) = file_ref.get_untracked() else { return };
        let input: web_sys::HtmlInputElement = input.unchecked_into();
        let Some(file) = input.files().and_then(|f| f.get(0)) else { return };
        input.set_value("");
        wasm_bindgen_futures::spawn_local(async move {
            let text = wasm_bindgen_futures::JsFuture::from(file.text()).await.ok().and_then(|v| v.as_string());
            let Some(text) = text else {
                status.set(Some((false, "Import failed: the file could not be read.".into())));
                return;
            };
            match parse_import(&text) {
                Err(msg) => status.set(Some((false, msg))),
                Ok(incoming) => {
                    let mut added = 0;
                    let mut skipped = 0;
                    let incoming_n = incoming.len();
                    ctx.sessions.update(|all| {
                        // Drop untouched placeholder chats so imports land on top.
                        all.retain(|s| incoming_n == 0 || !s.events.is_empty());
                        let (a, k) = merge_import(all, incoming);
                        added = a;
                        skipped = k;
                    });
                    if let Some(first) = ctx.sessions.get_untracked().first() {
                        ctx.current_id.set(first.id.clone());
                    }
                    storage::save_sessions(&ctx);
                    status.set(Some((true, format!("Imported {added} chat{} ({skipped} already here).", if added == 1 { "" } else { "s" }))));
                }
            }
        });
    };

    view! {
        <div class="sec-title">"Chats"</div>
        <div class="data-row">
            <button class="secondary-btn export-btn" on:click=export>"Export chats"</button>
            <label class="secondary-btn import-btn">
                "Import chats"
                <input node_ref=file_ref class="visually-hidden" type="file" accept=".json,application/json" on:change=on_file />
            </label>
        </div>
        <span class="field-hint">{move || if storage::idb::handle().is_some() {
            "Chats are stored in IndexedDB on this device. Exports never include your proxy URL or access token."
        } else {
            "Chats are stored in localStorage on this device (IndexedDB unavailable). Exports never include your proxy URL or access token."
        }}</span>
        <Show when=move || status.get().is_some()>
            <div
                class="health-note data-status"
                class:is-err=move || status.get().is_some_and(|s| !s.0)
                role=move || if status.get().is_some_and(|s| !s.0) { "alert" } else { "status" }
            >
                {move || status.get().map(|s| s.1).unwrap_or_default()}
            </div>
        </Show>
    }
}

/// Download text as a file via a Blob URL (works in iOS Safari 13+).
fn download_json(name: &str, raw: &str) -> Result<(), String> {
    let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(raw));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/json");
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts).map_err(|_| "could not build the file".to_string())?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| "could not build the file".to_string())?;
    let doc = crate::js::document();
    let a: web_sys::HtmlAnchorElement = doc.create_element("a").map_err(|_| "no document".to_string())?.unchecked_into();
    a.set_href(&url);
    a.set_download(name);
    let _ = a.set_attribute("style", "display:none");
    if let Some(body) = doc.body() {
        let _ = body.append_child(&a);
    }
    a.click();
    a.remove();
    // Revoke after the download has started.
    let cb = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let _ = crate::js::window().set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), 4000);
    Ok(())
}
