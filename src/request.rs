use anyhow::{Context, bail};
use serde_json::Value;
use std::time::Duration;

use crate::cli::RequestArgs;
use crate::config::Config;

/// Result of a completed Chat Completions request. `raw` carries the
/// unmodified provider response so `--json` can pipe the full response
/// object, while the extracted fields serve `--text` and the TUI header.
#[derive(Debug, Clone)]
pub struct RequestResponse {
    pub status: u16,
    pub raw: Value,
    pub usage: Option<Value>,
    pub assistant_content: Option<String>,
}

pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

/// Build a Chat Completions request body following the OpenAI schema:
/// `model` and `messages` are always present; sampling parameters sit at
/// the top level and are omitted entirely when not supplied.
pub fn build_payload(
    model: &str,
    messages: &[Value],
    temperature: Option<f32>,
    top_p: Option<f32>,
    max_tokens: Option<u32>,
    stop: &[String],
    stream: bool,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "model".to_string(),
        serde_json::Value::String(model.to_string()),
    );
    payload.insert(
        "messages".to_string(),
        serde_json::Value::Array(messages.to_vec()),
    );
    if let Some(t) = temperature {
        payload.insert("temperature".to_string(), serde_json::json!(t));
    }
    if let Some(p) = top_p {
        payload.insert("top_p".to_string(), serde_json::json!(p));
    }
    if let Some(mt) = max_tokens {
        payload.insert(
            "max_tokens".to_string(),
            serde_json::Value::Number(mt.into()),
        );
    }
    if !stop.is_empty() {
        payload.insert(
            "stop".to_string(),
            serde_json::Value::Array(
                stop.iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if stream {
        payload.insert("stream".to_string(), serde_json::Value::Bool(true));
        let mut stream_options = serde_json::Map::new();
        stream_options.insert("include_usage".to_string(), serde_json::Value::Bool(true));
        payload.insert("stream_options".to_string(), serde_json::Value::Object(stream_options));
    }
    serde_json::Value::Object(payload)
}

/// Resolve the effective request settings: CLI flag > env var > config file.
/// `base_url` and `api_key` come from `--base-url`/`--api-key`, then the
/// `LLMHELPER_API_KEY` env var (key only), then the `[request]` config
/// section. The model falls back to `[request] default_model` when `--model`
/// is absent.
#[derive(Debug, Clone)]
pub struct RequestSettings {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub timeout: Duration,
}

impl RequestSettings {
    pub fn resolve(args: &RequestArgs, config: &Config) -> anyhow::Result<Self> {
        let base_url = args
            .base_url
            .clone()
            .or_else(|| config.request_base_url.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("base-url is required (via --base-url or config [request] section)")
            })?
            .trim_end_matches('/')
            .to_string();
        let api_key = args
            .api_key
            .clone()
            .or_else(|| std::env::var("LLMHELPER_API_KEY").ok())
            .or_else(|| config.request_api_key.clone());
        let model = args
            .model
            .clone()
            .or_else(|| config.request_default_model.clone())
            .ok_or_else(|| anyhow::anyhow!("model is required (via --model or config [request] default_model)"))?;
        let timeout = Duration::from_secs(
            config
                .request_timeout_seconds
                .unwrap_or(DEFAULT_TIMEOUT_SECONDS),
        );
        Ok(Self {
            base_url,
            api_key,
            model,
            timeout,
        })
    }
}

/// Send the Chat Completions request and extract the fields the CLI/TUI
/// need from the provider response. Non-2xx responses carry the body
/// snippet in the error message.
pub async fn send_chat_completion(
    settings: &RequestSettings,
    payload: &Value,
) -> anyhow::Result<RequestResponse> {
    let endpoint = format!("{}/v1/chat/completions", settings.base_url);
    let mut req = reqwest::Client::builder()
        .timeout(settings.timeout)
        .build()?
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .json(payload);
    if let Some(key) = &settings.api_key {
        req = req.header("Authorization", format!("Bearer {}", key));
    }
    let resp = req
        .send()
        .await
        .context("failed to send request to OpenAI-compatible endpoint")?;
    let status_code = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .context("failed to read response body")?;
    if !(200..300).contains(&status_code) {
        bail!(
            "HTTP {}: {}",
            status_code,
            body.chars().take(500).collect::<String>()
        );
    }
    let raw: Value = serde_json::from_str(&body).context("failed to parse response JSON")?;
    Ok(parse_response(status_code, raw))
}

#[derive(Debug)]
pub struct StreamResponse {
    pub status: u16,
    pub usage: Option<Value>,
    pub content: String,
}

/// Stream a Chat Completions request, feeding each parsed SSE event to `on_event`.
/// The function returns the final HTTP status, the last usage object seen, and the
/// concatenated content accumulated from all `delta.content` events.
pub async fn send_chat_completion_stream<F>(
    settings: &RequestSettings,
    payload: &Value,
    mut on_event: F,
) -> anyhow::Result<StreamResponse>
where
    F: FnMut(Value),
{
    use futures_util::StreamExt;
    let endpoint = format!("{}/v1/chat/completions", settings.base_url);
    let client = reqwest::Client::builder()
        .connect_timeout(settings.timeout)
        .build()?;
    let mut req = client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .json(payload);
    if let Some(key) = &settings.api_key {
        req = req.header("Authorization", format!("Bearer {}", key));
    }
    let resp = req.send().await.context("failed to send request to OpenAI-compatible endpoint")?;
    let status_code = resp.status().as_u16();
    if !(200..300).contains(&status_code) {
        let body = resp.text().await.unwrap_or_default();
        bail!("HTTP {}: {}", status_code, body.chars().take(500).collect::<String>());
    }
    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::new();
    let mut content = String::new();
    let mut usage = None;
    loop {
        let maybe_chunk = stream.next().await;
        match maybe_chunk {
            Some(Ok(chunk)) => {
                let text = String::from_utf8_lossy(&chunk);
                let events = parser.feed(&text);
                for event in events {
                    on_event(event.clone());
                    if let Some(delta) = extract_delta_content(&event) {
                        content.push_str(&delta);
                    }
                    if let Some(u) = extract_stream_usage(&event) {
                        usage = Some(u);
                    }
                }
            }
            Some(Err(e)) => {
                bail!("stream interrupted: {}", e);
            }
            None => break,
        }
    }
    let final_events = parser.finish();
    for event in final_events {
        on_event(event.clone());
        if let Some(delta) = extract_delta_content(&event) {
            content.push_str(&delta);
        }
        if let Some(u) = extract_stream_usage(&event) {
            usage = Some(u);
        }
    }
    Ok(StreamResponse {
        status: status_code,
        usage,
        content,
    })
}

/// Extract `usage` and the first assistant content from a Chat
/// Completions response. Pure function of the response JSON.
pub fn parse_response(status: u16, raw: Value) -> RequestResponse {
    let usage = raw.get("usage").cloned();
    let assistant_content = extract_assistant_content(&raw);
    RequestResponse {
        status,
        raw,
        usage,
        assistant_content,
    }
}

/// Read the first choice's message content, if present and textual.
pub fn extract_assistant_content(raw: &Value) -> Option<String> {
    raw.get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()
        .map(str::to_string)
}

/// Load messages from a JSON file of `{role, content}` objects, validating
/// each entry has a non-empty string role and content.
pub fn load_messages_file(path: &std::path::Path) -> anyhow::Result<Vec<Value>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read messages file: {}", path.display()))?;
    let messages: Vec<Value> = serde_json::from_str(&content)
        .context("failed to parse messages JSON (expected array of {role, content} objects)")?;
    for msg in &messages {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
        if role.is_empty() || content.is_empty() {
            bail!("each message must have non-empty 'role' and 'content' fields");
        }
    }
    Ok(messages)
}

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
    pending_data: Option<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &str) -> Vec<Value> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line_raw = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 1..].to_string();
            let line = line_raw.trim_end_matches('\r');
            self.process_line(line, &mut events);
        }
        events
    }

    pub fn finish(mut self) -> Vec<Value> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = self.buffer.trim_end_matches('\r').to_string();
            self.buffer.clear();
            self.process_line(&line, &mut events);
        }
        self.flush_event(&mut events);
        events
    }

    fn process_line(&mut self, line: &str, events: &mut Vec<Value>) {
        if line.is_empty() {
            self.flush_event(events);
            return;
        }
        if line.starts_with(':') {
            return;
        }
        if let Some(colon) = line.find(": ") {
            let field = &line[..colon];
            let value = &line[colon + 2..];
            if field == "data" {
                match &mut self.pending_data {
                    Some(existing) => {
                        existing.push('\n');
                        existing.push_str(value);
                    }
                    None => {
                        self.pending_data = Some(value.to_string());
                    }
                }
            }
        }
    }

    fn flush_event(&mut self, events: &mut Vec<Value>) {
        if let Some(data) = self.pending_data.take() {
            if data.trim() == "[DONE]" {
                return;
            }
            if !data.trim().is_empty() {
                if let Ok(val) = serde_json::from_str::<Value>(&data) {
                    events.push(val);
                }
            }
        }
    }
}

