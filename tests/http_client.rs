use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use mcpeval::http_client::HttpMcpClient;
use mcpeval::mcp_client::ToolResponse;
use serde_json::{json, Value};

fn read_request(stream: &TcpStream) -> (Vec<String>, Value) {
    let mut reader = BufReader::new(stream);
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap();
        }
        headers.push(line.trim().to_owned());
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
    (headers, serde_json::from_slice(&body).unwrap())
}

fn serve(stream: &mut TcpStream, sse: bool, session_required: bool) {
    let (headers, request) = read_request(stream);
    assert!(headers
        .iter()
        .any(|line| line.eq_ignore_ascii_case("accept: application/json, text/event-stream")));
    assert!(headers
        .iter()
        .any(|line| line.eq_ignore_ascii_case("mcp-protocol-version: 2025-06-18")));
    if session_required {
        assert!(headers
            .iter()
            .any(|line| line.eq_ignore_ascii_case("mcp-session-id: fixture-session")));
    }
    if request.get("id").is_none() {
        stream
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        return;
    }
    let id = request["id"].as_u64().unwrap();
    let result = match request["method"].as_str().unwrap() {
        "initialize" => json!({
            "protocolVersion":"2025-06-18","capabilities":{"tools":{}},
            "serverInfo":{"name":"fixture","version":"1"}
        }),
        "tools/list" => json!({"tools":[{
            "name":"read_counter","inputSchema":{"type":"object","properties":{}}
        }]}),
        "tools/call" => json!({"status":"ready","content":[{"type":"text","text":"CANARY raw"}]}),
        _ => json!({}),
    };
    let response = json!({"jsonrpc":"2.0","id":id,"result":result});
    let body = if sse {
        format!("event: message\ndata: {response}\n\n")
    } else {
        response.to_string()
    };
    let content_type = if sse {
        "text/event-stream"
    } else {
        "application/json"
    };
    let session = if request["method"] == "initialize" {
        "Mcp-Session-Id: fixture-session\r\n"
    } else {
        ""
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{session}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}

fn fixture(sse: bool) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        for index in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            serve(&mut stream, sse, index > 0);
        }
    });
    (endpoint, handle)
}

fn exercise(sse: bool) {
    let (endpoint, server) = fixture(sse);
    let mut client = HttpMcpClient::connect(&endpoint, false).unwrap();
    client.initialize().unwrap();
    assert_eq!(client.list_tools().unwrap(), vec!["read_counter"]);
    match client.call_tool("read_counter", &json!({})).unwrap() {
        ToolResponse::Success(value) => assert_eq!(value["status"], "ready"),
        ToolResponse::Error { .. } => panic!("fixture returned an error"),
    }
    server.join().unwrap();
}

#[test]
fn streamable_http_supports_json_and_sse_responses() {
    exercise(false);
    exercise(true);
}

#[test]
fn endpoint_policy_is_local_and_credential_free_by_default() {
    assert!(HttpMcpClient::connect("http://127.0.0.1:1/mcp", false).is_ok());
    assert!(HttpMcpClient::connect("http://example.com/mcp", true).is_err());
    assert!(HttpMcpClient::connect("https://example.com/mcp", false).is_err());
    assert!(HttpMcpClient::connect("https://example.com/mcp", true).is_ok());
    assert!(HttpMcpClient::connect("https://user:secret@example.com/mcp", true).is_err());
    assert!(HttpMcpClient::connect("https://example.com/mcp?token=secret", true).is_err());
}
