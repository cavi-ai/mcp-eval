use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn free_address() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().to_string()
}

fn read_http_body(stream: &TcpStream) -> Value {
    let mut reader = BufReader::new(stream);
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
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn drain_http_body(stream: &TcpStream) {
    let mut reader = BufReader::new(stream);
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
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
}

fn upstream_fixture() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_http_body(&stream);
        let response = json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {"content": [{"type": "text", "text": "PRIVATE_RESPONSE_CANARY"}]}
        });
        let body = response.to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: fixture-session\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (url, handle)
}

fn broken_upstream_fixture() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        drain_http_body(&stream);
        let body = "PRIVATE_BROKEN_RESPONSE";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (url, handle)
}

/// A running `mcpeval shim-http`, killed when dropped so a failing
/// assertion never leaks the process.
struct Proxy(Child);

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_proxy(upstream: &str, home: &std::path::Path) -> (Proxy, String) {
    let listen = free_address();
    let mut proxy = Proxy(
        Command::new(bin())
            .args([
                "shim-http",
                "--server",
                "fixture",
                "--listen",
                &listen,
                "--upstream",
                upstream,
            ])
            .env("MCPEVAL_HOME", home)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    wait_for_proxy(&listen, &mut proxy.0);
    (proxy, listen)
}

/// One POST carrying exactly `headers` (each line CRLF-terminated) plus
/// Content-Length; returns the status code.
fn status_with_headers(listen: &str, headers: &str, body: &[u8]) -> u16 {
    let mut stream = TcpStream::connect(listen).unwrap();
    write!(
        stream,
        "POST /mcp HTTP/1.1\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();
    let mut status_line = String::new();
    BufReader::new(stream).read_line(&mut status_line).unwrap();
    status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

fn wait_for_proxy(address: &str, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("proxy exited before listening: {status}");
        }
        assert!(Instant::now() < deadline, "proxy did not start");
        thread::sleep(Duration::from_millis(20));
        if TcpListener::bind(address).is_err() {
            return;
        }
    }
}

#[test]
fn http_shim_forwards_and_persists_only_sanitized_call_data() {
    let (upstream, fixture) = upstream_fixture();
    let home = std::env::temp_dir().join(format!("mcpeval-shim-http-{}", uuid::Uuid::new_v4()));
    let (proxy, listen) = start_proxy(&upstream, &home);

    let request = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {"name": "lookup", "arguments": {"secret": "PRIVATE_REQUEST_CANARY"}}
    });
    let body = request.to_string();
    let mut client = TcpStream::connect(&listen).unwrap();
    write!(
        client,
        "POST /mcp HTTP/1.1\r\nHost: {listen}\r\nOrigin: http://localhost:6274\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let response = read_http_body(&client);
    assert_eq!(response["id"], 7);
    assert_eq!(
        response["result"]["content"][0]["text"],
        "PRIVATE_RESPONSE_CANARY"
    );

    fixture.join().unwrap();
    drop(proxy);

    let stored = std::fs::read_dir(home.join("store"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(stored.contains("\"method\":\"tools/call\""));
    assert!(stored.contains("\"tool\":\"lookup\""));
    assert!(!stored.contains("PRIVATE_REQUEST_CANARY"));
    assert!(!stored.contains("PRIVATE_RESPONSE_CANARY"));
    assert!(!stored.contains("fixture-session"));
    assert!(!stored.contains(&upstream));
}

/// A web page can POST to the loopback listener directly or through a
/// rebound DNS name. Such requests are refused before anything reaches
/// the upstream.
#[test]
fn http_shim_rejects_requests_a_browser_page_can_forge() {
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    upstream.set_nonblocking(true).unwrap();
    let upstream_url = format!("http://{}/mcp", upstream.local_addr().unwrap());
    let home = std::env::temp_dir().join(format!("mcpeval-shim-http-{}", uuid::Uuid::new_v4()));
    let (_proxy, listen) = start_proxy(&upstream_url, &home);
    let port = listen.rsplit(':').next().unwrap();
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let host = format!("Host: {listen}\r\n");
    let json_type = "Content-Type: application/json\r\n";
    let cases: Vec<(String, u16)> = vec![
        (
            format!("{host}{json_type}Origin: https://evil.example\r\n"),
            403,
        ),
        (format!("{host}{json_type}Origin: null\r\n"), 403),
        (format!("Host: evil.example:{port}\r\n{json_type}"), 403),
        (json_type.to_owned(), 400),
        (
            format!("{host}Host: evil.example:{port}\r\n{json_type}"),
            400,
        ),
        (format!("{host}Content-Type: text/plain\r\n"), 415),
        (host.clone(), 415),
    ];
    for (headers, expected) in cases {
        assert_eq!(
            status_with_headers(&listen, &headers, body),
            expected,
            "{headers:?}"
        );
    }
    assert_eq!(
        upstream.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "a rejected request reached the upstream"
    );
}

#[test]
fn http_shim_rejects_non_loopback_listeners() {
    let output = Command::new(bin())
        .args([
            "shim-http",
            "--server",
            "fixture",
            "--listen",
            "0.0.0.0:8080",
            "--upstream",
            "http://127.0.0.1:9999/mcp",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("loopback"));
    assert!(!stderr.contains("127.0.0.1:9999"));
}

#[test]
fn http_shim_records_content_free_markers_for_broken_json() {
    let (upstream, fixture) = broken_upstream_fixture();
    let home = std::env::temp_dir().join(format!("mcpeval-shim-http-{}", uuid::Uuid::new_v4()));
    let (proxy, listen) = start_proxy(&upstream, &home);

    let body = "PRIVATE_BROKEN_REQUEST";
    let mut client = TcpStream::connect(&listen).unwrap();
    write!(
        client,
        "POST /mcp HTTP/1.1\r\nHost: {listen}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert!(response.contains("PRIVATE_BROKEN_RESPONSE"));

    fixture.join().unwrap();
    drop(proxy);

    let stored = std::fs::read_dir(home.join("store"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(stored.contains("\"method\":\"unparsed/outbound\""));
    assert!(stored.contains("\"method\":\"unparsed/inbound\""));
    assert!(!stored.contains("PRIVATE_BROKEN_REQUEST"));
    assert!(!stored.contains("PRIVATE_BROKEN_RESPONSE"));
}
