//! Integration tests for `request` driven through the real CLI binary
//! against a tiny local HTTP server, so no HTTP mocking crate is needed.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_llmhelper"))
}

/// Start a one-shot HTTP server on an ephemeral port. Returns the address
/// plus a handle to join after the client request completes, so the test can
/// assert on the request the server actually received.
fn start_server(
    status: &str,
    body: &str,
) -> (std::net::SocketAddr, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let status = status.to_string();
    let body = body.to_string();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut req = String::new();
        let mut buf = [0u8; 4096];
        let mut header_end: Option<usize> = None;
        loop {
            let n = match stream.read(&mut buf) {
                Ok(n) if n > 0 => n,
                _ => break,
            };
            req.push_str(&String::from_utf8_lossy(&buf[..n]));
            if header_end.is_none() {
                if let Some(pos) = req.find("\r\n\r\n") {
                    header_end = Some(pos + 4);
                }
            }
            if let Some(pos) = header_end {
                let Some(length) = content_length(&req[..pos]) else {
                    break;
                };
                if req.len() >= pos + length {
                    break;
                }
            }
        }
        let request_line = req.lines().next().unwrap_or("");
        let path = request_line.split_whitespace().nth(1).unwrap_or("");
        let (status_line, content_type, payload) = if path == "/v1/chat/completions" {
            (status.as_str(), "application/json", body.as_str())
        } else {
            ("200 OK", "text/html", HTML_FALLBACK_BODY)
        };
        let resp = format!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\n\r\n{}",
            status_line, content_type, payload
        );
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.flush();
        req
    });
    (addr, handle)
}

/// Parse `Content-Length` out of a raw HTTP header block.
fn content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        let Some(value) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
            .or_else(|| line.strip_prefix("CONTENT-LENGTH:"))
        else {
            continue;
        };
        return value.trim().parse::<usize>().ok();
    }
    None
}

const OK_BODY: &str = r#"{"id":"chatcmpl-test","model":"gpt-4","choices":[{"index":0,"message":{"role":"assistant","content":"Hello back"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;

/// SPA fallback served for any path other than `/v1/chat/completions`,
/// mirroring the real OpenAI-compatible frontend behaviour that made a
/// wrong endpoint path surface as 200 + HTML instead of a JSON body.
const HTML_FALLBACK_BODY: &str =
    "<!doctype html>\n<html lang=\"en\"><body>frontend</body></html>\n";

fn base_args(addr: &std::net::SocketAddr) -> Vec<String> {
    vec![
        "--base-url".into(),
        format!("http://{}", addr),
        "--api-key".into(),
        "sk-test-secret".into(),
        "--model".into(),
        "gpt-4".into(),
    ]
}

fn run(extra_args: &[&str]) -> Output {
    Command::new(bin())
        .args(["request"])
        .args(extra_args)
        .output()
        .expect("failed to run llmhelper")
}

fn run_with_args(args: &[String]) -> Output {
    let mut cmd = Command::new(bin());
    cmd.args(["request"]);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.output().expect("failed to run llmhelper")
}

fn body_of(req: &str) -> serde_json::Value {
    serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap()
}

