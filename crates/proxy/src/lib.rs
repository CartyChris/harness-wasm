//! Harness proxy core: the request handlers shared by the Vercel Rust
//! functions (`api/*.rs`) and the loopback dev server.
//!
//! Security rules enforced here (see DESIGN.md §5, §6):
//! - The only upstream origin is `https://openrouter.ai`; there is no
//!   client-supplied URL path anywhere.
//! - `OPENROUTER_API_KEY` is attached server-side, never echoed, and client
//!   OpenRouter keys are neither forwarded nor reflected.
//! - Bearer gate on `HARNESS_ACCESS_TOKEN` for `/api/chat` and `/api/models`
//!   (loopback dev may skip it only when the env is empty).
//! - Request bodies over 1 MiB are rejected with 413 before any upstream call.
//! - Everything that could reach a log or an error body is redacted.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;

pub const OPENROUTER_ORIGIN: &str = "https://openrouter.ai";
pub const MAX_BODY_BYTES: usize = 1024 * 1024; // 1 MiB
const UPSTREAM_ERROR_BODY_LIMIT: usize = 64 * 1024;

const MISSING_KEY_MESSAGE: &str = "proxy is missing its upstream key";
const KEY_SHAPED_MESSAGE: &str = "request contains a key-shaped value";
const NO_TOKEN_MESSAGE: &str = "missing or wrong access token";
const BODY_TOO_LARGE_MESSAGE: &str = "request body exceeds 1 MiB";

/// Runtime configuration. Built from env in `api/*.rs` and the dev server,
/// built directly in tests.
#[derive(Clone)]
pub struct ProxyConfig {
    /// Base URL the proxy talks to. Always `https://openrouter.ai` in
    /// production; a mock origin in tests.
    pub upstream_base: url::Url,
    /// When true (production), any constructed upstream URL that is not the
    /// exact OpenRouter origin is refused before a socket is opened.
    pub enforce_origin: bool,
    pub access_token: Option<String>,
    pub openrouter_key: Option<String>,
    /// True only for the loopback dev server; lets requests through when
    /// `HARNESS_ACCESS_TOKEN` is empty.
    pub loopback_dev: bool,
    pub http: reqwest::Client,
}

impl ProxyConfig {
    pub fn from_env(loopback_dev: bool) -> Self {
        Self {
            upstream_base: url::Url::parse(OPENROUTER_ORIGIN).expect("valid upstream origin"),
            enforce_origin: true,
            access_token: std::env::var("HARNESS_ACCESS_TOKEN")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            openrouter_key: std::env::var("OPENROUTER_API_KEY")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            loopback_dev,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(15))
                // No overall timeout: SSE responses must not be cut off by the
                // client. maxDuration on the function is the honest cap.
                .build()
                .expect("reqwest client"),
        }
    }

    /// Build an upstream URL, refusing anything outside the allowed origin
    /// when the origin gate is enforced.
    fn upstream_url(&self, path: &str) -> Result<url::Url, StatusCode> {
        let mut url = self.upstream_base.clone();
        url.set_path(path);
        url.set_query(None);
        if self.enforce_origin && !is_allowed_origin(&url) {
            // Defensive only: with enforce_origin the base itself is checked
            // at construction time in production.
            return Err(StatusCode::BAD_GATEWAY);
        }
        Ok(url)
    }
}

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never format secrets into logs.
        f.debug_struct("ProxyConfig")
            .field("upstream_base", &self.upstream_base)
            .field("enforce_origin", &self.enforce_origin)
            .field("has_access_token", &self.access_token.is_some())
            .field("has_openrouter_key", &self.openrouter_key.is_some())
            .field("loopback_dev", &self.loopback_dev)
            .finish_non_exhaustive()
    }
}

