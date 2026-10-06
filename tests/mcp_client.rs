use mcpeval::mcp_client::{McpClient, ToolResponse, TransportFailure};
use serde_json::json;

const FIXTURE: &str = "tests/fixtures/probe_clean_server.py";

#[test]
fn non_object_tool_results_are_rejected_without_echoing_payloads() {
    for result in [
        json!(null),
        json!(true),
        json!(7),
        json!("CANARY private result"),
        json!(["CANARY private result"]),
    ] {
        let error = mcpeval::mcp_client::classify_tool_response(&json!({"result":result}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("must be an object"));
        assert!(!error.contains("CANARY"));
    }
}

#[test]
fn malformed_tool_error_flags_are_rejected_instead_of_success() {
    for flag in [
        json!("CANARY private flag"),
        json!(1),
        json!(null),
        json!([]),
        json!({}),
    ] {
        let response = json!({"jsonrpc":"2.0","id":1,"result":{
            "isError":flag,"content":[{"type":"text","text":"CANARY private output"}]}});
        let error = mcpeval::mcp_client::classify_tool_response(&response)
            .unwrap_err()
            .to_string();
        assert!(error.contains("isError"));
        assert!(!error.contains("CANARY"));
    }
    for flag in [None, Some(false), Some(true)] {
        let mut result = json!({"content":[]});
        if let Some(flag) = flag {
            result["isError"] = json!(flag);
        }
        let response =
            mcpeval::mcp_client::classify_tool_response(&json!({"result":result})).unwrap();
        assert_eq!(
            matches!(response, ToolResponse::Error { .. }),
            flag == Some(true)
        );
    }
}

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
    client.set_response_timeout(Some(std::time::Duration::from_secs(2)));
    let error = client.initialize().unwrap_err();
    assert_eq!(
        TransportFailure::of(&error),
        Some(TransportFailure::Timeout)
    );
    let error = error.to_string();
    assert!(error.contains("timed out"));
    assert!(!error.contains("not-json"));
}

#[test]
fn reports_early_exit_without_hanging() {
    let mut client = McpClient::spawn(&command(Some("early-exit"))).unwrap();
    let error = client.initialize().unwrap_err();
    assert_eq!(TransportFailure::of(&error), Some(TransportFailure::Closed));
    assert!(error.to_string().contains("closed stdout"));
}
