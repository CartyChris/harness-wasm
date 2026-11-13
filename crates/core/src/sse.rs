//! Incremental SSE parsing for the client side of `/api/chat`.
//!
//! Pure Rust: bytes in (already decoded to text), SSE `data:` payloads out.
//! Handles `\n\n`, `\r\n\r\n`, and `\r\r` event boundaries across chunk
//! splits, multi-line `data:` fields, comment lines, and `event:`/`id:` lines.

#[derive(Debug, Default)]
pub struct SseParser {
    buf: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a decoded chunk; appends any complete events' data payloads.
    pub fn feed(&mut self, chunk: &str, events: &mut Vec<String>) {
        self.buf.push_str(chunk);
        while let Some((block, consumed)) = self.next_event_block() {
            if let Some(data) = parse_block(&block) {
                events.push(data);
            }
            self.buf.drain(..consumed);
        }
    }

    /// Flush a trailing block that never saw a blank-line terminator (a
    /// truncated-but-ended stream).
    pub fn finish(&mut self, events: &mut Vec<String>) {
        let rest = self.buf.clone();
        self.buf.clear();
        if rest.trim().is_empty() {
            return;
        }
        if let Some(data) = parse_block(&rest) {
            events.push(data);
        }
    }

    /// Find the earliest event-boundary in the buffer; returns the block
    /// before it and how many bytes to consume (including the boundary).
    fn next_event_block(&self) -> Option<(String, usize)> {
        let bytes = self.buf.as_bytes();
        let mut best: Option<(usize, usize)> = None; // (position, boundary_len)
        for pattern in [b"\n\n".as_slice(), b"\r\n\r\n".as_slice(), b"\r\r".as_slice()] {
            if let Some(pos) = find_sub(bytes, pattern) {
                let better = match best {
                    Some((bp, _)) if bp <= pos => false,
                    _ => true,
                };
                if better {
                    best = Some((pos, pattern.len()));
                }
            }
        }
        best.map(|(pos, len)| (self.buf[..pos].to_string(), pos + len))
    }
}

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Extract the joined `data:` payload of one event block, or None for blocks
/// without data (comments, keep-alives).
fn parse_block(block: &str) -> Option<String> {
    let mut data: Vec<&str> = Vec::new();
    for line in block.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
        // `event:`, `id:`, `retry:`, and `:` comments are ignored.
    }
    if data.is_empty() {
        None
    } else {
        Some(data.join("\n"))
    }
}

// --- OpenRouter chunk payloads ----------------------------------------------

/// `choices[0].delta.content` (streaming) or `choices[0].message.content`
/// (non-stream reply) of an SSE data payload.
pub fn delta_content(data: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    let choice = value.get("choices")?.get(0)?;
    if let Some(delta) = choice.get("delta").and_then(|d| d.get("content")) {
        if let Some(text) = delta.as_str() {
            return Some(text.to_string());
        }
    }
    choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(str::to_string)
}

/// `error.message` if the payload is an in-stream error object.
pub fn error_message(data: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    value
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(str::to_string)
}

/// Everything one streamed chunk can carry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChunkDelta {
    pub content: Option<String>,
    /// Reasoning / thinking text some models stream before the answer.
    pub reasoning: Option<String>,
    pub tool_calls: Vec<crate::tools::ToolCallFragment>,
    pub finish_reason: Option<String>,
}

/// Parse one OpenRouter SSE `data:` payload (streaming chat chunk).
pub fn parse_chunk(data: &str) -> Option<ChunkDelta> {
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    let choice = value.get("choices")?.get(0)?;
    let mut out = ChunkDelta {
        content: delta_content(data),
        finish_reason: choice.get("finish_reason").and_then(|f| f.as_str()).map(str::to_string),
        ..Default::default()
    };
    if let Some(delta) = choice.get("delta") {
        out.reasoning = delta
            .get("reasoning")
            .or_else(|| delta.get("reasoning_content"))
            .and_then(|r| r.as_str())
            .filter(|r| !r.is_empty())
            .map(str::to_string);
        if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
            for (pos, call) in calls.iter().enumerate() {
                let function = call.get("function");
                out.tool_calls.push(crate::tools::ToolCallFragment {
                    index: call.get("index").and_then(|i| i.as_u64()).map(|i| i as usize).unwrap_or(pos),
                    id: call.get("id").and_then(|v| v.as_str()).map(str::to_string),
                    name: function.and_then(|f| f.get("name")).and_then(|v| v.as_str()).map(str::to_string),
                    arguments: function.and_then(|f| f.get("arguments")).and_then(|v| v.as_str()).map(str::to_string),
                });
            }
        }
    }
    Some(out)
}

