use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const FIXTURE: &str = "tests/fixtures/echo_server.py";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

struct TestHome(PathBuf);

impl TestHome {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mcpeval-shim-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shim_command(home: &TestHome) -> Command {
    let mut command = Command::new(bin());
    command
        .args(["shim", "--server", "demo", "--", "python3", FIXTURE])
        .env("MCPEVAL_HOME", home.path())
        .env_remove("MCPEVAL_SESSION")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn read_store(home: &TestHome) -> String {
    let mut body = String::new();
    for entry in std::fs::read_dir(home.path().join("store")).unwrap() {
        body.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap());
    }
    body
}

fn read_records(home: &TestHome) -> Vec<serde_json::Value> {
    read_store(home)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("shim did not exit within {timeout:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn proxies_messages_and_records_privacy_safe_shapes() {
    const SESSION: &str = "session-from-environment";
    const REQUESTS: &[u8] = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"navigate\",\"arguments\":{\"waitUntil\":\"networkIdle\",\"url\":\"https://www.example.com/secret?token=CANARY-query\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"boom\",\"arguments\":{}}}\n",
    )
    .as_bytes();
    const RESPONSES: &[u8] = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"navigate\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"waitUntil\":{\"type\":\"string\",\"enum\":[\"commit\",\"networkIdle\"]}}}}]}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"echo\":true}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"error\":{\"code\":-32000,\"message\":\"session 0be9b59c-af70-47b0-9169-d9de92330600 gone\"}}\n",
    )
    .as_bytes();

    let home = TestHome::new();
    let mut child = shim_command(&home)
        .env("MCPEVAL_SESSION", SESSION)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut actual_responses = Vec::new();
    for request in REQUESTS.split_inclusive(|byte| *byte == b'\n') {
        stdin.write_all(request).unwrap();
        stdin.flush().unwrap();
        stdout.read_until(b'\n', &mut actual_responses).unwrap();
    }
    drop(stdin);

    let status = child.wait().unwrap();
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    assert!(
        status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert_eq!(
        actual_responses, RESPONSES,
        "child stdout changed in transit"
    );

    let records = read_records(&home);
    assert_eq!(records.len(), 3);
    let expected_session = mcpeval::privacy::opaque_session(SESSION);
    assert!(records
        .iter()
        .all(|record| record["session"] == expected_session));
    assert!(!read_store(&home).contains(SESSION));

    let call = records
        .iter()
        .find(|record| record["tool"] == "navigate")
        .unwrap();
    assert_eq!(call["args"]["waitUntil"], "enum:networkIdle");
    assert_eq!(call["args"]["url"], "url:example.com");
    assert_eq!(call["outcome"], "ok");

    let failed = records
        .iter()
        .find(|record| record["outcome"] == "error")
        .unwrap();
    assert_eq!(failed["error"]["template"], "{message}");

    let stored = read_store(&home);
    assert!(!stored.contains("CANARY"));
    assert!(!stored.contains("secret?token"));
    assert!(!stored.contains("0be9b59c-af70-47b0-9169-d9de92330600"));
}

#[test]
fn forwards_unparsed_bytes_exactly_without_persisting_them() {
    const UNPARSED: &[u8] = b"RAW-CANARY /Users/someone/private.pdf?token=CANARY-query \xff";

    let home = TestHome::new();
    let mut child = shim_command(&home).spawn().unwrap();
    child.stdin.take().unwrap().write_all(UNPARSED).unwrap();

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, UNPARSED, "unparsed frame changed in transit");

    let records = read_records(&home);
    assert_eq!(records.len(), 2, "both directions must record as unparsed");
    assert!(records.iter().all(|record| record["outcome"] == "unparsed"));
    assert!(records
        .iter()
        .any(|record| record["method"] == "unparsed/outbound"));
    assert!(records
        .iter()
        .any(|record| record["method"] == "unparsed/inbound"));

    let stored = read_store(&home);
    for forbidden in ["RAW-CANARY", "CANARY-query", "private.pdf", "token="] {
        assert!(
            !stored.contains(forbidden),
            "unparsed payload reached disk: {stored}"
        );
    }
}

