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
    assert_eq!(tools, vec!["read_counter", "describe_status"]);
    let response = client.call_tool("read_counter", &json!({})).unwrap();
    match response {
        ToolResponse::Success(value) => {
            assert_eq!(value["structuredContent"]["count"], 1);
        }
        ToolResponse::Error { .. } => panic!("clean fixture returned an error"),
    }
}

#[test]
fn rejects_mismatched_ids_without_echoing_payloads() {
    let mut client = McpClient::spawn(&command(Some("mismatched-id"))).unwrap();
    let error = client.initialize().unwrap_err().to_string();
    assert!(error.contains("response id"));
    assert!(!error.contains("probe-fixture"));
}

#[test]
fn rejects_malformed_frames_without_echoing_raw_lines() {
    let mut client = McpClient::spawn(&command(Some("malformed"))).unwrap();
    let error = client.initialize().unwrap_err().to_string();
    assert!(error.contains("valid JSON"));
    assert!(!error.contains("not-json"));
}

#[test]
fn reports_early_exit_without_hanging() {
    let mut client = McpClient::spawn(&command(Some("early-exit"))).unwrap();
    let error = client.initialize().unwrap_err().to_string();
    assert!(error.contains("closed stdout"));
}
