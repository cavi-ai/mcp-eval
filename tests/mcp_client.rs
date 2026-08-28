use mcpeval::mcp_client::{McpClient, ToolResponse};
use serde_json::json;

const FIXTURE: &str = "tests/fixtures/probe_clean_server.py";

fn command(mode: Option<&str>) -> Vec<String> {
    let mut command = vec!["python3".into(), FIXTURE.into()];
    if let Some(mode) = mode {
        command.push(mode.into());
    }
    command
}

#[test]
fn initializes_lists_and_calls_a_real_stdio_server() {
    let mut client = McpClient::spawn(&command(None)).unwrap();
    client.initialize().unwrap();
    let tools = client.list_tools().unwrap();
    assert_eq!(
        tools,
        vec![
            "read_counter",
            "describe_status",
            "flaky_read",
            "break_session",
            "recover_session",
            "session_status",
            "shared_read"
        ]
    );
    let response = client.call_tool("read_counter", &json!({})).unwrap();
    match response {
        ToolResponse::Success(value) => {
            assert_eq!(value["structuredContent"]["count"], 1);
        }
        ToolResponse::Error { .. } => panic!("clean fixture returned an error"),
    }
}

#[test]
fn mismatched_ids_fail_fast_without_echoing_payloads() {
    let mut client = McpClient::spawn(&command(Some("mismatched-id"))).unwrap();
    let error = client.initialize().unwrap_err().to_string();
    assert!(error.contains("response id"));
    assert!(!error.contains("probe-fixture"));
}

#[test]
fn banners_and_notifications_are_skipped_without_failing_the_session() {
    // A server that writes prose to stdout before (and between) JSON-RPC
    // frames is a supported reality, not a protocol violation: the "banner"
    // mode emits one unparseable line per request and then answers
    // normally. The session must survive, and the prose must never surface
    // in an error message.
    let mut client = McpClient::spawn(&command(Some("banner"))).unwrap();
    client.initialize().unwrap();
    let tools = client.list_tools().unwrap();
    assert!(tools.contains(&"describe_status".to_owned()));
    let response = client.call_tool("read_counter", &json!({})).unwrap();
    assert!(matches!(response, ToolResponse::Success(_)));
}

#[test]
fn a_server_that_never_answers_fails_with_a_timeout() {
    // The "malformed" mode emits unparseable lines and never responds:
    // skipping them must lead to a timeout, not an echo of the prose.
    let mut client = McpClient::spawn(&command(Some("malformed"))).unwrap();
    let error = client.initialize().unwrap_err().to_string();
    assert!(error.contains("timed out"));
    assert!(!error.contains("not-json"));
}

#[test]
fn reports_early_exit_without_hanging() {
    let mut client = McpClient::spawn(&command(Some("early-exit"))).unwrap();
    let error = client.initialize().unwrap_err().to_string();
    assert!(error.contains("closed stdout"));
}