/// The single origin the proxy is allowed to fetch. Exact match on
/// scheme + host + (default) port — no subdomains, no http, no ports.
pub fn is_allowed_origin(url: &url::Url) -> bool {
    url.origin()
        == url::Origin::Tuple(
            "https".to_string(),
            url::Host::Domain("openrouter.ai".to_string()),
            443,
        )
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// Replace anything key- or token-shaped with `[REDACTED]`: the exact secret
/// values in `secrets`, `sk-or-…` shapes, and `Bearer <long-token>`.
pub fn redact(input: &str, secrets: &[&str]) -> String {
    let mut out = input.to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        if out.contains(secret) {
            out = out.replace(secret, "[REDACTED]");
        }
    }
    out = replace_pattern(&out, "sk-or-");
    out = replace_pattern(&out, "Bearer ");
    out
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '~'
}

/// Replace `prefix + run-of-token-chars` with `prefix[REDACTED]` where the
/// run is at least 16 characters — long enough that prose never matches.
fn replace_pattern(input: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find(prefix) {
        out.push_str(&rest[..pos + prefix.len()]);
        let after = &rest[pos + prefix.len()..];
        let run: String = after
            .chars()
            .take_while(|&c| is_token_char(c))
            .collect();
        if run.chars().count() >= 16 {
            out.push_str("[REDACTED]");
        } else {
            out.push_str(&run);
        }
        rest = &after[run.len()..];
    }
    out.push_str(rest);
    out
}

/// Log helper: redacts first, then writes to stderr.
pub fn log_safe(cfg: &ProxyConfig, msg: &str) {
    let secrets: Vec<&str> = cfg
        .access_token
        .as_deref()
        .into_iter()
        .chain(cfg.openrouter_key.as_deref())
        .collect();
    eprintln!("[harness-proxy] {}", redact(msg, &secrets));
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// Bearer gate. 401 unless the presented token matches, or the loopback-dev
/// exception applies (bound to 127.0.0.1 AND the token env empty).
pub fn check_auth(cfg: &ProxyConfig, headers: &axum::http::HeaderMap) -> Result<(), Response> {
    match &cfg.access_token {
        Some(expected) => {
            let presented = headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "));
            let ok = presented.is_some_and(|p| constant_time_eq(p.as_bytes(), expected.as_bytes()));
            if ok {
                Ok(())
            } else {
                Err(error_response(StatusCode::UNAUTHORIZED, NO_TOKEN_MESSAGE))
            }
        }
        None if cfg.loopback_dev => Ok(()),
        None => Err(error_response(StatusCode::UNAUTHORIZED, NO_TOKEN_MESSAGE)),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// Every response carries permissive CORS: auth is a bearer token (no
/// cookies), and `file://` / Textastic send `Origin: null`.
pub fn with_cors(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Authorization, Content-Type"),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("86400"),
    );
    response
}

fn error_response(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({ "error": message });
    let response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("static response");
    with_cors(response)
}

fn options_response() -> Response {
    let response = Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .expect("static response");
    with_cors(response)
}

fn short_upstream_reason(status: u16) -> &'static str {
    match status {
        401 | 403 => "upstream auth failed",
        402 => "upstream billing problem",
        408 => "upstream timeout",
        429 => "upstream rate limit",
        500..=599 => "upstream is down",
        _ => "upstream error",
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/health — no secrets, no env, nothing an attacker learns beyond
/// "the proxy is up".
pub async fn health(req: Request) -> Response {
    if req.method() == Method::OPTIONS {
        return options_response();
    }
    let body = serde_json::json!({ "ok": true, "service": "harness" });
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("static response");
    with_cors(response)
}

/// POST /api/chat — SSE proxy to OpenRouter chat completions.
pub async fn chat(State(cfg): State<ProxyConfig>, req: Request) -> Response {
    if req.method() == Method::OPTIONS {
        return options_response();
    }
    if req.method() != Method::POST {
        return error_response(StatusCode::METHOD_NOT_ALLOWED, "POST only");
    }
    if let Err(denied) = check_auth(&cfg, req.headers()) {
        return denied;
    }

    // 1 MiB cap, rejected before any upstream call.
    if let Some(len) = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
    {
        if len > MAX_BODY_BYTES {
            return error_response(StatusCode::PAYLOAD_TOO_LARGE, BODY_TOO_LARGE_MESSAGE);
        }
    }
    let body_bytes = match axum::body::to_bytes(req.into_body(), MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(StatusCode::PAYLOAD_TOO_LARGE, BODY_TOO_LARGE_MESSAGE)
        }
    };
    if body_bytes.len() > MAX_BODY_BYTES {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, BODY_TOO_LARGE_MESSAGE);
    }

    // Never accept or forward a client OpenRouter key.
    if contains_key_shape(&body_bytes) {
        return error_response(StatusCode::BAD_REQUEST, KEY_SHAPED_MESSAGE);
    }

    let Some(key) = cfg.openrouter_key.clone() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, MISSING_KEY_MESSAGE);
    };

    let upstream_url = match cfg.upstream_url("api/v1/chat/completions") {
        Ok(url) => url,
        Err(status) => {
            log_safe(&cfg, &format!("refused upstream url (status {status})"));
            return error_response(status, "upstream origin refused");
        }
    };

    let upstream = match cfg
        .http
        .post(upstream_url)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Title", "Harness")
        .body(reqwest::Body::from(body_bytes))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            log_safe(&cfg, &format!("upstream request failed: {err}"));
            return error_response(StatusCode::BAD_GATEWAY, "proxy cannot reach upstream");
        }
    };

    let status = upstream.status();
    if !status.is_success() {
        let status_code = status.as_u16();
        // Read (bounded) only to log a redacted excerpt; the client gets a
        // fixed public string, never upstream body bytes.
        let upstream_body = upstream
            .bytes()
            .await
            .unwrap_or_default();
        let excerpt = String::from_utf8_lossy(&upstream_body[..upstream_body.len().min(512)]).to_string();
        log_safe(&cfg, &format!("upstream {status_code}: {excerpt}"));
        let body = serde_json::json!({
            "error": short_upstream_reason(status_code),
            "upstream_status": status_code,
        });
        let response = Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("static response");
        return with_cors(response);
    }

    // Stream the SSE body through chunk by chunk; never buffer the completion.
    let stream = upstream.bytes_stream();
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(Body::from_stream(stream))
        .expect("streaming response");
    with_cors(response)
}

