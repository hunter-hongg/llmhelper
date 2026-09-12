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
