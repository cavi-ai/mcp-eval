use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use serde_json::Value;

use crate::correlate::Correlator;
use crate::fingerprint::Salt;
use crate::store::Store;

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const WORKER_COUNT: usize = 8;
const REQUEST_QUEUE_CAPACITY: usize = 32;
const FORWARDED_REQUEST_HEADERS: [&str; 7] = [
    "accept",
    "content-type",
    "mcp-protocol-version",
    "mcp-session-id",
    "last-event-id",
    "authorization",
    "origin",
];
const FORWARDED_RESPONSE_HEADERS: [&str; 3] =
    ["content-type", "mcp-protocol-version", "mcp-session-id"];

pub fn run(
    server: String,
    listen: String,
    upstream: String,
    allow_remote: bool,
) -> anyhow::Result<()> {
    if !crate::privacy::valid_server(&server) {
        bail!("server must be a valid privacy-safe label");
    }
    let address: SocketAddr = listen.parse().context("listen address is invalid")?;
    if !address.ip().is_loopback() {
        bail!("HTTP capture listeners must use a loopback address");
    }
    let upstream = crate::http_client::validate_endpoint(&upstream, allow_remote)?;
    let listener = TcpListener::bind(address).context("binding HTTP listener failed")?;
    let store = Store::open(None).context("opening recording store")?;
    let salt = Salt::load(store.root()).context("loading fingerprint salt")?;
    let session =
        std::env::var("MCPEVAL_SESSION").unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
    let state = Arc::new(Mutex::new(CaptureState {
        correlator: Correlator::new(server, session, salt),
        store,
    }));
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(IO_TIMEOUT)
        .timeout_read(IO_TIMEOUT)
        .timeout_write(IO_TIMEOUT)
        .build();
    let (sender, receiver) = mpsc::sync_channel::<TcpStream>(REQUEST_QUEUE_CAPACITY);
    let receiver = Arc::new(Mutex::new(receiver));
    for _ in 0..WORKER_COUNT {
        let agent = agent.clone();
        let receiver = Arc::clone(&receiver);
        let upstream = upstream.clone();
        let state = Arc::clone(&state);
        thread::spawn(move || loop {
            let stream = {
                let Ok(receiver) = receiver.lock() else {
                    return;
                };
                let Ok(stream) = receiver.recv() else {
                    return;
                };
                stream
            };
            let _ = forward(stream, &agent, &upstream, &state);
        });
    }
    for connection in listener.incoming() {
        let stream = connection.context("accepting HTTP connection failed")?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        match sender.try_send(stream) {
            Ok(()) => {}
            Err(TrySendError::Full(mut stream)) => {
                let _ = write_response(&mut stream, 503, &[], &[]);
            }
            Err(TrySendError::Disconnected(_)) => bail!("HTTP workers stopped"),
        }
    }
    Ok(())
}

struct CaptureState {
    correlator: Correlator,
    store: Store,
}

