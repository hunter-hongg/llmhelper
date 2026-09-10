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