pub fn extract_delta_content(event: &Value) -> Option<String> {
    event
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("content"))
        .and_then(|content| content.as_str())
        .map(str::to_string)
}

pub fn extract_stream_usage(event: &Value) -> Option<Value> {
    event.get("usage").cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_msg(content: &str) -> Value {
        json!({"role": "user", "content": content})
    }

    #[test]
    fn build_payload_includes_model_and_messages() {
        let payload = build_payload("gpt-4", &[user_msg("Hello")], None, None, None, &[], false);
        assert_eq!(payload["model"].as_str().unwrap(), "gpt-4");
        assert_eq!(payload["messages"], json!([{"role": "user", "content": "Hello"}]));
    }

    #[test]
    fn build_payload_places_sampling_params_top_level() {
        let payload = build_payload(
            "gpt-4",
            &[user_msg("Hello")],
            Some(0.7),
            Some(0.95),
            Some(100),
            &["<stop>".to_string()],
            false,
        );
        let temp = payload["temperature"].as_f64().unwrap();
        let top_p = payload["top_p"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 0.001, "temperature was {}", temp);
        assert!((top_p - 0.95).abs() < 0.001, "top_p was {}", top_p);
        assert_eq!(payload["max_tokens"].as_u64().unwrap(), 100);
        assert_eq!(payload["stop"], json!(["<stop>"]));
        assert!(payload.get("generation_config").is_none());
    }

    #[test]
    fn build_payload_omits_absent_sampling_params() {
        let payload = build_payload("gpt-4", &[user_msg("Hello")], None, None, None, &[], false);
        for key in ["temperature", "top_p", "max_tokens", "stop"] {
            assert!(payload.get(key).is_none(), "{} should be omitted", key);
        }
    }

    #[test]
    fn build_payload_respects_multiple_stop_sequences() {
        let payload = build_payload(
            "gpt-4",
            &[user_msg("Hi")],
            None,
            None,
            None,
            &["<stop1>".to_string(), "<stop2>".to_string()],
            false,
        );
        let stop_arr = payload["stop"].as_array().unwrap();
        assert_eq!(stop_arr.len(), 2);
        assert_eq!(stop_arr[0].as_str().unwrap(), "<stop1>");
        assert_eq!(stop_arr[1].as_str().unwrap(), "<stop2>");
    }

    #[test]
    fn parse_response_extracts_usage_and_content() {
        let raw = json!({
            "model": "gpt-4",
            "choices": [{"message": {"role": "assistant", "content": "Hello back"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
        });
        let resp = parse_response(200, raw);
        assert_eq!(resp.status, 200);
        assert_eq!(
            resp.usage.as_ref().unwrap()["total_tokens"].as_u64().unwrap(),
            30
        );
        assert_eq!(resp.assistant_content, Some("Hello back".to_string()));
    }

    #[test]
    fn parse_response_handles_missing_content() {
        let raw = json!({"choices": [{"message": {"content": null}}]});
        let resp = parse_response(200, raw);
        assert_eq!(resp.assistant_content, None);
    }

    #[test]
    fn parse_response_handles_empty_choices() {
        let raw = json!({"choices": []});
        assert_eq!(extract_assistant_content(&raw), None);
    }

    #[test]
    fn load_messages_file_validates_entries() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("ok.json");
        std::fs::write(&ok, r#"[{"role":"user","content":"hi"}]"#).unwrap();
        assert_eq!(load_messages_file(&ok).unwrap().len(), 1);

        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, r#"[{"role":"","content":"hi"}]"#).unwrap();
        assert!(load_messages_file(&bad).is_err());

        let not_array = dir.path().join("na.json");
        std::fs::write(&not_array, r#"{"role":"user"}"#).unwrap();
        assert!(load_messages_file(&not_array).is_err());
    }

    #[test]
    fn settings_resolve_flag_over_config() {
        let config = Config {
            request_base_url: Some("https://config.example.com".to_string()),
            request_api_key: Some("config-key".to_string()),
            request_default_model: Some("config-model".to_string()),
            request_timeout_seconds: Some(120),
            ..Default::default()
        };
        let args = RequestArgs {
            base_url: Some("https://flag.example.com".to_string()),
            api_key: Some("flag-key".to_string()),
            ..Default::default()
        };
        let s = RequestSettings::resolve(&args, &config).unwrap();
        assert_eq!(s.base_url, "https://flag.example.com");
        assert_eq!(s.api_key.as_deref(), Some("flag-key"));
        assert_eq!(s.model, "config-model");
        assert_eq!(s.timeout, Duration::from_secs(120));
    }

    #[test]
    fn settings_resolve_from_config_only() {
        let config = Config {
            request_base_url: Some("https://config.example.com".to_string()),
            request_default_model: Some("config-model".to_string()),
            request_timeout_seconds: None,
            ..Default::default()
        };
        let s = RequestSettings::resolve(&RequestArgs::default(), &config).unwrap();
        assert_eq!(s.base_url, "https://config.example.com");
        assert_eq!(s.model, "config-model");
        assert_eq!(s.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECONDS));
    }

    #[test]
    fn settings_error_without_base_url_or_model() {
        assert!(RequestSettings::resolve(&RequestArgs::default(), &Config::default()).is_err());
        let config = Config {
            request_base_url: Some("https://x.example.com".to_string()),
            ..Default::default()
        };
        assert!(RequestSettings::resolve(&RequestArgs::default(), &config).is_err());
    }

    #[test]
    fn settings_base_url_strips_trailing_slash() {
        let config = Config {
            request_base_url: Some("https://api.example.com/v1/".to_string()),
            request_default_model: Some("m".to_string()),
            ..Default::default()
        };
        let s = RequestSettings::resolve(&RequestArgs::default(), &config).unwrap();
        assert_eq!(s.base_url, "https://api.example.com/v1");
    }

    #[test]
    fn settings_api_key_env_var_beats_config() {
        let config = Config {
            request_base_url: Some("https://x.example.com".to_string()),
            request_api_key: Some("config-key".to_string()),
            request_default_model: Some("m".to_string()),
            ..Default::default()
        };
        std::env::set_var("LLMHELPER_API_KEY", "env-key");
        let s = RequestSettings::resolve(&RequestArgs::default(), &config).unwrap();
        std::env::remove_var("LLMHELPER_API_KEY");
        assert_eq!(s.api_key.as_deref(), Some("env-key"));
    }

    #[test]
    fn sse_parser_parses_simple_event() {
        let mut parser = SseParser::new();
        let data = r#"data: {"choices":[{"delta":{"content":"hi"}}]}

"#;
        let events = parser.feed(data);
        assert_eq!(events.len(), 1);
        assert_eq!(extract_delta_content(&events[0]), Some("hi".to_string()));
    }

    #[test]
    fn sse_parser_joins_multiline_data() {
        let mut parser = SseParser::new();
        let data = "data: {\"a\":1}\n\n";
        let events = parser.feed(data);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["a"].as_u64().unwrap(), 1);

        let data2 = "data: {\"b\":2}\n\n";
        let events2 = parser.feed(data2);
        assert_eq!(events2.len(), 1);
        assert_eq!(events2[0]["b"].as_u64().unwrap(), 2);
    }

    #[test]
    fn sse_parser_ignores_comments_and_empty_lines() {
        let mut parser = SseParser::new();
        let data = ": ping\n\ndata: {\"x\":1}\n\n";
        let events = parser.feed(data);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["x"].as_u64().unwrap(), 1);
    }

    #[test]
    fn sse_parser_skips_done_sentinel() {
        let mut parser = SseParser::new();
        let data = "data: [DONE]\n\n";
        let events = parser.feed(data);
        assert!(events.is_empty());
    }

    #[test]
    fn sse_parser_handles_partial_chunk_boundaries() {
        let mut parser = SseParser::new();
        let part1 = "data: {\"choices\":[{\"delta\":{\"content\":\"hel";
        let part2 = "lo\"}}]}\n\n";
        let e1 = parser.feed(part1);
        assert!(e1.is_empty());
        let e2 = parser.feed(part2);
        assert_eq!(e2.len(), 1);
        assert_eq!(extract_delta_content(&e2[0]), Some("hello".to_string()));
    }

    #[test]
    fn extract_delta_content_returns_none_for_missing() {
        let v = json!({});
        assert_eq!(extract_delta_content(&v), None);
    }

    #[test]
    fn extract_stream_usage_returns_usage() {
        let v = json!({"usage": {"total_tokens": 10}});
        let usage = extract_stream_usage(&v).unwrap();
        assert_eq!(usage["total_tokens"].as_u64().unwrap(), 10);
    }

    #[test]
    fn build_payload_includes_stream_fields_when_streaming() {
        let payload = build_payload("m", &[], None, None, None, &[], true);
        assert!(payload["stream"].as_bool().unwrap());
        assert!(payload["stream_options"]["include_usage"].as_bool().unwrap());
    }

    #[test]
    fn build_payload_omits_stream_fields_when_not_streaming() {
        let payload = build_payload("m", &[], None, None, None, &[], false);
        assert!(payload.get("stream").is_none());
        assert!(payload.get("stream_options").is_none());
    }
}
