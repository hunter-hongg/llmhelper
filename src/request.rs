use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};
use serde_json::Value;

use crate::cli::RequestArgs;
use crate::config::Config;

/// Error classification for request operations. Determines the process exit
/// code: `Client` errors (network / DNS / timeout) exit with code 2;
/// `Request` errors (HTTP non-2xx, parse failure) exit with code 1.
#[derive(Debug)]
pub enum RequestError {
    /// Network-level failure: connection refused, DNS, timeout, stream
    /// interruption.
    Client(String),
    /// Request-level failure: non-2xx HTTP status, response JSON parse error.
    Request(String),
    /// Pre-flight budget gate refusal: a named budget is already over its
    /// ceiling, so no request is sent at all.
    Budget(String),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(msg) => write!(f, "client error: {msg}"),
            Self::Request(msg) => write!(f, "request error: {msg}"),
            Self::Budget(msg) => write!(f, "budget gate: {msg}"),
        }
    }
}

impl std::error::Error for RequestError {}

pub fn request_exit_code(err: &RequestError) -> i32 {
    match err {
        RequestError::Client(_) => 2,
        RequestError::Request(_) => 1,
        RequestError::Budget(_) => 3,
    }
}

/// Result of a completed Chat Completions request. `raw` carries the
/// unmodified provider response so `--json` can pipe the full response
/// object, while the extracted fields serve `--text` and the TUI header.
#[derive(Debug, Clone)]
pub struct RequestResponse {
    pub status: u16,
    pub raw: Value,
    pub usage: Option<Value>,
    pub assistant_content: Option<String>,
    /// Reasoning text captured from the fields named by `--reasoning-field`,
    /// joined in flag order. `None` when no field matched, or when no field
    /// was requested at all.
    pub reasoning: Option<String>,
    /// The field paths actually consulted, in flag order. Empty when capture
    /// is off, which is what keeps the `--json` envelope off for everyone
    /// else.
    pub reasoning_fields: Vec<String>,
}

pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

/// Sampling parameters for a Chat Completions request. Every field is
/// optional and omitted from the payload when `None`/empty.
#[derive(Debug, Default, Clone)]
pub struct SamplingParams {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stop: Vec<String>,
    /// Provider reasoning configuration, embedded verbatim as the payload's
    /// `reasoning` key when present. This command does not interpret it.
    pub reasoning: Option<Value>,
}