#[test]
fn request_text_prints_assistant_content() {
    let (addr, handle) = start_server("200 OK", OK_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("Hello there".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Hello back\n");
    let received = body_of(&handle.join().unwrap());
    assert_eq!(received["model"].as_str().unwrap(), "gpt-4");
    assert_eq!(received["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        received["messages"][0]["content"].as_str().unwrap(),
        "Hello there"
    );
}

#[test]
fn request_json_prints_full_response_object() {
    let (addr, handle) = start_server(
        "200 OK",
        r#"{"id":"x","model":"gpt-4","choices":[{"message":{"content":"hi"}}],"usage":{"total_tokens":3}}"#,
    );
    let mut args = base_args(&addr);
    args.push("--json".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(output.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(parsed["id"].as_str().unwrap(), "x");
    assert_eq!(parsed["usage"]["total_tokens"].as_u64().unwrap(), 3);
    assert!(parsed.get("body").is_none());
    drop(handle);
}

#[test]
fn request_sampling_params_go_top_level() {
    let (addr, handle) = start_server("200 OK", r#"{"choices":[{"message":{"content":"ok"}}]}"#);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--temperature".into());
    args.push("0.25".into());
    args.push("--top-p".into());
    args.push("0.5".into());
    args.push("--max-tokens".into());
    args.push("42".into());
    args.push("--stop".into());
    args.push("<EOS>".into());
    args.push("--stop".into());
    args.push("<STOP2>".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let received = body_of(&handle.join().unwrap());
    let temp = received["temperature"].as_f64().unwrap();
    assert!((temp - 0.25).abs() < 0.001, "temperature was {}", temp);
    assert_eq!(received["top_p"].as_f64().unwrap(), 0.5);
    assert_eq!(received["max_tokens"].as_u64().unwrap(), 42);
    let stop = received["stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0].as_str().unwrap(), "<EOS>");
    assert!(received.get("generation_config").is_none());
}

#[test]
fn request_reads_messages_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("messages.json");
    std::fs::write(
        &path,
        r#"[{"role":"system","content":"be brief"},{"role":"user","content":"go"}]"#,
    )
    .unwrap();
    let (addr, handle) = start_server("200 OK", r#"{"choices":[{"message":{"content":"ok"}}]}"#);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--messages".into());
    args.push(path.to_str().unwrap().into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let received = body_of(&handle.join().unwrap());
    assert_eq!(received["messages"].as_array().unwrap().len(), 2);
    assert_eq!(received["messages"][0]["role"].as_str().unwrap(), "system");
}

#[test]
fn request_rejects_mutually_exclusive_flags() {
    let output = run(&["--json", "--text", "--base-url", "http://127.0.0.1:1"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mutually exclusive"));

    let output = run(&[
        "--messages",
        "/tmp/nope",
        "--prompt",
        "x",
        "--base-url",
        "http://127.0.0.1:1",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mutually exclusive"));
}

#[test]
fn request_requires_prompt_or_messages() {
    let output = run(&[
        "--base-url",
        "http://127.0.0.1:1",
        "--api-key",
        "k",
        "--model",
        "m",
        "--text",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--prompt or --messages"));
}

#[test]
fn request_errors_on_http_error_with_body_snippet() {
    let (addr, handle) = start_server("401 Unauthorized", r#"{"error":"invalid api key"}"#);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(!output.status.success());
    let req = handle.join().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("HTTP 401"));
    assert!(stderr.contains("invalid api key"));
    assert!(!stderr.contains("sk-test-secret"));
    assert!(req.contains("authorization: Bearer sk-test-secret"));
}

#[test]
fn request_errors_on_unreachable_endpoint() {
    let mut args = base_args(&"127.0.0.1:1".parse::<std::net::SocketAddr>().unwrap());
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to send request"));
}

/// Config file with a full `[request]` section. `Config::load` reads
/// `$XDG_CONFIG_HOME/llmhelper/config.toml` on Linux, so the test points that
/// env var at a temp dir to prove the config path is honoured.
#[test]
fn request_reads_config_file_defaults() {
    let cfg_dir = tempfile::tempdir().unwrap();
    let llm = cfg_dir.path().join("llmhelper");
    std::fs::create_dir_all(&llm).unwrap();
    std::fs::write(
        llm.join("config.toml"),
        r#"
[request]
api_key = "cfg-key"
default_model = "cfg-model"
timeout_seconds = 5
"#,
    )
    .unwrap();
    let (addr, handle) = start_server(
        "200 OK",
        r#"{"choices":[{"message":{"content":"from config"}}]}"#,
    );
    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg_dir.path())
        .args([
            "request",
            "--text",
            "--prompt",
            "hi",
            "--base-url",
            &format!("http://{}", addr),
        ])
        .output()
        .unwrap();
    let req = handle.join().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "from config\n");
    let received = body_of(&req);
    assert_eq!(received["model"].as_str().unwrap(), "cfg-model");
    assert!(req.contains("authorization: Bearer cfg-key"));
}

/// `[request] log_dir` sends `--log` lines to the configured directory
/// instead of the default `~/.config/llmhelper/logs`.
#[test]
fn request_log_dir_config_sends_lines_to_tempdir() {
    let log_dir = tempfile::tempdir().unwrap();
    let cfg_dir = tempfile::tempdir().unwrap();
    let llm = cfg_dir.path().join("llmhelper");
    std::fs::create_dir_all(&llm).unwrap();
    let log_dir_str = log_dir.path().display().to_string();
    std::fs::write(
        llm.join("config.toml"),
        format!(
            r#"
[request]
api_key = "cfg-key"
default_model = "cfg-model"
log_dir = "{}"
"#,
            log_dir_str
        ),
    )
    .unwrap();
    let (addr, handle) = start_server(
        "200 OK",
        r#"{"choices":[{"message":{"content":"ok"}}],"usage":{"total_tokens":1}}"#,
    );
    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg_dir.path())
        .args([
            "request",
            "--text",
            "--prompt",
            "hi",
            "--base-url",
            &format!("http://{}", addr),
            "--log",
        ])
        .output()
        .unwrap();
    let _ = handle.join().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files = std::fs::read_dir(log_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("request-"))
        .collect::<Vec<_>>();
    assert_eq!(
        files.len(),
        1,
        "one per-day log file expected in {}",
        log_dir_str
    );
    let content = std::fs::read_to_string(files[0].path()).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "one request + one response line, got {}",
        lines.len()
    );
    let entry: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(entry["direction"], "request");
    assert!(entry.get("timestamp").is_some());
    assert!(entry.get("body").is_some());
}

#[test]
fn request_posts_to_v1_chat_completions_endpoint() {
    let (addr, handle) = start_server("200 OK", OK_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let req = handle.join().unwrap();
    let request_line = req.lines().next().unwrap_or("");
    assert_eq!(request_line, "POST /v1/chat/completions HTTP/1.1");
}

#[test]
fn request_stream_text_prints_deltas() {
    let (addr, handle) = start_server(
        "200 OK",
        "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\ndata: [DONE]\n\n",
    );
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--stream".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello\n");
    let _req = handle.join().unwrap();
}

const STREAM_BODY: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\ndata: {\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2,\"total_tokens\":6}}\n\ndata: [DONE]\n\n";

#[test]
fn request_stream_json_emits_one_line_per_event() {
    let (addr, handle) = start_server("200 OK", STREAM_BODY);
    let mut args = base_args(&addr);
    args.push("--json".into());
    args.push("--stream".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "stdout was:\n{}", stdout);
    let events: Vec<serde_json::Value> = lines
        .iter()
        .map(|line| serde_json::from_str(line).expect("valid JSON line"))
        .collect();
    assert_eq!(
        events[0]["choices"][0]["delta"]["content"].as_str(),
        Some("he")
    );
    assert_eq!(
        events[1]["choices"][0]["delta"]["content"].as_str(),
        Some("llo")
    );
    assert!(events[0].get("usage").is_none());
    assert!(events[2].get("choices").is_none());
    let usage = &events[2]["usage"];
    assert_eq!(usage["prompt_tokens"].as_u64(), Some(4));
    assert_eq!(usage["completion_tokens"].as_u64(), Some(2));
    assert_eq!(usage["total_tokens"].as_u64(), Some(6));
    let _req = handle.join().unwrap();
}

#[test]
fn request_stream_payload_carries_stream_fields() {
    let (addr, handle) = start_server("200 OK", STREAM_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--stream".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let received = body_of(&handle.join().unwrap());
    assert_eq!(received["stream"].as_bool(), Some(true));
    assert_eq!(
        received["stream_options"]["include_usage"].as_bool(),
        Some(true)
    );
}

#[test]
fn request_one_shot_payload_omits_stream_fields() {
    let (addr, handle) = start_server("200 OK", OK_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let received = body_of(&handle.join().unwrap());
    assert!(received.get("stream").is_none());
    assert!(received.get("stream_options").is_none());
}

/// A `--tools` file containing a JSON array is embedded verbatim under the
/// payload's top-level `tools` key, with no client-side reshaping.
#[test]
fn request_tools_file_is_passed_through_verbatim() {
    let tools =
        r#"[{"type":"function","function":{"name":"get_weather","parameters":{"type":"object"}}}]"#;
    let dir = tempfile::tempdir().unwrap();
    let tools_path = dir.path().join("tools.json");
    std::fs::write(&tools_path, tools).unwrap();

    let (addr, handle) = start_server("200 OK", OK_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--tools".into());
    args.push(tools_path.to_string_lossy().into_owned());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let received = body_of(&handle.join().unwrap());
    assert_eq!(
        received["tools"],
        serde_json::from_str::<serde_json::Value>(tools).unwrap()
    );
}

/// Without `--tools` the `tools` key must be absent entirely, not `null`/`[]`.
#[test]
fn request_omits_tools_key_when_flag_absent() {
    let (addr, handle) = start_server("200 OK", OK_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let received = body_of(&handle.join().unwrap());
    assert!(received.get("tools").is_none());
}

/// A `--tools` file that is valid JSON but not an array is a usage error and
/// must exit 1 like the other CLI validation failures, without ever
/// contacting the endpoint.
#[test]
fn request_rejects_non_array_tools_file_with_exit_1() {
    let dir = tempfile::tempdir().unwrap();
    let tools_path = dir.path().join("tools.json");
    std::fs::write(&tools_path, r#"{"not":"an array"}"#).unwrap();

    let mut args = base_args(&"127.0.0.1:1".parse::<std::net::SocketAddr>().unwrap());
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--tools".into());
    args.push(tools_path.to_string_lossy().into_owned());
    let output = run_with_args(&args);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("JSON array"));
}

/// HTTP non-2xx is a request-level failure and maps to exit 1.
#[test]
fn request_http_error_exits_1() {
    let (addr, handle) = start_server("500 Internal Server Error", r#"{"error":"boom"}"#);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert_eq!(output.status.code(), Some(1));
    let _req = handle.join().unwrap();
}

/// A connection that cannot be established is a client-level failure and
/// maps to exit 2.
#[test]
fn request_connection_failure_exits_2() {
    let mut args = base_args(&"127.0.0.1:1".parse::<std::net::SocketAddr>().unwrap());
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("client error"));
}

const REASONING_BODY: &str = r#"{"id":"x","model":"gpt-4","choices":[{"message":{"role":"assistant","content":"the answer","reasoning":"because I said so"}}],"usage":{"total_tokens":9}}"#;

#[test]
fn request_reasoning_field_captured_into_text() {
    let (addr, handle) = start_server("200 OK", REASONING_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--reasoning-field".into());
    args.push("choices.0.message.reasoning".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("the answer\n"), "stdout: {}", stdout);
    assert!(stdout.contains("[reasoning]\nbecause I said so"));
    let _req = handle.join().unwrap();
}

#[test]
fn request_text_thinking_prints_reasoning_only() {
    let (addr, handle) = start_server("200 OK", REASONING_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--thinking".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--reasoning-field".into());
    args.push("choices.0.message.reasoning".into());
    let output = run_with_args(&args);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "because I said so\n"
    );
    let _req = handle.join().unwrap();
}

#[test]
fn request_reasoning_field_json_envelope() {
    let (addr, handle) = start_server("200 OK", REASONING_BODY);
    let mut args = base_args(&addr);
    args.push("--json".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--reasoning-field".into());
    args.push("choices.0.message.reasoning".into());
    let output = run_with_args(&args);
    assert!(output.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(parsed["reasoning"].as_str(), Some("because I said so"));
    assert_eq!(
        parsed["reasoning_fields"][0].as_str(),
        Some("choices.0.message.reasoning")
    );
    assert_eq!(parsed["response"]["id"].as_str(), Some("x"));
    let _req = handle.join().unwrap();
}

#[test]
fn request_without_reasoning_field_keeps_raw_json() {
    let (addr, handle) = start_server("200 OK", REASONING_BODY);
    let mut args = base_args(&addr);
    args.push("--json".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(output.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(parsed["id"].as_str(), Some("x"));
    assert!(parsed.get("response").is_none());
    assert!(parsed.get("reasoning").is_none());
    let _req = handle.join().unwrap();
}

/// A `[request] reasoning_fields` config key supplies the field list when the
/// flag is absent, so capture works without naming the field on the command
/// line. This exercises the resolution shared by every output path.
#[test]
fn request_reasoning_fields_config_key_enables_capture() {
    let cfg_dir = tempfile::tempdir().unwrap();
    let llm = cfg_dir.path().join("llmhelper");
    std::fs::create_dir_all(&llm).unwrap();
    std::fs::write(
        llm.join("config.toml"),
        "[request]\nreasoning_fields = [\"choices.0.message.reasoning\"]\n",
    )
    .unwrap();
    let (addr, handle) = start_server("200 OK", REASONING_BODY);
    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg_dir.path())
        .args([
            "request",
            "--json",
            "--prompt",
            "hi",
            "--model",
            "gpt-4",
            "--base-url",
            &format!("http://{}", addr),
        ])
        .output()
        .unwrap();
    let _req = handle.join().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(parsed["reasoning"].as_str(), Some("because I said so"));
    assert_eq!(
        parsed["reasoning_fields"][0].as_str(),
        Some("choices.0.message.reasoning")
    );
}

#[test]
fn request_reasoning_field_missing_is_empty_not_an_error() {
    let (addr, handle) = start_server("200 OK", OK_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--reasoning-field".into());
    args.push("does.not.exist".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Hello back\n");
    let _req = handle.join().unwrap();
}

/// A `--reasoning` file is embedded verbatim as the payload's `reasoning` key.
#[test]
fn request_reasoning_file_is_passed_through_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let reasoning_path = dir.path().join("reasoning.json");
    let body = r#"{"effort":"high","exclude":true}"#;
    std::fs::write(&reasoning_path, body).unwrap();

    let (addr, handle) = start_server("200 OK", OK_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--reasoning".into());
    args.push(reasoning_path.to_string_lossy().into_owned());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let received = body_of(&handle.join().unwrap());
    assert_eq!(
        received["reasoning"],
        serde_json::from_str::<serde_json::Value>(body).unwrap()
    );
}

#[test]
fn request_omits_reasoning_key_when_flag_absent() {
    let (addr, handle) = start_server("200 OK", OK_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    let output = run_with_args(&args);
    assert!(output.status.success());
    let received = body_of(&handle.join().unwrap());
    assert!(received.get("reasoning").is_none());
}

#[test]
fn request_rejects_non_object_reasoning_file_with_exit_1() {
    let dir = tempfile::tempdir().unwrap();
    let reasoning_path = dir.path().join("reasoning.json");
    std::fs::write(&reasoning_path, r#"["not","object"]"#).unwrap();

    let mut args = base_args(&"127.0.0.1:1".parse::<std::net::SocketAddr>().unwrap());
    args.push("--text".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--reasoning".into());
    args.push(reasoning_path.to_string_lossy().into_owned());
    let output = run_with_args(&args);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("JSON object"));
}

/// Interleaved reasoning + content SSE frames. Reasoning deltas use the
/// `delta.reasoning_content` field, matching the dotted-path capture the flag
/// accepts when applied to an event.
const REASONING_STREAM_BODY: &str = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"let me think\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"the answer\"}}]}\n\ndata: [DONE]\n\n";

#[test]
fn request_stream_json_annotates_channel() {
    let (addr, handle) = start_server("200 OK", REASONING_STREAM_BODY);
    let mut args = base_args(&addr);
    args.push("--json".into());
    args.push("--stream".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--reasoning-field".into());
    args.push("choices.0.delta.reasoning_content".into());
    let output = run_with_args(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["@channel"].as_str(), Some("reasoning"));
    assert_eq!(lines[1]["@channel"].as_str(), Some("content"));
    assert_eq!(
        lines[0]["choices"][0]["delta"]["reasoning_content"].as_str(),
        Some("let me think")
    );
    let _req = handle.join().unwrap();
}

#[test]
fn request_stream_text_splits_reasoning_to_stderr() {
    let (addr, handle) = start_server("200 OK", REASONING_STREAM_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--stream".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--reasoning-field".into());
    args.push("choices.0.delta.reasoning_content".into());
    let output = run_with_args(&args);
    assert!(output.status.success());
    // stdout carries only the answer; the thinking went to stderr.
    assert_eq!(String::from_utf8_lossy(&output.stdout), "the answer\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("let me think"));
    let _req = handle.join().unwrap();
}

#[test]
fn request_stream_text_thinking_prints_reasoning_only() {
    let (addr, handle) = start_server("200 OK", REASONING_STREAM_BODY);
    let mut args = base_args(&addr);
    args.push("--text".into());
    args.push("--thinking".into());
    args.push("--stream".into());
    args.push("--prompt".into());
    args.push("hi".into());
    args.push("--reasoning-field".into());
    args.push("choices.0.delta.reasoning_content".into());
    let output = run_with_args(&args);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "let me think\n");
    let _req = handle.join().unwrap();
}

/// Seed an OpenCode-schema database with one session costing `cost`, started
/// right now so it lands inside any `1d` budget window.
fn seed_opencode_db(path: &std::path::Path, cost: f64) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (
            id text PRIMARY KEY,
            project_id text NOT NULL,
            directory text NOT NULL,
            model text,
            agent text,
            title text NOT NULL DEFAULT '',
            cost real DEFAULT 0 NOT NULL,
            tokens_input integer DEFAULT 0 NOT NULL,
            tokens_output integer DEFAULT 0 NOT NULL,
            tokens_reasoning integer DEFAULT 0 NOT NULL,
            tokens_cache_read integer DEFAULT 0 NOT NULL,
            tokens_cache_write integer DEFAULT 0 NOT NULL,
            time_created integer NOT NULL,
            time_updated integer NOT NULL);",
    )
    .unwrap();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    conn.execute(
        "INSERT INTO session (id, project_id, directory, model, cost, time_created, time_updated)
         VALUES ('ses_gate', 'p', '/tmp/gate-test', 'gpt-4', ?1, ?2, ?2)",
        rusqlite::params![cost, now_ms],
    )
    .unwrap();
}

/// A temp config environment whose `[source.opencode]` points at a db seeded
/// with one session of the given cost. Returns the tempdir (keeps the config
/// and db alive for the test) so `XDG_CONFIG_HOME` can point at it.
fn gate_env(cost: f64, extra_config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("opencode.db");
    seed_opencode_db(&db, cost);
    let llm = dir.path().join("llmhelper");
    std::fs::create_dir_all(&llm).unwrap();
    std::fs::write(
        llm.join("config.toml"),
        format!(
            "[source.opencode]\ndb = [\"{}\"]\n{}",
            db.display(),
            extra_config
        ),
    )
    .unwrap();
    dir
}

/// The refusal is exit 3 — distinct from provider errors — names the budget,
/// source, window, spend and ceiling, and the unreachable `--base-url` proves
/// no provider was contacted: a leaked request would have exited 2.
#[test]
fn request_budget_gate_refuses_over_budget_with_exit_3() {
    let cfg = gate_env(12.5, "");
    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg.path())
        .args([
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "m",
            "--base-url",
            "http://127.0.0.1:1",
            "--budget",
            "opencode:5.00",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("budget gate"), "stderr: {}", stderr);
    assert!(stderr.contains("cli:opencode"), "stderr: {}", stderr);
    assert!(stderr.contains("source opencode"), "stderr: {}", stderr);
    assert!(stderr.contains("window 1d"), "stderr: {}", stderr);
    assert!(
        stderr.contains("measured 12.500000 over ceiling 5.000000"),
        "stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("no provider was contacted"),
        "stderr: {}",
        stderr
    );
}

/// A gate whose budgets are under its ceiling lets the request through
/// untouched: the provider is contacted and the response prints normally.
#[test]
fn request_budget_gate_passes_under_budget_and_sends() {
    let cfg = gate_env(1.0, "");
    let (addr, handle) = start_server("200 OK", OK_BODY);
    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg.path())
        .args([
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "sk-test-secret",
            "--model",
            "gpt-4",
            "--base-url",
            &format!("http://{}", addr),
            "--budget",
            "opencode:5.00",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Hello back\n");
    let _req = handle.join().unwrap();
}

/// With no budget flag the gate is off even when a configured source is over
/// its ceiling: `request` must behave exactly as before.
#[test]
fn request_budget_gate_off_without_flags() {
    let cfg = gate_env(12.5, "");
    let (addr, handle) = start_server("200 OK", OK_BODY);
    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg.path())
        .args([
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "sk-test-secret",
            "--model",
            "gpt-4",
            "--base-url",
            &format!("http://{}", addr),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _req = handle.join().unwrap();
}

/// `--budget-name daily` gates on the configured `[budget.daily]` entry: the
/// refusal names it, with the configured window and ceiling.
#[test]
fn request_budget_name_gates_on_configured_budget() {
    let cfg = gate_env(
        12.5,
        "[budget.daily]\nsource = \"opencode\"\nwindow = \"1d\"\nmax_cost = 5.0\n",
    );
    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg.path())
        .args([
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "m",
            "--base-url",
            "http://127.0.0.1:1",
            "--budget-name",
            "daily",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("daily (source opencode, window 1d)"),
        "stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("measured 12.500000 over ceiling 5.000000"),
        "stderr: {}",
        stderr
    );
}

/// Every over budget is named, not just the first: two one-offs on the same
/// source both breach, and the message counts them.
#[test]
fn request_budget_gate_names_every_offender() {
    let cfg = gate_env(12.5, "");
    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg.path())
        .args([
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "m",
            "--base-url",
            "http://127.0.0.1:1",
            "--budget",
            "opencode:5.00",
            "--budget",
            "opencode:10.00",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("2 budgets are already over their ceilings"),
        "stderr: {}",
        stderr
    );
    assert!(stderr.contains("ceiling 5.000000"), "stderr: {}", stderr);
    assert!(stderr.contains("ceiling 10.000000"), "stderr: {}", stderr);
}

/// A malformed `--budget` spec is a usage error (exit 1), not a gate refusal
/// (exit 3): the request never got a chance to be measured.
#[test]
fn request_bad_budget_spec_exits_1() {
    let cfg = gate_env(12.5, "");
    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg.path())
        .args([
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "m",
            "--base-url",
            "http://127.0.0.1:1",
            "--budget",
            "opencode",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected <source>:<amount>"),
        "stderr: {}",
        stderr
    );
}

/// An unknown `--budget-name` lists the configured names instead of failing
/// silently — a gate that measured nothing would pretend to protect the user.
#[test]
fn request_unknown_budget_name_lists_configured_names() {
    let cfg = gate_env(
        1.0,
        "[budget.daily]\nsource = \"opencode\"\nwindow = \"1d\"\nmax_cost = 5.0\n",
    );
    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg.path())
        .args([
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "m",
            "--base-url",
            "http://127.0.0.1:1",
            "--budget-name",
            "nope",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown --budget-name 'nope'"),
        "stderr: {}",
        stderr
    );
    assert!(stderr.contains("daily"), "stderr: {}", stderr);
}

// --- the llmhelper Source and the cost-control closed loop (spec 0027) ---

/// A response big enough that the `PRICE` rates put its cost well past the
/// tiny ceilings the refusals are asserted on.
const LOOP_BODY: &str = r#"{"id":"chatcmpl-loop","model":"gpt-4","choices":[{"index":0,"message":{"role":"assistant","content":"loop"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100000,"completion_tokens":50000,"total_tokens":150000}}"#;

/// The priced half of the loop: gpt-4 at 2.0/10.0 per Mtoken makes the
/// `LOOP_BODY` turn cost exactly 0.7.
const PRICE: &str = "[price.gpt-4]\ninput_per_mtoken = 2.0\noutput_per_mtoken = 10.0\n";

/// A config environment for the loop: `[request] log_dir` points at a fresh
/// `logs` dir (created), every other Source points at a nonexistent path
/// inside the temp env so no real user data is read, and `extra_config` is
/// appended verbatim. The log dir is `<dir>/logs`.
fn loop_env(extra_config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let llm = dir.path().join("llmhelper");
    std::fs::create_dir_all(&llm).unwrap();
    std::fs::write(
        llm.join("config.toml"),
        format!(
            "[source.claude]\ndir = \"{}\"\n\
             [source.opencode]\ndb = [\"{}\"]\n\
             [source.omp]\ndir = \"{}\"\n\
             [source.kilo]\ndb = [\"{}\"]\n\
             [request]\nlog_dir = \"{}\"\n{}",
            dir.path().join("absent-claude").display(),
            dir.path().join("absent-opencode.db").display(),
            dir.path().join("absent-omp").display(),
            dir.path().join("absent-kilo.db").display(),
            logs.display(),
            extra_config,
        ),
    )
    .unwrap();
    dir
}

/// Hand-write one request/response pair into the env's log dir with the given
/// usage, so a test can have spend on the books without a server round-trip.
fn seed_llmhelper_log(cfg: &tempfile::TempDir, prompt: u64, completion: u64) {
    seed_llmhelper_log_at(&cfg.path().join("logs"), prompt, completion);
}

fn seed_llmhelper_log_at(logs: &std::path::Path, prompt: u64, completion: u64) {
    std::fs::create_dir_all(logs).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let request = serde_json::json!({"model": "gpt-4", "messages": []}).to_string();
    let response = serde_json::json!({
        "model": "gpt-4",
        "choices": [{"message": {"content": "ok"}}],
        "usage": {"prompt_tokens": prompt, "completion_tokens": completion},
    })
    .to_string();
    let line = |direction: &str, body: &str| {
        serde_json::json!({"timestamp": now, "direction": direction, "body": body}).to_string()
    };
    let path = logs.join(format!(
        "request-{}.log",
        chrono::Utc::now().format("%Y-%m-%d")
    ));
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n",
            line("request", &request),
            line("response", &response)
        ),
    )
    .unwrap();
}

fn request_in(cfg: &tempfile::TempDir, args: &[&str]) -> Output {
    Command::new(bin())
        .env("XDG_CONFIG_HOME", cfg.path())
        .args(args)
        .output()
        .expect("failed to run llmhelper")
}

fn usage_llmhelper_json(cfg: &tempfile::TempDir) -> serde_json::Value {
    let output = request_in(cfg, &["usage", "--json", "--source", "llmhelper"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The whole loop, end to end: spend (a real `--log` request against the test
/// server) → measure (the log read back as the `llmhelper` Source) → gate (the
/// next request refused on that measured spend) → pass (a ceiling above it).
#[test]
fn the_loop_closes_measure_gate_spend_log_remeasure() {
    let cfg = loop_env(PRICE);

    // spend + log
    let (addr, handle) = start_server("200 OK", LOOP_BODY);
    let output = request_in(
        &cfg,
        &[
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "gpt-4",
            "--base-url",
            &format!("http://{}", addr),
            "--log",
        ],
    );
    let _req = handle.join().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // measure: the log is now a Source carrying the provider's token counts
    let json = usage_llmhelper_json(&cfg);
    let group = &json["groups"][0];
    assert_eq!(group["source"], "llmhelper");
    assert_eq!(group["tokens"]["input"], 100000);
    assert_eq!(group["tokens"]["output"], 50000);
    assert_eq!(group["tokens"]["cache_read"], 0);
    assert!((group["cost"].as_f64().unwrap() - 0.7).abs() < 1e-9);

    // gate: the *next* request is refused on the *previous* one's logged
    // spend. The unreachable base-url proves a refusal contacts no provider.
    let output = request_in(
        &cfg,
        &[
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "gpt-4",
            "--base-url",
            "http://127.0.0.1:1",
            "--budget",
            "llmhelper:0.01",
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cli:llmhelper (source llmhelper, window 1d)"),
        "stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("measured 0.700000 over ceiling 0.010000"),
        "stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("no provider was contacted"),
        "stderr: {}",
        stderr
    );

    // a ceiling above the spend lets the loop continue: provider contacted
    let (addr, handle) = start_server("200 OK", LOOP_BODY);
    let output = request_in(
        &cfg,
        &[
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "gpt-4",
            "--base-url",
            &format!("http://{}", addr),
            "--log",
            "--budget",
            "llmhelper:1000.00",
        ],
    );
    let _req = handle.join().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "loop\n");
}

/// `[budget.mine] source = "llmhelper"` reached via `--budget-name` refuses
/// identically to a one-off: the log Source is a budget target like any other.
#[test]
fn configured_llmhelper_budget_refuses_via_budget_name() {
    let cfg = loop_env(&format!(
        "{PRICE}[budget.mine]\nsource = \"llmhelper\"\nwindow = \"1d\"\nmax_cost = 0.01\n"
    ));
    seed_llmhelper_log(&cfg, 100000, 50000);
    let output = request_in(
        &cfg,
        &[
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "m",
            "--base-url",
            "http://127.0.0.1:1",
            "--budget-name",
            "mine",
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mine (source llmhelper, window 1d)"),
        "stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("measured 0.700000 over ceiling 0.010000"),
        "stderr: {}",
        stderr
    );
}

/// Absent pricing is not free: with no `[price]` entry the Source reports
/// `cost: null`, and a budget on it is `not measured` — which cannot be `Over`,
/// so the gate lets the request through (the unreachable provider then makes
/// it exit 2, proving the refusal did not happen).
#[test]
fn unpriced_model_is_not_measured_so_the_gate_cannot_refuse() {
    let cfg = loop_env("");
    seed_llmhelper_log(&cfg, 100000, 50000);
    let json = usage_llmhelper_json(&cfg);
    assert_eq!(json["groups"][0]["cost"], serde_json::Value::Null);
    assert_eq!(json["groups"][0]["tokens"]["input"], 100000);

    let output = request_in(
        &cfg,
        &[
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "m",
            "--base-url",
            "http://127.0.0.1:1",
            "--budget",
            "llmhelper:0.01",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("budget gate"),
        "gate must not refuse unmeasured spend; stderr: {}",
        stderr
    );
}

/// A gate naming `llmhelper` while `--log` is off warns on stderr — the gate
/// cannot see this run's own spend — and still proceeds.
#[test]
fn llmhelper_budget_without_log_warns_and_proceeds() {
    let cfg = loop_env(PRICE);
    seed_llmhelper_log(&cfg, 100000, 50000);
    let (addr, handle) = start_server("200 OK", LOOP_BODY);
    let output = request_in(
        &cfg,
        &[
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "gpt-4",
            "--base-url",
            &format!("http://{}", addr),
            "--budget",
            "llmhelper:1000.00",
        ],
    );
    let _req = handle.join().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot see unlogged requests"),
        "stderr: {}",
        stderr
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "loop\n");
}

/// An `llmhelper` budget with no log dir at all yields no records rather than
/// an error: absent data is not zero, and the gate is simply satisfiable.
#[test]
fn llmhelper_row_appears_only_when_the_log_dir_exists() {
    let cfg = loop_env("");
    std::fs::remove_dir(cfg.path().join("logs")).unwrap();
    let json = usage_llmhelper_json(&cfg);
    let names: Vec<&str> = json["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&"llmhelper"),
        "a never-logging user sees no phantom row: {:?}",
        names
    );

    std::fs::create_dir_all(cfg.path().join("logs")).unwrap();
    let json = usage_llmhelper_json(&cfg);
    let names: Vec<&str> = json["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"llmhelper"), "sources: {:?}", names);
}

/// A truncated log line is skipped with a warning, never fatal: the
/// measure-half of the loop must survive a crash in the spend-half.
#[test]
fn a_malformed_log_line_warns_and_the_good_lines_still_measure() {
    let cfg = loop_env(PRICE);
    let logs = cfg.path().join("logs");
    let good = {
        let now = chrono::Utc::now().to_rfc3339();
        let body = serde_json::json!({
            "model": "gpt-4",
            "usage": {"prompt_tokens": 100000, "completion_tokens": 50000},
        })
        .to_string();
        serde_json::json!({"timestamp": now, "direction": "response", "body": body}).to_string()
    };
    std::fs::write(
        logs.join(format!(
            "request-{}.log",
            chrono::Utc::now().format("%Y-%m-%d")
        )),
        format!("{{\"timestamp\": \"truncat\n{}\n", good),
    )
    .unwrap();
    let output = request_in(&cfg, &["usage", "--json", "--source", "llmhelper"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["groups"][0]["tokens"]["input"], 100000);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("1 malformed line(s) skipped"),
        "stderr: {}",
        stderr
    );
}

/// Gating on one Source reads only that Source: an over-budget `opencode`
/// check must not even open the request log, so a corrupt log dir neither
/// warns nor slows the gate.
#[test]
fn gate_on_another_source_never_reads_the_log_dir() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("opencode.db");
    seed_opencode_db(&db, 12.5);
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("request-2026-09-22.log"), "not json at all\n").unwrap();
    let llm = dir.path().join("llmhelper");
    std::fs::create_dir_all(&llm).unwrap();
    std::fs::write(
        llm.join("config.toml"),
        format!(
            "[source.opencode]\ndb = [\"{}\"]\n[request]\nlog_dir = \"{}\"\n",
            db.display(),
            logs.display()
        ),
    )
    .unwrap();

    let output = Command::new(bin())
        .env("XDG_CONFIG_HOME", dir.path())
        .args([
            "request",
            "--text",
            "--prompt",
            "hi",
            "--api-key",
            "k",
            "--model",
            "m",
            "--base-url",
            "http://127.0.0.1:1",
            "--budget",
            "opencode:5.00",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("source opencode"), "stderr: {}", stderr);
    assert!(
        !stderr.contains("malformed"),
        "the log dir must not have been read; stderr: {}",
        stderr
    );
}
