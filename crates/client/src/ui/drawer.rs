//! Session drawer: new, switch, rename, delete. Unmounts fully on close.

use crate::state::AppCtx;
use crate::storage;
use harness_core::Session;
use leptos::prelude::*;

#[component]
pub fn SessionDrawer(ctx: AppCtx) -> impl IntoView {
    let editing_id = RwSignal::new(None::<String>);
    let draft_title = RwSignal::new(String::new());
    let confirming_id = RwSignal::new(None::<String>);

    let close = {
        let ctx = ctx.clone();
        move |_| ctx.drawer_open.set(false)
    };

    let new_session = {
        let ctx = ctx.clone();
        move |_| new_session(&ctx)
    };

    let list_ctx = ctx.clone();

    view! {
        <div class="overlay" role="dialog" aria-modal="true" aria-label="Sessions">
            <button class="overlay-backdrop" aria-label="Close sessions" on:click=close></button>
            <aside class="drawer">
                <header class="drawer-head">
                    <h2>"Sessions"</h2>
                    <button class="icon-btn" aria-label="Close" on:click=close>
                        <span class="icon-close" aria-hidden="true"></span>
                    </button>
                </header>
                <button class="primary-btn" on:click=new_session>"New session"</button>
                <div class="drawer-list">
                    <For
                        each=move || list_ctx.sessions.get()
                        key=|session| session.id.clone()
                        let(session)
                    >
                        <SessionRow
                            ctx=ctx.clone()
                            session=session
                            editing_id=editing_id
                            confirming_id=confirming_id
                            draft_title=draft_title
                        />
                    </For>
                </div>
            </aside>
        </div>
    }
}

/// New empty session, made current. Reuses an existing empty one at the top
/// instead of stacking blank chats.
pub fn new_session(ctx: &AppCtx) {
    let reuse = ctx.sessions.with_untracked(|s| s.first().filter(|s| s.events.is_empty()).map(|s| s.id.clone()));
    let id = match reuse {
        Some(id) => id,
        None => {
            let session = Session::new(storage::uid("s"), storage::NEW_CHAT_TITLE.to_string(), storage::now_ms());
            let id = session.id.clone();
            ctx.sessions.update(|sessions| sessions.insert(0, session));
            id
        }
    };
    ctx.current_id.set(id);
    ctx.pinned.set(true);
    storage::save_sessions(ctx);
    ctx.drawer_open.set(false);
}

