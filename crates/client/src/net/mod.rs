//! Proxy client: fetch + SSE stream reader. Carries only the proxy base URL
//! and the harness access token — never an OpenRouter key.

use crate::js;
use harness_core::sse::{self, ChunkDelta, SseParser};
use harness_core::Settings;
use wasm_bindgen::{closure::Closure, JsCast};
use wasm_bindgen_futures::JsFuture;

#[derive(Debug, Clone)]
pub enum NetError {
    /// 401 from the proxy — open the Connect card.
    Unauthorized,
    /// Network-level failure (offline, DNS, CORS, dropped stream).
    Unreachable(String),
    /// The proxy answered with an error body ({"error": "..."}).
    Api { status: u16, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Cancelled,
}

fn chat_url(base_url: &str) -> String {
    format!("{}/api/chat", base_url.trim_end_matches('/'))
}

fn models_url(base_url: &str) -> String {
    format!("{}/api/models", base_url.trim_end_matches('/'))
}

fn err_text(err: &wasm_bindgen::JsValue) -> String {
    err.as_string().unwrap_or_default()
}

/// Stream one model step. `on_chunk` is called with each parsed chunk
/// (text, reasoning, tool-call fragments) as it arrives. Returns when the
/// stream ends, is cancelled via `abort`, or fails. `body` is the full
/// OpenAI-compatible request (model, messages, stream, tools).
pub async fn stream_chat(
    settings: &Settings,
    body: &serde_json::Value,
    on_chunk: &dyn Fn(ChunkDelta),
    abort: &web_sys::AbortController,
) -> Result<Outcome, NetError> {
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_body(&wasm_bindgen::JsValue::from_str(&body.to_string()));
    let signal = abort.signal();
    init.set_signal(Some(&signal));
    let headers = web_sys::Headers::new().expect("headers");
    headers
        .set("Content-Type", "application/json")
        .expect("content-type");
    if !settings.token.is_empty() {
        headers
            .set("Authorization", &format!("Bearer {}", settings.token))
            .expect("authorization");
    }
    init.set_headers(headers.unchecked_ref());

    let request = web_sys::Request::new_with_str_and_init(&chat_url(&settings.base_url), &init)
        .map_err(|e| NetError::Unreachable(err_text(&e)))?;

    let response = match JsFuture::from(js::window().fetch_with_request(&request)).await {
        Ok(value) => value.dyn_into::<web_sys::Response>().expect("fetch Response"),
        Err(err) => {
            return if abort.signal().aborted() {
                Ok(Outcome::Cancelled)
            } else {
                Err(NetError::Unreachable(format!(
                    "request failed: {}",
                    short_reason(&err_text(&err))
                )))
            };
        }
    };

    let status = response.status();
    if status == 401 {
        return Err(NetError::Unauthorized);
    }
    if !response.ok() {
        let text = JsFuture::from(response.text().expect("response.text()"))
            .await
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("proxy error ({status})"));
        return Err(NetError::Api { status, message });
    }

    let stream = response
        .body()
        .ok_or_else(|| NetError::Unreachable("empty response body".to_string()))?;
    let reader: web_sys::ReadableStreamDefaultReader = stream
        .get_reader()
        .dyn_into()
        .map_err(|_| NetError::Unreachable("cannot read stream".to_string()))?;

    let decoder = web_sys::TextDecoder::new().expect("TextDecoder");
    let decode_options = web_sys::TextDecodeOptions::new();
    decode_options.set_stream(true);
    let mut parser = SseParser::new();
    let mut events: Vec<String> = Vec::new();

