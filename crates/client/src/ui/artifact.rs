//! Artifact sheet: fenced code full-screen text, HTML in a sandboxed iframe
//! (`sandbox="allow-scripts"`, NO `allow-same-origin` — model HTML can never
//! touch parent storage or the access token). Unmounts on close.

use crate::state::{AppCtx, Artifact};
use leptos::prelude::*;

#[component]
pub fn ArtifactSheet(ctx: AppCtx, artifact: Artifact) -> impl IntoView {
    let show_source = RwSignal::new(false);
    let copied = RwSignal::new(None::<String>);
    let title = match &artifact {
        Artifact::Code { lang, .. } => format!("{lang} artifact"),
        Artifact::Html { title, .. } => title.clone(),
    };
    let body = match &artifact {
        Artifact::Code { body, .. } => body.clone(),
        Artifact::Html { body, .. } => body.clone(),
    };
    let is_html = matches!(artifact, Artifact::Html { .. });

    let close = {
        let ctx = ctx.clone();
        move |_| ctx.artifact.set(None)
    };

    let copy_body = body.clone();
    let copy = move |_: _| {
        let copied = copied.clone();
        let copy_body = copy_body.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let clipboard = crate::js::window().navigator().clipboard();
            let promise = clipboard.write_text(&copy_body);
            let succeeded = wasm_bindgen_futures::JsFuture::from(promise)
                .await
                .is_ok();
            copied.set(Some(if succeeded {
                "Copied.".to_string()
            } else {
                "Copy failed — select the text manually.".to_string()
            }));
        });
    };

    let toggle_source = show_source;
    let source_label = move || if toggle_source.get() { "Render" } else { "Source" };

    let body_view = {
        let artifact = artifact.clone();
        let show_source = show_source.clone();
        move || {
            match artifact.clone() {
                Artifact::Code { body, .. } => view! {
                    <pre class="artifact-pre"><code>{body}</code></pre>
                }
                .into_any(),
                Artifact::Html { body, .. } => {
                    if show_source.get() {
                        view! {
                            <pre class="artifact-pre"><code>{body}</code></pre>
                        }
                        .into_any()
                    } else {
                        view! {
                            <iframe
                                class="artifact-frame"
                                sandbox="allow-scripts"
                                title="HTML preview"
                                srcdoc=body
                            ></iframe>
                        }
                        .into_any()
                    }
                }
            }
        }
    };

    let close_button = close.clone();
    view! {
        <div class="overlay" role="dialog" aria-modal="true" aria-label=title.clone()>
            <button class="overlay-backdrop" aria-label="Close artifact" on:click=close></button>
            <section class="sheet artifact-sheet">
                <header class="sheet-head">
                    <h2>{title.clone()}</h2>
                    <div class="sheet-head-actions">
                        <Show when=move || is_html>
                            <button class="mini-btn" on:click=move |_| toggle_source.update(|v| *v = !*v)>
                                {source_label}
                            </button>
                        </Show>
                        <button class="icon-btn" aria-label="Close" on:click=close_button>
                            <span class="icon-close" aria-hidden="true"></span>
                        </button>
                    </div>
                </header>
                <div class="artifact-body">{body_view}</div>
                <footer class="sheet-foot">
                    <button class="secondary-btn" on:click=copy>"Copy"</button>
                    <Show when=move || copied.get().is_some()>
                        <span class="health-note" role="status">{move || copied.get().unwrap_or_default()}</span>
                    </Show>
                </footer>
            </section>
        </div>
    }
}