/// Build a Chat Completions request body following the OpenAI schema:
/// `model` and `messages` are always present; sampling parameters sit at
/// the top level and are omitted entirely when not supplied. `tools`, when
/// present, is embedded verbatim as the `tools` array.
pub fn build_payload(
    model: &str,
    messages: &[Value],
    params: &SamplingParams,
    stream: bool,
    tools: Option<&Value>,
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
    if let Some(t) = params.temperature {
        payload.insert("temperature".to_string(), serde_json::json!(t));
    }
    if let Some(p) = params.top_p {
        payload.insert("top_p".to_string(), serde_json::json!(p));
    }
    if let Some(mt) = params.max_tokens {
        payload.insert(
            "max_tokens".to_string(),
            serde_json::Value::Number(mt.into()),
        );
    }
    if !params.stop.is_empty() {
        payload.insert(
            "stop".to_string(),
            serde_json::Value::Array(
                params
                    .stop
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if let Some(tools) = tools {
        payload.insert("tools".to_string(), tools.clone());
    }
    if let Some(reasoning) = &params.reasoning {
        payload.insert("reasoning".to_string(), reasoning.clone());
    }
    if stream {
        payload.insert("stream".to_string(), serde_json::Value::Bool(true));
        let mut stream_options = serde_json::Map::new();
        stream_options.insert("include_usage".to_string(), serde_json::Value::Bool(true));
        payload.insert(
            "stream_options".to_string(),
            serde_json::Value::Object(stream_options),
        );
    }
    serde_json::Value::Object(payload)
}

/// Resolve the effective request settings: CLI flag > env var > config file.
/// `base_url` and `api_key` come from `--base-url`/`--api-key`, then the
/// `LLMHELPER_API_KEY` env var (key only), then the `[request]` config
/// section. The model falls back to `[request] default_model` when `--model`
/// is absent, and `reasoning_fields` to `[request] reasoning_fields` when no
/// `--reasoning-field` is given.
#[derive(Debug, Clone)]
pub struct RequestSettings {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub timeout: Duration,
    pub reasoning_fields: Vec<String>,
    /// Where `--log` writes the per-day request log. Always `Some` after
    /// `resolve`: config `[request] log_dir` (anchored to the config file's
    /// directory) or the default `log_dir()`. The write site uses this rather
    /// than re-deriving the directory, so a run cannot log to two places.
    pub log_dir: Option<PathBuf>,
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
            .ok_or_else(|| {
                anyhow::anyhow!("model is required (via --model or config [request] default_model)")
            })?;
        let timeout = Duration::from_secs(
            config
                .request_timeout_seconds
                .unwrap_or(DEFAULT_TIMEOUT_SECONDS),
        );
        let reasoning_fields = if args.reasoning_field.is_empty() {
            config.request_reasoning_fields.clone().unwrap_or_default()
        } else {
            args.reasoning_field.clone()
        };
        let log_dir = resolved_log_dir(config);
        Ok(Self {
            base_url,
            api_key,
            model,
            timeout,
            reasoning_fields,
            log_dir,
        })
    }
}

/// Build the POST request for the Chat Completions endpoint, shared by the
/// one-shot and streaming senders so the URL and auth wiring cannot drift.
///
/// `streaming` selects the client timeout policy: a one-shot request bounds
/// the whole exchange with `timeout`, while a streaming request must stay
/// open for as long as the server emits events and therefore only bounds the
/// initial connection.
fn build_request(
    settings: &RequestSettings,
    payload: &Value,
    streaming: bool,
) -> Result<reqwest::RequestBuilder, RequestError> {
    let endpoint = format!("{}/v1/chat/completions", settings.base_url);
    let builder = reqwest::Client::builder();
    let builder = if streaming {
        builder.connect_timeout(settings.timeout)
    } else {
        builder.timeout(settings.timeout)
    };
    let client = builder
        .build()
        .map_err(|e| RequestError::Client(format!("failed to set up HTTP client: {e}")))?;
    let mut req = client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .json(payload);
    if let Some(key) = &settings.api_key {
        req = req.header("Authorization", format!("Bearer {}", key));
    }
    Ok(req)
}

/// Send the Chat Completions request and extract the fields the CLI/TUI
/// need from the provider response. Non-2xx responses carry the body
/// snippet in the error message.
pub async fn send_chat_completion(
    settings: &RequestSettings,
    payload: &Value,
) -> Result<RequestResponse, RequestError> {
    let req = build_request(settings, payload, false)?;
    let resp = req.send().await.map_err(|e| {
        RequestError::Client(format!(
            "failed to send request to OpenAI-compatible endpoint: {e}"
        ))
    })?;
    let status_code = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .map_err(|e| RequestError::Client(format!("failed to read response body: {e}")))?;
    if !(200..300).contains(&status_code) {
        return Err(RequestError::Request(format!(
            "HTTP {}: {}",
            status_code,
            body.chars().take(500).collect::<String>()
        )));
    }
    let raw: Value = serde_json::from_str(&body)
        .map_err(|e| RequestError::Request(format!("failed to parse response JSON: {e}")))?;
    Ok(parse_response_with(
        status_code,
        raw,
        &settings.reasoning_fields,
    ))
}

#[derive(Debug)]
pub struct StreamResponse {
    pub status: u16,
    pub usage: Option<Value>,
    pub content: String,
    /// Reasoning text accumulated from each event's named field. Empty when
    /// capture is off or no event carried reasoning.
    pub reasoning: String,
}

/// Which text channel a single SSE event fed, if any. Determined by the
/// reasoning-field paths: an event that produced reasoning text is
/// `"reasoning"`, one that produced answer text is `"content"`, and one that
/// produced neither (usage-only, keep-alive, `[DONE]`) is `None`. Used to
/// annotate `--json --stream` lines and route `--text --stream` output without
/// restructuring the raw event.
pub fn event_channel(event: &Value, reasoning_fields: &[String]) -> Option<&'static str> {
    if extract_delta_content(event).is_some() {
        return Some("content");
    }
    if reasoning_fields
        .iter()
        .any(|p| extract_text_by_path(event, p).is_some())
    {
        return Some("reasoning");
    }
    None
}

/// Accumulates the observable state of an SSE stream: concatenated answer
/// content, concatenated reasoning text from each configured field path, and
/// the last usage object seen. Kept separate from the transport so the same
/// fold applies to live chunks and to whatever `SseParser::finish` returns.
#[derive(Default)]
struct StreamAccumulator {
    content: String,
    reasoning: String,
    usage: Option<Value>,
}

impl StreamAccumulator {
    fn feed(&mut self, event: &Value, reasoning_fields: &[String]) {
        if let Some(delta) = extract_delta_content(event) {
            self.content.push_str(&delta);
        }
        for field in reasoning_fields {
            if let Some(r) = extract_text_by_path(event, field) {
                self.reasoning.push_str(&r);
            }
        }
        if let Some(u) = extract_stream_usage(event) {
            self.usage = Some(u);
        }
    }
}

/// Stream a Chat Completions request, feeding each parsed SSE event to `on_event`.
/// The function returns the final HTTP status, the last usage object seen, and the
/// concatenated content accumulated from all `delta.content` events.
pub async fn send_chat_completion_stream<F>(
    settings: &RequestSettings,
    payload: &Value,
    mut on_event: F,
) -> Result<StreamResponse, RequestError>
where
    F: FnMut(Value),
{
    use futures_util::StreamExt;
    let req = build_request(settings, payload, true)?;
    let resp = req.send().await.map_err(|e| {
        RequestError::Client(format!(
            "failed to send request to OpenAI-compatible endpoint: {e}"
        ))
    })?;
    let status_code = resp.status().as_u16();
    if !(200..300).contains(&status_code) {
        let body = resp.text().await.unwrap_or_default();
        return Err(RequestError::Request(format!(
            "HTTP {}: {}",
            status_code,
            body.chars().take(500).collect::<String>()
        )));
    }
    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::new();
    let mut acc = StreamAccumulator::default();
    loop {
        let maybe_chunk = stream.next().await;
        match maybe_chunk {
            Some(Ok(chunk)) => {
                let text = String::from_utf8_lossy(&chunk);
                for event in parser.feed(&text) {
                    on_event(event.clone());
                    acc.feed(&event, &settings.reasoning_fields);
                }
            }
            Some(Err(e)) => {
                return Err(RequestError::Client(format!("stream interrupted: {e}")));
            }
            None => break,
        }
    }
    for event in parser.finish() {
        on_event(event.clone());
        acc.feed(&event, &settings.reasoning_fields);
    }
    Ok(StreamResponse {
        status: status_code,
        usage: acc.usage,
        content: acc.content,
        reasoning: acc.reasoning,
    })
}

/// Extract `usage` and the first assistant content from a Chat
/// Completions response. Pure function of the response JSON.
pub fn parse_response(status: u16, raw: Value) -> RequestResponse {
    parse_response_with(status, raw, &[])
}

/// As `parse_response`, additionally capturing reasoning from each field path
/// in `reasoning_fields`. The provider response is never modified; the
/// captured text is carried beside it. Empty paths mean capture is off, which
/// is what keeps every output mode byte-for-byte unchanged by default.
pub fn parse_response_with(
    status: u16,
    raw: Value,
    reasoning_fields: &[String],
) -> RequestResponse {
    let usage = raw.get("usage").cloned();
    let assistant_content = extract_assistant_content(&raw);
    let matches: Vec<String> = reasoning_fields
        .iter()
        .filter_map(|path| extract_text_by_path(&raw, path))
        .collect();
    let reasoning = if matches.is_empty() {
        None
    } else {
        Some(matches.join("\n\n"))
    };
    RequestResponse {
        status,
        raw,
        usage,
        assistant_content,
        reasoning,
        reasoning_fields: reasoning_fields.to_vec(),
    }
}

/// Walk a dot-separated path into a JSON value and return the string it
/// holds. A segment is an object key, or a decimal index when the current
/// value is an array (`choices.0.message.content`). A missing segment, a
/// non-object intermediate, a non-string leaf, and an empty or
/// whitespace-only string all read as absent — a field that does not match is
/// simply empty, never an error.
pub fn extract_text_by_path(value: &Value, path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = match current {
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => current.get(segment)?,
        };
    }
    let text = current.as_str()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Read the first choice's message content, if present and textual.
pub fn extract_assistant_content(raw: &Value) -> Option<String> {
    extract_text_by_path(raw, "choices.0.message.content")
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

/// Load a tools definition file: a JSON array of OpenAI tool objects. The
/// array is returned verbatim for embedding in the request payload; no
/// per-entry schema validation is performed beyond the array shape.
pub fn load_tools_file(path: &std::path::Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read tools file: {}", path.display()))?;
    let tools: Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse tools file: {}", path.display()))?;
    if !tools.is_array() {
        bail!("tools file must contain a JSON array of tool definitions");
    }
    Ok(tools)
}

/// Resolve a flag-supplied path relative to the loaded config file's
/// directory, so a `--config some/dir/config.toml` can carry `--reasoning
/// thinking.json` meaning the file next to that config. Delegates to the
/// shared resolver so flags and config-file values interpret paths the same
/// way.
pub fn resolve_path_against_config(
    config_path: Option<&std::path::Path>,
    candidate: std::path::PathBuf,
) -> std::path::PathBuf {
    crate::config::resolve_against_config(config_path, candidate)
}

/// Load a reasoning configuration file: a JSON object. The object is returned
/// verbatim for embedding as the request `reasoning` key; a non-object is a
/// usage error so a malformed config fails locally rather than as an HTTP 400.
pub fn load_reasoning_file(path: &std::path::Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read reasoning file: {}", path.display()))?;
    let reasoning: Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse reasoning file: {}", path.display()))?;
    if !reasoning.is_object() {
        bail!("reasoning file must contain a JSON object");
    }
    Ok(reasoning)
}

/// Directory under which per-day request logs are written. Resolves to
/// `~/.config/llmhelper/logs/`.
pub fn log_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("llmhelper").join("logs"))
}