struct IncomingRequest {
    method: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn forward(
    mut stream: TcpStream,
    agent: &ureq::Agent,
    upstream: &str,
    state: &Mutex<CaptureState>,
) -> anyhow::Result<()> {
    let incoming = match read_request(&mut stream) {
        Ok(request) => request,
        Err(_) => {
            let _ = write_response(&mut stream, 400, &[], &[]);
            return Ok(());
        }
    };
    let started_ms = now_ms();
    let request_scope = uuid::Uuid::new_v4().to_string();
    let request_value = serde_json::from_slice::<Value>(&incoming.body).ok();
    {
        let mut capture = state
            .lock()
            .map_err(|_| anyhow::anyhow!("capture state failed"))?;
        if let Some(value) = &request_value {
            capture
                .correlator
                .on_outbound_scoped(value, started_ms, &request_scope);
        } else {
            let record = capture.correlator.on_unparsed("outbound", started_ms);
            capture.store.append(&record)?;
        }
    }
    let mut outgoing = agent.request(&incoming.method, upstream);
    for (name, value) in &incoming.headers {
        if FORWARDED_REQUEST_HEADERS
            .iter()
            .any(|allowed| name.eq_ignore_ascii_case(allowed))
        {
            outgoing = outgoing.set(name, value);
        }
    }
    let upstream_response = match outgoing.send_bytes(&incoming.body) {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(_) => {
            write_response(&mut stream, 502, &[], &[])?;
            return Ok(());
        }
    };
    let status = upstream_response.status();
    let headers = FORWARDED_RESPONSE_HEADERS
        .iter()
        .filter_map(|name| {
            upstream_response
                .header(name)
                .map(|value| ((*name).to_owned(), value.to_owned()))
        })
        .collect::<Vec<_>>();
    let body = read_bounded(upstream_response.into_reader())?;
    {
        let mut capture = state
            .lock()
            .map_err(|_| anyhow::anyhow!("capture state failed"))?;
        if let Some(value) = response_value(&headers, &body) {
            if let Some(record) =
                capture
                    .correlator
                    .on_inbound_scoped(&value, now_ms(), &request_scope)
            {
                capture.store.append(&record)?;
            }
        } else {
            let record = capture.correlator.on_unparsed("inbound", now_ms());
            capture.store.append(&record)?;
        }
    }
    write_response(&mut stream, status, &headers, &body)?;
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> anyhow::Result<IncomingRequest> {
    let mut reader = BufReader::new(stream);
    let deadline = Instant::now() + IO_TIMEOUT;
    let request_line = read_line_bounded(&mut reader, MAX_HEADER_BYTES, deadline)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().context("HTTP method is missing")?.to_owned();
    let _path = parts.next().context("HTTP path is missing")?;
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        bail!("unsupported HTTP request line");
    }
    let mut headers = Vec::new();
    let mut header_bytes = request_line.len();
    loop {
        let line = read_line_bounded(
            &mut reader,
            MAX_HEADER_BYTES.saturating_sub(header_bytes),
            deadline,
        )?;
        header_bytes = header_bytes.saturating_add(line.len());
        if header_bytes > MAX_HEADER_BYTES {
            bail!("HTTP headers exceeded the size limit");
        }
        if line == "\r\n" {
            break;
        }
        let (name, value) = line
            .trim_end_matches(['\r', '\n'])
            .split_once(':')
            .context("HTTP header is invalid")?;
        if name.is_empty()
            || name
                .bytes()
                .any(|byte| !byte.is_ascii_alphanumeric() && byte != b'-')
        {
            bail!("HTTP header name is invalid");
        }
        let value = value.trim();
        if value.bytes().any(|byte| byte.is_ascii_control()) {
            bail!("HTTP header value is invalid");
        }
        headers.push((name.to_owned(), value.to_owned()));
    }
    let content_length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.parse::<usize>())
        .transpose()?
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        bail!("HTTP body exceeded the size limit");
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    Ok(IncomingRequest {
        method,
        headers,
        body,
    })
}

fn read_line_bounded(
    reader: &mut BufReader<&mut TcpStream>,
    limit: usize,
    deadline: Instant,
) -> anyhow::Result<String> {
    if limit == 0 {
        bail!("HTTP headers exceeded the size limit");
    }
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("HTTP request deadline exceeded")?;
        reader.get_mut().set_read_timeout(Some(remaining))?;
        let available = reader.fill_buf()?;
        if available.is_empty() {
            bail!("HTTP request ended before the headers were complete");
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if bytes.len().saturating_add(consumed) > limit {
            bail!("HTTP headers exceeded the size limit");
        }
        let complete = available.get(consumed.saturating_sub(1)) == Some(&b'\n');
        bytes.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if complete {
            return String::from_utf8(bytes).context("HTTP headers are not UTF-8");
        }
    }
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> anyhow::Result<()> {
    write!(stream, "HTTP/1.1 {status} {}\r\n", reason(status))?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(
        stream,
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        413 => "Payload Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Response",
    }
}

fn response_value(headers: &[(String, String)], body: &[u8]) -> Option<Value> {
    let content_type = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.split(';').next().unwrap_or(""));
    match content_type {
        Some("application/json") => serde_json::from_slice(body).ok(),
        Some("text/event-stream") => std::str::from_utf8(body).ok().and_then(|text| {
            text.replace("\r\n", "\n")
                .split("\n\n")
                .filter_map(|event| {
                    let data = event
                        .lines()
                        .filter_map(|line| line.strip_prefix("data:"))
                        .map(str::trim_start)
                        .collect::<Vec<_>>()
                        .join("\n");
                    serde_json::from_str(&data).ok()
                })
                .find(|value: &Value| value.get("id").is_some())
        }),
        _ => None,
    }
}

fn read_bounded(mut reader: impl Read) -> anyhow::Result<Vec<u8>> {
    let mut body = Vec::new();
    reader
        .by_ref()
        .take((MAX_BODY_BYTES + 1) as u64)
        .read_to_end(&mut body)?;
    if body.len() > MAX_BODY_BYTES {
        bail!("HTTP body exceeded the size limit");
    }
    Ok(body)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
