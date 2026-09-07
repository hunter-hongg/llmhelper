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
    let endpoint = format!("{}/chat/completions", settings.base_url);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_msg(content: &str) -> Value {
        json!({"role": "user", "content": content})
    }

    #[test]
    fn build_payload_includes_model_and_messages() {
        let payload = build_payload("gpt-4", &[user_msg("Hello")], None, None, None, &[]);
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
        let payload = build_payload("gpt-4", &[user_msg("Hello")], None, None, None, &[]);
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
}