/// The one log directory both halves of the loop use: `--log` writes here and
/// the `llmhelper` Source reads from here, so they can never disagree. Config
/// `[request] log_dir` (already anchored to the config file by `Config::load`)
/// wins; otherwise the platform default.
pub fn resolved_log_dir(config: &Config) -> Option<PathBuf> {
    config.request_log_dir.clone().or_else(log_dir)
}

/// Append one JSON entry (`timestamp`, `direction`, `body`) to the per-day
/// request log file under `log_dir`. Non-fatal: any I/O failure logs a warning
/// to stderr and is otherwise ignored. The API key is never included in
/// `body`.
pub fn write_request_log(log_dir: Option<&Path>, direction: &str, body: &str) {
    let Some(base) = log_dir else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(base) {
        eprintln!("warn: cannot create request log dir {:?}: {}", base, e);
        return;
    }
    let day = chrono::Utc::now().format("%Y-%m-%d");
    let path = base.join(format!("request-{}.log", day));
    let line = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "direction": direction,
        "body": body,
    });
    let line = format!("{}\n", serde_json::to_string(&line).unwrap_or_default());
    if let Err(e) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()))
    {
        eprintln!("warn: cannot write request log {:?}: {}", path, e);
    }
}

/// Build the OSC 52 escape sequence that sets the terminal clipboard to
/// `text`. Kept separate from the printing so it can be asserted on directly.
fn osc52_clipboard_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