#[component]
fn SessionRow(
    ctx: AppCtx,
    session: Session,
    editing_id: RwSignal<Option<String>>,
    confirming_id: RwSignal<Option<String>>,
    draft_title: RwSignal<String>,
) -> impl IntoView {
    let is_current = {
        let ctx = ctx.clone();
        let session_id = session.id.clone();
        move || ctx.current_id.get() == session_id
    };
    let is_confirming = {
        let confirming_id = confirming_id.clone();
        let session_id = session.id.clone();
        move || confirming_id.get().as_deref() == Some(session_id.as_str())
    };

    let open_session = {
        let ctx = ctx.clone();
        let session_id = session.id.clone();
        move |_| {
            ctx.current_id.set(session_id.clone());
            ctx.drawer_open.set(false);
        }
    };
    let start_rename = {
        let session = session.clone();
        let editing_id = editing_id.clone();
        let confirming_id = confirming_id.clone();
        let draft_title = draft_title.clone();
        move |_| {
            if editing_id.get_untracked().as_deref() != Some(session.id.as_str()) {
                draft_title.set(session.title.clone());
                confirming_id.set(None);
                editing_id.set(Some(session.id.clone()));
            }
        }
    };
    let ask_delete = {
        let ctx = ctx.clone();
        let session_id = session.id.clone();
        let confirming_id = confirming_id.clone();
        let editing_id = editing_id.clone();
        move |_| {
            if confirming_id.get_untracked().as_deref() == Some(session_id.as_str()) {
                delete_session(&ctx, &session_id);
                confirming_id.set(None);
                editing_id.set(None);
            } else {
                confirming_id.set(Some(session_id.clone()));
            }
        }
    };

    // One reactive slot: the open button, or the inline rename input. Fresh
    // closures per evaluation (signals clone cheaply).
    let title = session.title.clone();
    let message_count = session
        .events
        .iter()
        .filter(|e| matches!(e.kind, harness_core::events::EventKind::UserMessage { .. }))
        .count();
    let event_count = session.events.len();
    let edit_slot = {
        let ctx = ctx.clone();
        let session_id = session.id.clone();
        let title = title.clone();
        let editing_id = editing_id.clone();
        let draft_title = draft_title.clone();
        move || {
            let editing = editing_id.get().as_deref() == Some(session_id.as_str());
            if editing {
                let ctx_commit = ctx.clone();
                let session_commit = session_id.clone();
                let editing_commit = editing_id.clone();
                let draft_commit = draft_title.clone();
                let draft_input = draft_title.clone();
                let ctx_key = ctx.clone();
                let session_key = session_id.clone();
                let editing_key = editing_id.clone();
                let draft_key = draft_title.clone();
                view! {
                    <input
                        class="session-rename"
                        type="text"
                        maxlength="80"
                        prop:value=move || draft_title.get()
                        on:input=move |ev| draft_input.set(event_target_value(&ev))
                        on:keydown=move |event: leptos::ev::KeyboardEvent| {
                            if event.key() == "Enter" {
                                event.prevent_default();
                                commit_rename(
                                    &ctx_key,
                                    &session_key,
                                    draft_key.get_untracked(),
                                    editing_key,
                                );
                            }
                        }
                        on:blur=move |_| {
                            commit_rename(
                                &ctx_commit,
                                &session_commit,
                                draft_commit.get_untracked(),
                                editing_commit,
                            );
                        }
                    />
                }
                .into_any()
            } else {
                let open = open_session.clone();
                view! {
                    <button class="session-open" on:click=open>
                        <span class="session-title">{title.clone()}</span>
                        <span class="session-meta">{message_count} " messages · " {event_count} " events"</span>
                    </button>
                }
                .into_any()
            }
        }
    };

    let delete_label = {
        let is_confirming = is_confirming.clone();
        move || if is_confirming() { "Delete?" } else { "Delete" }
    };

    view! {
        <div class="session-row" class:is-current=is_current>
            {edit_slot}
            <div class="session-actions">
                <button class="mini-btn" aria-label="Rename session" on:click=start_rename>
                    <span class="icon-rename" aria-hidden="true"></span>
                </button>
                <button class="mini-btn danger" class:is-confirming=is_confirming aria-label="Delete session" on:click=ask_delete>
                    {delete_label}
                </button>
            </div>
        </div>
    }
}

fn commit_rename(
    ctx: &AppCtx,
    session_id: &str,
    title: String,
    editing_id: RwSignal<Option<String>>,
) {
    let title = title.trim().to_string();
    if !title.is_empty() {
        crate::state::with_session(ctx, session_id, |session| {
            session.title = title;
        });
        storage::save_sessions(ctx);
    }
    editing_id.set(None);
}

fn delete_session(ctx: &AppCtx, session_id: &str) {
    let was_current = ctx.current_id.get_untracked() == session_id;
    ctx.sessions
        .update(|sessions| sessions.retain(|s| s.id != session_id));
    if was_current {
        let next = ctx.sessions.get_untracked().first().map(|s| s.id.clone());
        match next {
            Some(id) => ctx.current_id.set(id),
            None => {
                let session = Session::new(storage::uid("s"), storage::NEW_CHAT_TITLE.to_string(), storage::now_ms());
                let id = session.id.clone();
                ctx.sessions.update(|sessions| sessions.push(session));
                ctx.current_id.set(id);
            }
        }
    }
    storage::save_sessions(ctx);
}
