//! Session/message model shared across the client. Kept free of any
//! framework so the pure logic stays host-testable.

use serde::{Deserialize, Serialize};

pub const KIND_USER: &str = "user";
pub const KIND_ASSISTANT: &str = "assistant";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub updated_at: f64,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    #[serde(rename = "type")]
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Done | Streaming | Interrupted | Cancelled | Failed
    #[serde(default = "default_status")]
    pub status: String,
    /// Short user-facing reason when `status` is Failed/Interrupted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn default_status() -> String {
    "Done".to_string()
}

impl Message {
    pub fn user(id: String, content: String) -> Self {
        Message {
            id,
            role: KIND_USER.to_string(),
            content,
            model: None,
            status: "Done".to_string(),
            error: None,
        }
    }

    pub fn assistant(id: String, model: String) -> Self {
        Message {
            id,
            role: KIND_ASSISTANT.to_string(),
            content: String::new(),
            model: Some(model),
            status: "Streaming".to_string(),
            error: None,
        }
    }

    pub fn is_user(&self) -> bool {
        self.role == KIND_USER
    }
}

/// Wire message sent to `/api/chat`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Proxy base URL, e.g. `https://harness-proxy.vercel.app`. Empty string
    /// means same origin as the page.
    #[serde(default)]
    pub base_url: String,
    /// Harness access token (the bearer gate) — NEVER an OpenRouter key.
    #[serde(default)]
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_serde_roundtrip() {
        let m = Message::user("m1".into(), "hello".into());
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"type\":\"user\""));
        let back: Message = serde_json::from_str(&json).unwrap();
        assert!(back.is_user());

        // Old exports without a status field still load.
        let legacy = r#"{"id":"m2","type":"assistant","content":"hi","model":"x-ai/grok-4.7"}"#;
        let m2: Message = serde_json::from_str(legacy).unwrap();
        assert_eq!(m2.status, "Done");
    }
}