/// Copy `text` to the terminal clipboard using the OSC 52 escape sequence.
/// Works in terminals that support OSC 52 (iTerm2, Alacritty, Kitty, WezTerm,
/// foot, ...). The sequence is written directly to stdout; if the terminal
/// does not support it the bytes are simply ignored.
pub fn copy_to_clipboard(text: &str) {
    print!("{}", osc52_clipboard_sequence(text));
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

/// Minimal base64 encoder (standard alphabet, with padding) so the clipboard
/// path does not pull in an extra dependency for a single escape sequence.
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
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
        let payload = build_payload(
            "gpt-4",
            &[user_msg("Hello")],
            &SamplingParams::default(),
            false,
            None,
        );
        assert_eq!(payload["model"].as_str().unwrap(), "gpt-4");
        assert_eq!(
            payload["messages"],
            json!([{"role": "user", "content": "Hello"}])
        );
    }

    #[test]
    fn build_payload_places_sampling_params_top_level() {
        let payload = build_payload(
            "gpt-4",
            &[user_msg("Hello")],
            &SamplingParams {
                temperature: Some(0.7),
                top_p: Some(0.95),
                max_tokens: Some(100),
                stop: vec!["<stop>".to_string()],
                ..Default::default()
            },
            false,
            None,
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
        let payload = build_payload(
            "gpt-4",
            &[user_msg("Hello")],
            &SamplingParams::default(),
            false,
            None,
        );
        for key in ["temperature", "top_p", "max_tokens", "stop"] {
            assert!(payload.get(key).is_none(), "{} should be omitted", key);
        }
    }

    #[test]
    fn build_payload_respects_multiple_stop_sequences() {
        let payload = build_payload(
            "gpt-4",
            &[user_msg("Hi")],
            &SamplingParams {
                stop: vec!["<stop1>".to_string(), "<stop2>".to_string()],
                ..Default::default()
            },
            false,
            None,
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
            resp.usage.as_ref().unwrap()["total_tokens"]
                .as_u64()
                .unwrap(),
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
        let payload = build_payload("m", &[], &SamplingParams::default(), true, None);
        assert!(payload["stream"].as_bool().unwrap());
        assert!(payload["stream_options"]["include_usage"]
            .as_bool()
            .unwrap());
    }

    #[test]
    fn build_payload_omits_stream_fields_when_not_streaming() {
        let payload = build_payload("m", &[], &SamplingParams::default(), false, None);
        assert!(payload.get("stream").is_none());
        assert!(payload.get("stream_options").is_none());
    }

    #[test]
    fn build_payload_embeds_tools_verbatim() {
        let tools = json!([{"type": "function", "function": {"name": "get_weather"}}]);
        let payload = build_payload("m", &[], &SamplingParams::default(), false, Some(&tools));
        assert_eq!(payload["tools"], tools);
        assert_eq!(
            payload["tools"][0]["function"]["name"].as_str(),
            Some("get_weather")
        );
    }

    #[test]
    fn build_payload_omits_tools_when_absent() {
        let payload = build_payload("m", &[], &SamplingParams::default(), false, None);
        assert!(payload.get("tools").is_none());
    }

    #[test]
    fn build_payload_embeds_reasoning_verbatim() {
        let reasoning = json!({"effort": "high", "exclude": true});
        let params = SamplingParams {
            reasoning: Some(reasoning.clone()),
            ..Default::default()
        };
        let payload = build_payload("m", &[], &params, false, None);
        assert_eq!(payload["reasoning"], reasoning);
    }

    #[test]
    fn build_payload_omits_reasoning_when_absent() {
        let payload = build_payload("m", &[], &SamplingParams::default(), false, None);
        assert!(payload.get("reasoning").is_none());
    }

    #[test]
    fn load_reasoning_file_parses_object_and_rejects_non_object() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("r.json");
        std::fs::write(&ok, r#"{"effort":"high"}"#).unwrap();
        assert_eq!(
            load_reasoning_file(&ok).unwrap()["effort"].as_str(),
            Some("high")
        );

        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, r#"["not","object"]"#).unwrap();
        let err = load_reasoning_file(&bad).unwrap_err();
        assert!(err.to_string().contains("JSON object"));
    }

    #[test]
    fn resolve_path_against_config_relative_and_absolute() {
        use std::path::PathBuf;
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "").unwrap();

        let rel = PathBuf::from("thinking.json");
        let resolved = resolve_path_against_config(Some(&cfg), rel.clone());
        assert_eq!(resolved, dir.path().join("thinking.json"));

        let abs = PathBuf::from("/abs/path.json");
        assert_eq!(resolve_path_against_config(Some(&cfg), abs.clone()), abs);

        let missing = dir.path().join("nope.toml");
        let rel2 = PathBuf::from("thinking.json");
        assert_eq!(
            resolve_path_against_config(Some(&missing), rel2.clone()),
            rel2
        );
        assert_eq!(resolve_path_against_config(None, rel2.clone()), rel2);
    }

    #[test]
    fn load_tools_file_parses_array() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tools.json");
        std::fs::write(&path, r#"[{"type":"function","function":{"name":"f"}}]"#).unwrap();
        let tools = load_tools_file(&path).unwrap();
        assert!(tools.as_array().unwrap().len() == 1);

        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, r#"{"not":"an array"}"#).unwrap();
        assert!(load_tools_file(&bad).is_err());
    }

    #[test]
    fn request_error_maps_to_exit_codes() {
        assert_eq!(
            request_exit_code(&RequestError::Client("boom".to_string())),
            2
        );
        assert_eq!(
            request_exit_code(&RequestError::Request("HTTP 400".to_string())),
            1
        );
        // A budget-gate refusal is its own class: the request never left the
        // machine, so it must not be confused with a provider error.
        assert_eq!(
            request_exit_code(&RequestError::Budget("over ceiling".to_string())),
            3
        );
    }

    #[test]
    fn base64_encode_standard_alphabet_with_padding() {
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello world"), "aGVsbG8gd29ybGQ=");
    }

    #[test]
    fn osc52_sequence_wraps_base64_payload() {
        assert_eq!(
            osc52_clipboard_sequence("hello world"),
            "\x1b]52;c;aGVsbG8gd29ybGQ=\x07"
        );
        // Empty input still produces a well-formed sequence.
        assert_eq!(osc52_clipboard_sequence(""), "\x1b]52;c;\x07");
    }

    #[test]
    fn extract_text_by_path_walks_segments() {
        let v = json!({"a": {"b": "hi"}});
        assert_eq!(extract_text_by_path(&v, "a.b"), Some("hi".to_string()));
        assert_eq!(extract_text_by_path(&v, "a"), None);
    }

    #[test]
    fn extract_text_by_path_rejects_missing_and_mistyped() {
        let v = json!({"a": {"b": 7}, "c": "", "d": null});
        assert_eq!(extract_text_by_path(&v, "a.zzz"), None);
        assert_eq!(extract_text_by_path(&v, "a.b"), None);
        assert_eq!(extract_text_by_path(&v, "c"), None);
        assert_eq!(extract_text_by_path(&v, "d"), None);
        assert_eq!(extract_text_by_path(&v, ""), None);
    }

    #[test]
    fn parse_response_captures_reasoning_from_named_field() {
        let raw = json!({
            "choices": [{"message": {"content": "answer", "reasoning": "because"}}]
        });
        let resp = parse_response_with(200, raw, &["choices.0.message.reasoning".to_string()]);
        assert_eq!(resp.assistant_content, Some("answer".to_string()));
        assert_eq!(resp.reasoning, Some("because".to_string()));
        assert_eq!(
            resp.reasoning_fields,
            vec!["choices.0.message.reasoning".to_string()]
        );
    }

    #[test]
    fn parse_response_without_fields_leaves_reasoning_none() {
        let raw = json!({"choices": [{"message": {"content": "answer"}}]});
        let resp = parse_response(200, raw);
        assert_eq!(resp.reasoning, None);
        assert!(resp.reasoning_fields.is_empty());
    }

    #[test]
    fn parse_response_joins_multiple_reasoning_fields_in_flag_order() {
        let raw = json!({
            "choices": [{"message": {"content": "a", "reasoning": "one"}}],
            "reasoning_content": "two"
        });
        let resp = parse_response_with(
            200,
            raw,
            &[
                "reasoning_content".to_string(),
                "choices.0.message.reasoning".to_string(),
            ],
        );
        assert_eq!(resp.reasoning, Some("two\n\none".to_string()));
    }

    #[test]
    fn parse_response_reasoning_absent_is_not_an_error() {
        let raw = json!({"choices": [{"message": {"content": "a"}}]});
        let resp = parse_response_with(200, raw, &["nope".to_string()]);
        assert_eq!(resp.reasoning, None);
        assert_eq!(resp.reasoning_fields, vec!["nope".to_string()]);
    }
}