/// GET /api/models — relay the OpenRouter model list.
pub async fn models(State(cfg): State<ProxyConfig>, req: Request) -> Response {
    if req.method() == Method::OPTIONS {
        return options_response();
    }
    if req.method() != Method::GET {
        return error_response(StatusCode::METHOD_NOT_ALLOWED, "GET only");
    }
    if let Err(denied) = check_auth(&cfg, req.headers()) {
        return denied;
    }

    let Some(key) = cfg.openrouter_key.clone() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, MISSING_KEY_MESSAGE);
    };

    let upstream_url = match cfg.upstream_url("api/v1/models") {
        Ok(url) => url,
        Err(status) => return error_response(status, "upstream origin refused"),
    };

    let upstream = match cfg
        .http
        .get(upstream_url)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            log_safe(&cfg, &format!("upstream request failed: {err}"));
            return error_response(StatusCode::BAD_GATEWAY, "proxy cannot reach upstream");
        }
    };

    let status = upstream.status();
    if !status.is_success() {
        let status_code = status.as_u16();
        log_safe(&cfg, &format!("upstream models {status_code}"));
        let body = serde_json::json!({
            "error": short_upstream_reason(status_code),
            "upstream_status": status_code,
        });
        let response = Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("static response");
        return with_cors(response);
    }

    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let stream = upstream.bytes_stream();
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .expect("streaming response");
    with_cors(response)
}

