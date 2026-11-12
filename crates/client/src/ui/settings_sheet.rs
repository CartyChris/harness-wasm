//! Settings / Connect card: proxy base URL + access token only (never an
//! OpenRouter key field), plus the locked model picker with a best-effort
//! live list. Unmounts fully on close.

use crate::net::{self, NetError};
use crate::state::AppCtx;
use crate::storage;
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
                <header class="sheet-head">
                    <h2>"Connect"</h2>
                    <button class="icon-btn" aria-label="Close" on:click=close>
                        <span class="icon-close" aria-hidden="true"></span>
                    </button>
                </header>

                <div class="sheet-body">
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
                </div>

                <footer class="sheet-foot">
                    <button class="primary-btn" on:click=save>"Save"</button>
                </footer>
            </section>
        </div>
    }
}

pub fn describe_net_error(err: &NetError) -> String {
    match err {
        NetError::Unauthorized => "access token missing or wrong".to_string(),
        NetError::Unreachable(reason) => format!("proxy unreachable ({reason})"),
        NetError::Api { message, .. } => message.clone(),
    }
}