    loop {
        let read = match JsFuture::from(reader.read()).await {
            Ok(value) => value,
            Err(err) => {
                return if abort.signal().aborted() {
                    Ok(Outcome::Cancelled)
                } else {
                    Err(NetError::Unreachable(format!(
                        "stream dropped: {}",
                        short_reason(&err_text(&err))
                    )))
                };
            }
        };
        let done = js_sys::Reflect::get(&read, &"done".into())
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if done {
            break;
        }
        let value = js_sys::Reflect::get(&read, &"value".into())
            .map_err(|_| NetError::Unreachable("bad chunk".to_string()))?;
        let bytes = js_sys::Uint8Array::from(value).to_vec();
        let chunk = decoder
            .decode_with_u8_array_and_options(&bytes, &decode_options)
            .map_err(|e| NetError::Unreachable(err_text(&e)))?;

        parser.feed(&chunk, &mut events);
        for data in events.drain(..) {
            if data == "[DONE]" {
                return Ok(Outcome::Done);
            }
            if let Some(message) = sse::error_message(&data) {
                return Err(NetError::Api { status: 200, message });
            }
            if let Some(chunk) = sse::parse_chunk(&data) {
                on_chunk(chunk);
            }
        }
    }

    // Stream ended without [DONE]; flush any trailing event.
    parser.finish(&mut events);
    for data in events {
        if let Some(chunk) = sse::parse_chunk(&data) {
            on_chunk(chunk);
        }
    }
    Ok(Outcome::Done)
}

/// Fetch the model list (best-effort; a failure never touches the locked
/// model).
pub async fn fetch_models(settings: &Settings) -> Result<Vec<String>, NetError> {
    let init = web_sys::RequestInit::new();
    init.set_method("GET");
    if !settings.token.is_empty() {
        let headers = web_sys::Headers::new().expect("headers");
        headers
            .set("Authorization", &format!("Bearer {}", settings.token))
            .expect("authorization");
        init.set_headers(headers.unchecked_ref());
    }
    let request = web_sys::Request::new_with_str_and_init(&models_url(&settings.base_url), &init)
        .map_err(|e| NetError::Unreachable(err_text(&e)))?;
    let response = match JsFuture::from(js::window().fetch_with_request(&request)).await {
        Ok(value) => value.dyn_into::<web_sys::Response>().expect("fetch Response"),
        Err(err) => {
            return Err(NetError::Unreachable(format!(
                "request failed: {}",
                short_reason(&err_text(&err))
            )))
        }
    };
    let status = response.status();
    if status == 401 {
        return Err(NetError::Unauthorized);
    }
    if !response.ok() {
        return Err(NetError::Api {
            status,
            message: "proxy error".to_string(),
        });
    }
    let text = JsFuture::from(response.text().expect("response.text()"))
        .await
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    sse::model_ids(&text)
        .filter(|ids| !ids.is_empty())
        .ok_or_else(|| NetError::Api {
            status,
            message: "unparsable model list".to_string(),
        })
}

/// Best-effort health probe for the Connect card.
pub async fn health(settings: &Settings) -> Result<bool, NetError> {
    let url = format!("{}/api/health", settings.base_url.trim_end_matches('/'));
    let request = web_sys::Request::new_with_str(&url)
        .map_err(|e| NetError::Unreachable(err_text(&e)))?;
    match JsFuture::from(js::window().fetch_with_request(&request)).await {
        Ok(value) => {
            let response: web_sys::Response =
                value.dyn_into().expect("fetch Response");
            Ok(response.ok())
        }
        Err(err) => Err(NetError::Unreachable(format!(
            "request failed: {}",
            short_reason(&err_text(&err))
        ))),
    }
}

/// Keep raw JS error strings out of the UI: truncate and strip anything that
/// looks like a URL or key (paranoia; the proxy never sends keys).
fn short_reason(raw: &str) -> String {
    let mut out: String = raw.chars().take(120).collect();
    if out.is_empty() {
        out.push_str("network error");
    }
    if out.contains("Failed to fetch") || out.contains("NetworkError") {
        out = "proxy unreachable (offline, wrong URL, or CORS)".to_string();
    }
    out
}

// `Closure` is used by js.rs; referenced here to keep the import when tree
// shaking warnings appear in some toolchains.
#[allow(dead_code)]
fn _closure_type_anchor() -> Option<Closure<dyn FnMut()>> {
    None
}