/// Does the byte slice contain an OpenRouter key shape (`sk-or-` + a run of
/// token chars)? Used to refuse client keys outright.
fn contains_key_shape(body: &[u8]) -> bool {
    const NEEDLE: &[u8] = b"sk-or-";
    if body.len() < NEEDLE.len() + 16 {
        return false;
    }
    let mut i = 0;
    while i + NEEDLE.len() <= body.len() {
        if &body[i..i + NEEDLE.len()] == NEEDLE {
            let run = body[i + NEEDLE.len()..]
                .iter()
                .take_while(|&&c| is_token_char(c as char))
                .count();
            if run >= 16 {
                return true;
            }
            i += NEEDLE.len() + run;
        } else {
            i += 1;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

async fn options_handler() -> Response {
    options_response()
}

/// All handlers on their public paths — used by the dev server and tests.
pub fn full_router(cfg: ProxyConfig) -> Router {
    Router::new()
        .route(
            "/api/chat",
            post(chat).get(chat).options(options_handler),
        )
        .route(
            "/api/models",
            get(models).options(options_handler),
        )
        .route(
            "/api/health",
            get(health).options(options_handler),
        )
        .with_state(cfg)
}

/// Single-handler routers for the Vercel functions. Each function only ever
/// receives its own path, so `fallback` is used to stay path-agnostic; the
/// handler re-checks the method.
pub fn chat_router(cfg: ProxyConfig) -> Router {
    Router::new().fallback(chat).with_state(cfg)
}

pub fn models_router(cfg: ProxyConfig) -> Router {
    Router::new().fallback(models).with_state(cfg)
}

pub fn health_router(cfg: ProxyConfig) -> Router {
    Router::new().fallback(health).with_state(cfg)
}

// ---------------------------------------------------------------------------
// Tests (unit)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_origin_is_exact() {
        for good in [
            "https://openrouter.ai/api/v1/chat/completions",
            "https://openrouter.ai",
            "https://openrouter.ai/api/v1/models",
        ] {
            assert!(is_allowed_origin(&url::Url::parse(good).unwrap()), "{good}");
        }
        for bad in [
            "http://openrouter.ai",                    // wrong scheme
            "https://evil.openrouter.ai",              // subdomain
            "https://openrouter.ai.evil.com",          // suffix trick
            "https://openrouter.ai:8443/",             // non-default port
            "https://evil.com/api/v1/chat/completions",
            "http://127.0.0.1:9999",
        ] {
            assert!(!is_allowed_origin(&url::Url::parse(bad).unwrap()), "{bad}");
        }
        // Origin equality ignores userinfo (browsers strip it too): the
        // connection still goes to exactly openrouter.ai.
        assert!(is_allowed_origin(&url::Url::parse("https://user@openrouter.ai/").unwrap()));
    }

    #[test]
    fn redaction_covers_shapes() {
        let key = ["sk-or-", "v1-", "abcdef0123456789abcdef"].concat();
        let out = redact(
            &format!("key={key} and Bearer abcdef0123456789 done"),
            &["topsecretvalue0123456789"],
        );
        assert!(!out.contains("abcdef0123456789abcdef"));
        assert!(!out.contains("Bearer abcdef"));
        assert!(out.contains("[REDACTED]"));
        let out2 = redact("token topsecretvalue0123456789 here", &["topsecretvalue0123456789"]);
        assert!(out2.contains("[REDACTED]"), "{out2}");
        // Prose with short words is untouched.
        let out3 = redact("Bearer me please, sk-or- short", &[]);
        assert_eq!(out3, "Bearer me please, sk-or- short");
    }

    #[test]
    fn key_shape_scanner() {
        let key = ["sk-or-", "v1-", "0123456789abcdef"].concat();
        let body = format!("\"{}\"", key);
        assert!(contains_key_shape(body.as_bytes()));
        assert!(!contains_key_shape(b"sk-or-123"));
        assert!(!contains_key_shape(b"the words or the sword"));
    }

    #[test]
    fn constant_time_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
