//! End-to-end tests for custom HTTP tools.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use common::{crabot_workspace, run_tool};
use crabot::tools::custom::{
    CustomTool, HttpHeader, HttpMethod, HttpRequest, ParameterType, ToolKind, ToolParameter,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// One captured HTTP request, asserted after the tool call finishes.
struct CapturedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// Serve exactly one request from a local port.
fn spawn_server(
    status: &'static str,
    response_body: &'static str,
) -> (String, mpsc::Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());

        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();

        let mut headers = Vec::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                let value = value.trim().to_string();
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.parse().unwrap_or(0);
                }
                headers.push((name.to_string(), value));
            }
        }

        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        let _ = tx.send(CapturedRequest {
            method,
            path,
            headers,
            body: String::from_utf8(body).unwrap(),
        });

        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });
    (format!("http://{address}"), rx)
}

fn http_tool(url: String, body: String, response_template: String) -> CustomTool {
    CustomTool {
        name: "web_search".into(),
        description: "Search the web".into(),
        instruction: "Search the web".into(),
        parameters: vec![
            ToolParameter {
                name: "query".into(),
                kind: ParameterType::String,
                description: "Search query".into(),
                required: true,
            },
            ToolParameter {
                name: "count".into(),
                kind: ParameterType::Integer,
                description: "Result count".into(),
                required: false,
            },
        ],
        kind: ToolKind::Http(HttpRequest {
            method: HttpMethod::Post,
            url,
            headers: vec![HttpHeader {
                name: "Authorization".into(),
                value: "Bearer test-key".into(),
            }],
            body,
            response_template,
        }),
    }
}

#[test]
fn http_tool_posts_typed_json_and_renders_results() {
    crabot::setup::install_crypto_provider();
    crabot::tools::configure_proxy(false, false);

    let (base_url, rx) = spawn_server(
        "200 OK",
        r#"{"results":[
            {"title":"A","url":"https://a.example","summary":"first"},
            {"title":"B","url":"https://b.example","summary":"second"}
        ]}"#,
    );
    let tool = http_tool(
        format!("{base_url}/search?q={{query}}"),
        r#"{"query": "{query}", "count": "{count}"}"#.into(),
        r#"{{ for r in response.results }}{r.title} — {r.url}
{r.summary}

{{ endfor }}"#
            .into(),
    );

    let output = run_tool(
        Arc::new(tool),
        json!({"query": "a b", "count": 2}),
        &crabot_workspace(),
        CancellationToken::new(),
    )
    .unwrap();
    assert!(output.contains("A — https://a.example"), "{output}");
    assert!(output.contains("first"), "{output}");
    assert!(output.contains("B — https://b.example"), "{output}");

    let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/search?q=a%20b");
    assert!(
        request
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                && value == "Bearer test-key"),
        "{:?}",
        request.headers
    );
    assert!(
        request
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("content-type")
                && value == "application/json"),
        "{:?}",
        request.headers
    );
    let sent: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(sent["query"], json!("a b"));
    assert_eq!(sent["count"], json!(2));
}

#[test]
fn http_tool_reports_api_error_message() {
    crabot::setup::install_crypto_provider();
    crabot::tools::configure_proxy(false, false);

    let (base_url, _rx) = spawn_server(
        "400 Bad Request",
        r#"{"error":{"message":"'count' must be between 1 and 50"}}"#,
    );
    let tool = http_tool(
        format!("{base_url}/search"),
        r#"{"query": "{query}"}"#.into(),
        String::new(),
    );

    let error = run_tool(
        Arc::new(tool),
        json!({"query": "x", "count": 99}),
        &crabot_workspace(),
        CancellationToken::new(),
    )
    .unwrap_err();
    assert!(error.contains("400"), "{error}");
    assert!(
        error.contains("'count' must be between 1 and 50"),
        "{error}"
    );
}
