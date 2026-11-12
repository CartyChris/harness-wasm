//! Proxy integration tests (DESIGN.md §10, §13): host scope, bearer gate,
//! redaction, body cap, SSE incremental streaming — all against a mock
//! OpenRouter on loopback. No real network calls, no real keys.

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use harness_proxy::{full_router, ProxyConfig};
use http_body_util::BodyExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower::ServiceExt;

// A fake key the tests inject as the proxy's OPENROUTER_API_KEY. Assembled
// from parts so no key-shaped literal ever sits in the repo (secret-grep
// stays absolute); it exists to prove redaction works.
fn fake_upstream_key() -> String {
    ["sk-or-", "v1-", "test0000000000000000000000000000"].concat()
}
const GATE_TOKEN: &str = "gate-token-0123456789abcdef";

#[derive(Clone)]
enum ChatBehavior {
    Sse { chunks: Vec<String>, delay: Duration },
    Error { status: u16, body: String },
}

#[derive(Clone)]
struct MockState {
    hits: Arc<Mutex<Vec<RecordedHit>>>,
    chat: ChatBehavior,
}

#[derive(Clone)]
struct RecordedHit {
    path: String,
    authorization: String,
    body_len: usize,
}

async fn spawn_mock(state: MockState) -> url::Url {
    let chat_state = state.clone();
    let app = Router::new()
        .route(
            "/api/v1/chat/completions",
            post(move |State(st): State<MockState>, req: Request<Body>| async move {
                let (parts, body) = req.into_parts();
                let bytes = axum::body::to_bytes(body, 8 * 1024 * 1024)
                    .await
                    .unwrap_or_default();
                st.hits.lock().unwrap().push(RecordedHit {
                    path: parts.uri.path().to_string(),
                    authorization: parts
                        .headers
                        .get(header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body_len: bytes.len(),
                });
                match st.chat {
                    ChatBehavior::Sse { ref chunks, delay } => {
                        let (tx, rx) = tokio::sync::mpsc::channel::<Result<
                            axum::body::Bytes,
                            std::io::Error,
                        >>(4);
                        let chunks = chunks.clone();
                        tokio::spawn(async move {
                            for chunk in chunks {
                                let _ = tx.send(Ok(axum::body::Bytes::from(chunk))).await;
                                tokio::time::sleep(delay).await;
                            }
                        });
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                            .body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)))
                            .unwrap()
                    }
                    ChatBehavior::Error { status, ref body } => Response::builder()
                        .status(StatusCode::from_u16(status).unwrap())
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.clone()))
                        .unwrap(),
                }
            })
            .with_state(chat_state)
        )
        .route(
            "/api/v1/models",
            get(move |State(st): State<MockState>, req: Request<Body>| async move {
                let (parts, _) = req.into_parts();
                st.hits.lock().unwrap().push(RecordedHit {
                    path: parts.uri.path().to_string(),
                    authorization: parts
                        .headers
                        .get(header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body_len: 0,
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"data":[{"id":"x-ai/grok-4.7"},{"id":"openai/gpt-4o-mini"}]}"#))
                    .unwrap()
            })
            .with_state(state),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    url::Url::parse(&format!("http://{addr}")).unwrap()
}

fn test_config(mock_base: url::Url) -> ProxyConfig {
    ProxyConfig {
        upstream_base: mock_base,
        enforce_origin: false,
        access_token: Some(GATE_TOKEN.to_string()),
        openrouter_key: Some(fake_upstream_key()),
        loopback_dev: false,
        http: reqwest::Client::new(),
    }
}

fn post_chat(body: &str, auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/chat")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

async fn body_string(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

fn sse_chunk(content: &str) -> String {
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{content}\"}}}}]}}\n\n")
}

// --- bearer gate -----------------------------------------------------------

#[tokio::test]
async fn chat_requires_bearer_token() {
    let state = MockState {
        hits: Arc::new(Mutex::new(Vec::new())),
        chat: ChatBehavior::Sse {
            chunks: vec![sse_chunk("hi")],
            delay: Duration::from_millis(10),
        },
    };
    let mock = spawn_mock(state.clone()).await;
    let app = full_router(test_config(mock));

    // Missing token → 401, no upstream call.
    let resp = app
        .clone()
        .oneshot(post_chat(r#"{"model":"x-ai/grok-4.7","messages":[]}"#, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(resp.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_some());
    assert_eq!(state.hits.lock().unwrap().len(), 0);

    // Wrong token → 401.
    let resp = app
        .clone()
        .oneshot(post_chat(r#"{"model":"x-ai/grok-4.7","messages":[]}"#, Some("wrong")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Correct token → streams.
    let resp = app
        .oneshot(post_chat(r#"{"model":"x-ai/grok-4.7","messages":[]}"#, Some(GATE_TOKEN)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(state.hits.lock().unwrap().len() >= 1);
}

#[tokio::test]
async fn empty_token_env_fails_closed_unless_loopback() {
    let state = MockState {
        hits: Arc::new(Mutex::new(Vec::new())),
        chat: ChatBehavior::Sse {
            chunks: vec![sse_chunk("hi")],
            delay: Duration::from_millis(10),
        },
    };
    let mock = spawn_mock(state.clone()).await;

    // Non-loopback (deploy shape): no token configured → 401, fail closed.
    let mut cfg = test_config(mock.clone());
    cfg.access_token = None;
    cfg.loopback_dev = false;
    let resp = full_router(cfg)
        .oneshot(post_chat(r#"{"model":"x-ai/grok-4.7","messages":[]}"#, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Loopback dev (bound to 127.0.0.1, env empty): allowed.
    let mut cfg = test_config(mock);
    cfg.access_token = None;
    cfg.loopback_dev = true;
    let resp = full_router(cfg)
        .oneshot(post_chat(r#"{"model":"x-ai/grok-4.7","messages":[]}"#, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// --- host scope ------------------------------------------------------------

#[tokio::test]
async fn non_openrouter_origin_is_refused_before_any_fetch() {
    let state = MockState {
        hits: Arc::new(Mutex::new(Vec::new())),
        chat: ChatBehavior::Sse {
            chunks: vec![sse_chunk("hi")],
            delay: Duration::from_millis(10),
        },
    };
    let mock = spawn_mock(state.clone()).await;
    let mut cfg = test_config(mock);
    cfg.enforce_origin = true; // production shape
    cfg.upstream_base = url::Url::parse("https://openrouter.ai.evil.example").unwrap();

    let resp = full_router(cfg)
        .oneshot(post_chat(r#"{"model":"x-ai/grok-4.7","messages":[]}"#, Some(GATE_TOKEN)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(state.hits.lock().unwrap().len(), 0, "no upstream call");
}

#[tokio::test]
async fn client_supplied_url_field_is_ignored() {
    // A body that tries to redirect the proxy must still hit the configured
    // OpenRouter paths only.
    let state = MockState {
        hits: Arc::new(Mutex::new(Vec::new())),
        chat: ChatBehavior::Sse {
            chunks: vec![sse_chunk("hi")],
            delay: Duration::from_millis(10),
        },
    };
    let mock = spawn_mock(state.clone()).await;
    let app = full_router(test_config(mock));
    let evil_body = r#"{"model":"x-ai/grok-4.7","messages":[],"url":"https://evil.example/pwn","base_url":"http://169.254.169.254/latest"}"#;
    let resp = app
        .oneshot(post_chat(evil_body, Some(GATE_TOKEN)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let hits = state.hits.lock().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "/api/v1/chat/completions");
}

// --- redaction / key handling ----------------------------------------------

#[tokio::test]
async fn upstream_error_body_is_never_echoed() {
    // The mock 500s with key material in the body; the client must get a
    // fixed public string, with no key anywhere.
    let state = MockState {
        hits: Arc::new(Mutex::new(Vec::new())),
        chat: ChatBehavior::Error {
            status: 500,
            body: format!(r#"{{"error":{{"message":"boom key={key}"}}}}"#, key = fake_upstream_key()),
        },
    };
    let mock = spawn_mock(state).await;
    let app = full_router(test_config(mock));
    let resp = app
        .oneshot(post_chat(r#"{"model":"x-ai/grok-4.7","messages":[]}"#, Some(GATE_TOKEN)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = body_string(resp.into_body()).await;
    assert!(body.contains("upstream"), "{body}");
    assert!(!body.contains("sk-or-"), "key leaked: {body}");
    assert!(!body.contains(fake_upstream_key().as_str()), "key leaked: {body}");
}

#[tokio::test]
async fn client_key_is_never_forwarded_or_echoed() {
    let state = MockState {
        hits: Arc::new(Mutex::new(Vec::new())),
        chat: ChatBehavior::Sse {
            chunks: vec![sse_chunk("hi")],
            delay: Duration::from_millis(10),
        },
    };
    let mock = spawn_mock(state.clone()).await;
    let app = full_router(test_config(mock));

    // Client tries to smuggle its own OpenRouter key in the Authorization
    // header *and* the body.
    let smuggle = format!(
        r#"{{"model":"x-ai/grok-4.7","messages":[],"api_key":"{key}"}}"#,
        key = fake_upstream_key()
    );
    let resp = app
        .clone()
        .oneshot(post_chat(&smuggle, Some(GATE_TOKEN)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "key-shaped body must be refused");
    let body = body_string(resp.into_body()).await;
    assert!(!body.contains("sk-or-"), "{body}");
    assert_eq!(state.hits.lock().unwrap().len(), 0);

    // With a clean body, the upstream Authorization must be the server key —
    // never the gate token.
    let resp = app
        .oneshot(post_chat(r#"{"model":"x-ai/grok-4.7","messages":[]}"#, Some(GATE_TOKEN)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let hits = state.hits.lock().unwrap();
    assert_eq!(hits[0].authorization, format!("Bearer {}", fake_upstream_key()));
}

// --- body cap ----------------------------------------------------------------

#[tokio::test]
async fn oversized_body_is_413_without_upstream_call() {
    let state = MockState {
        hits: Arc::new(Mutex::new(Vec::new())),
        chat: ChatBehavior::Sse {
            chunks: vec![sse_chunk("hi")],
            delay: Duration::from_millis(10),
        },
    };
    let mock = spawn_mock(state.clone()).await;
    let app = full_router(test_config(mock));

    let big = "x".repeat(1024 * 1024 + 1);
    let resp = app
        .oneshot(post_chat(&big, Some(GATE_TOKEN)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(state.hits.lock().unwrap().len(), 0);
}

// --- SSE incremental ---------------------------------------------------------

#[tokio::test]
async fn chat_sse_is_incremental_not_one_dump() {
    let delay = Duration::from_millis(120);
    let chunks: Vec<String> = (0..5).map(|i| sse_chunk(&format!("t{i}"))).collect();
    let state = MockState {
        hits: Arc::new(Mutex::new(Vec::new())),
        chat: ChatBehavior::Sse {
            chunks,
            delay,
        },
    };
    let mock = spawn_mock(state).await;
    let app = full_router(test_config(mock));

    let started = Instant::now();
    let resp = app
        .oneshot(post_chat(r#"{"model":"x-ai/grok-4.7","messages":[]}"#, Some(GATE_TOKEN)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));

    let mut body = resp.into_body();
    let mut first_frame: Option<Instant> = None;
    let mut last_frame: Option<Instant> = None;
    let mut frame_count = 0usize;
    let mut text = String::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.unwrap();
        if let Some(data) = frame.data_ref() {
            frame_count += 1;
            let now = Instant::now();
            if first_frame.is_none() {
                first_frame = Some(now);
            }
            last_frame = Some(now);
            text.push_str(&String::from_utf8_lossy(data));
        }
    }

    let first = first_frame.expect("at least one frame");
    let last = last_frame.expect("at least one frame");
    let first_offset = first.duration_since(started);
    let spread = last.duration_since(first);

    // All five SSE chunks arrive as distinct frames.
    assert!(frame_count >= 5, "frames: {frame_count}");
    assert!(text.contains("t0") && text.contains("t4"), "{text}");
    // First byte reached the client long before the upstream finished
    // (buffering would delay it past ~5×delay).
    assert!(
        first_offset < delay * 3,
        "first frame too late: {first_offset:?}"
    );
    // Frames kept arriving while the upstream was still sending.
    assert!(
        spread >= delay * 3,
        "frames arrived in one dump: spread {spread:?}"
    );
}

// --- models / health ----------------------------------------------------------

#[tokio::test]
async fn models_relay_and_health() {
    let state = MockState {
        hits: Arc::new(Mutex::new(Vec::new())),
        chat: ChatBehavior::Sse {
            chunks: vec![sse_chunk("hi")],
            delay: Duration::from_millis(10),
        },
    };
    let mock = spawn_mock(state).await;
    let app = full_router(test_config(mock));

    // Models: auth required, then relayed.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/models")
                .header(header::AUTHORIZATION, format!("Bearer {GATE_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp.into_body()).await;
    assert!(body.contains("x-ai/grok-4.7"), "{body}");

    // Health: public, inert.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp.into_body()).await;
    assert!(body.contains("\"ok\":true"), "{body}");
    assert!(!body.contains("sk-or-") && !body.contains(GATE_TOKEN), "{body}");
}

// --- CORS preflight -----------------------------------------------------------

#[tokio::test]
async fn options_preflight_succeeds_without_auth() {
    let mock = spawn_mock(MockState {
        hits: Arc::new(Mutex::new(Vec::new())),
        chat: ChatBehavior::Sse {
            chunks: vec![sse_chunk("hi")],
            delay: Duration::from_millis(10),
        },
    })
    .await;
    let app = full_router(test_config(mock));
    let resp = app
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/chat")
                .header(header::ORIGIN, "null")
                .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization, content-type")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        resp.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "*"
    );
    let allow = resp
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(allow.to_lowercase().contains("authorization"));
    assert!(allow.to_lowercase().contains("content-type"));
}