#[test]
fn semantically_invalid_json_is_forwarded_and_recorded_as_unparsed() {
    const INVALID: &[u8] = b"{}\n";
    const RESPONSE: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":null,\"result\":{\"echo\":true}}\n";
    let home = TestHome::new();
    let mut child = shim_command(&home).spawn().unwrap();
    child.stdin.take().unwrap().write_all(INVALID).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, RESPONSE);
    let records = read_records(&home);
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record["outcome"] == "unparsed"));
}

#[test]
fn parsed_payload_values_paths_and_queries_never_reach_disk() {
    const REQUEST: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"x\",\"arguments\":{\"secret\":\"CANARY-8f3a\",\"path\":\"/Users/someone/private.pdf\",\"url\":\"https://CANARY.customer.example.co.uk/a?token=CANARY-9b2\"}}}\n";

    let home = TestHome::new();
    let mut child = shim_command(&home).spawn().unwrap();
    child.stdin.take().unwrap().write_all(REQUEST).unwrap();

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stored = read_store(&home);
    assert!(stored.contains("url:example.co.uk"));
    for forbidden in ["CANARY", "private.pdf", "/Users/", "token=", "?token"] {
        assert!(
            !stored.contains(forbidden),
            "parsed payload reached disk: {stored}"
        );
    }
}

#[test]
fn generates_one_uuid_session_for_the_process() {
    const REQUESTS: &[u8] = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
    )
    .as_bytes();

    let home = TestHome::new();
    let mut child = shim_command(&home).spawn().unwrap();
    child.stdin.take().unwrap().write_all(REQUESTS).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let records = read_records(&home);
    assert_eq!(records.len(), 2);
    let session = records[0]["session"].as_str().unwrap();
    assert_eq!(session.len(), 72);
    assert!(session.starts_with("session:"));
    assert!(session[8..].bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(records.iter().all(|record| record["session"] == session));
}

#[test]
fn returns_child_status_and_stderr_without_waiting_for_agent_eof() {
    let home = TestHome::new();
    let mut child = Command::new(bin())
        .args([
            "shim",
            "--server",
            "demo",
            "--",
            "python3",
            FIXTURE,
            "--exit-code",
            "23",
        ])
        .env("MCPEVAL_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let open_agent_stdin = child.stdin.take().unwrap();

    let status = wait_for_exit(&mut child, Duration::from_secs(3));
    assert_eq!(status.code(), Some(23));

    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    assert_eq!(stderr, b"fixture stderr exact\n");
    drop(open_agent_stdin);
}

#[cfg(unix)]
#[test]
fn maps_child_signal_status_without_reporting_success() {
    let home = TestHome::new();
    let output = Command::new(bin())
        .args([
            "shim", "--server", "demo", "--", "python3", FIXTURE, "--signal", "SIGTERM",
        ])
        .env("MCPEVAL_HOME", home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(128 + 15));
}

#[test]
fn duplex_backpressure_never_deadlocks_or_changes_child_stdout() {
    const LINE_COUNT: usize = 512;
    const LINE_WIDTH: usize = 4_096;
    const INPUT_SIZE: usize = 2 * 1024 * 1024;

    let home = TestHome::new();
    let mut child = Command::new(bin())
        .args([
            "shim",
            "--server",
            "demo",
            "--",
            "python3",
            FIXTURE,
            "--duplex-stress",
            &LINE_COUNT.to_string(),
            &LINE_WIDTH.to_string(),
            &(INPUT_SIZE + 1).to_string(),
        ])
        .env("MCPEVAL_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let writer = thread::spawn(move || {
        let mut input = vec![b'I'; INPUT_SIZE];
        input.push(b'\n');
        stdin.write_all(&input)
    });
    let mut stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });

    let status = wait_for_exit(&mut child, Duration::from_secs(3));
    writer.join().unwrap().unwrap();
    let output = reader.join().unwrap().unwrap();

    assert!(status.success());
    let line = [vec![b'O'; LINE_WIDTH - 1], vec![b'\n']].concat();
    assert_eq!(output, line.repeat(LINE_COUNT));
}