/// Model ids from a `/api/models` JSON body.
pub fn model_ids(models_json: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(models_json).ok()?;
    let data = value.get("data")?.as_array()?;
    let mut ids: Vec<String> = data
        .iter()
        .filter_map(|entry| entry.get("id").and_then(|id| id.as_str()))
        .map(str::to_string)
        .collect();
    ids.dedup();
    Some(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(chunks: &[&str]) -> Vec<String> {
        let mut parser = SseParser::new();
        let mut events = Vec::new();
        for chunk in chunks {
            parser.feed(chunk, &mut events);
        }
        parser.finish(&mut events);
        events
    }

    #[test]
    fn simple_events() {
        let events = collect(&[
            "data: {\"a\":1}\n\ndata: {\"b\":2}\n\n",
        ]);
        assert_eq!(events, vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn chunks_split_anywhere() {
        // Every possible split point of a two-event stream still yields both.
        let raw = "data: one\n\ndata: two\n\n";
        for split in 0..raw.len() {
            let (a, b) = raw.split_at(split);
            let events = collect(&[a, b]);
            assert_eq!(events, vec!["one", "two"], "split at {split}");
        }
    }

    #[test]
    fn crlf_boundaries() {
        // `next` alone has no `data:` field, so that block yields nothing.
        let events = collect(&["data: a\r\ndata: b\r\n\r\nnext\r\rdata: c\n\n"]);
        assert_eq!(events, vec!["a\nb", "c"]);
    }

    #[test]
    fn comments_and_fields_ignored() {
        let events = collect(&[": keep-alive\n\nevent: x\ndata: payload\nid: 7\n\n"]);
        assert_eq!(events, vec!["payload"]);
    }

    #[test]
    fn truncated_stream_flushes() {
        let mut parser = SseParser::new();
        let mut events = Vec::new();
        parser.feed("data: partial", &mut events);
        assert!(events.is_empty());
        parser.finish(&mut events);
        assert_eq!(events, vec!["partial"]);
    }

    #[test]
    fn done_sentinel_survives() {
        let events = collect(&["data: [DONE]\n\n"]);
        assert_eq!(events, vec!["[DONE]"]);
    }

    #[test]
    fn delta_extraction() {
        assert_eq!(
            delta_content(r#"{"choices":[{"delta":{"content":"hi"}}]}"#),
            Some("hi".to_string())
        );
        assert_eq!(
            delta_content(r#"{"choices":[{"delta":{}}]}"#),
            None
        );
        assert_eq!(delta_content("[DONE]"), None);
        assert_eq!(
            delta_content(r#"{"choices":[{"message":{"content":"full"}}]}"#),
            Some("full".to_string())
        );
    }

    #[test]
    fn error_extraction() {
        assert_eq!(
            error_message(r#"{"error":{"message":"rate limited"}}"#),
            Some("rate limited".to_string())
        );
        assert_eq!(error_message(r#"{"choices":[]}"#), None);
    }

    #[test]
    fn model_ids_extraction() {
        let ids = model_ids(r#"{"data":[{"id":"x-ai/grok-4.7"},{"id":"openai/gpt-4o-mini"}]}"#);
        assert_eq!(
            ids,
            Some(vec!["x-ai/grok-4.7".to_string(), "openai/gpt-4o-mini".to_string()])
        );
        assert_eq!(model_ids(r#"{"nope":1}"#), None);
    }

    #[test]
    fn parse_chunk_reads_tool_call_fragments() {
        let d = parse_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"write_file","arguments":"{\"pa"}}]},"finish_reason":null}]}"#).unwrap();
        assert_eq!(d.tool_calls.len(), 1);
        assert_eq!(d.tool_calls[0].name.as_deref(), Some("write_file"));
        assert_eq!(d.tool_calls[0].arguments.as_deref(), Some("{\"pa"));
        let end = parse_chunk(r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#).unwrap();
        assert_eq!(end.finish_reason.as_deref(), Some("tool_calls"));
        let r = parse_chunk(r#"{"choices":[{"delta":{"reasoning":"hmm","content":""}}]}"#).unwrap();
        assert_eq!(r.reasoning.as_deref(), Some("hmm"));
        assert_eq!(parse_chunk("[DONE]"), None);
    }
}
