use std::time::{Duration, Instant};

use mcpeval::mcp_client::{McpClient, TransportFailure};
use serde_json::json;

fn client(mode: &str) -> McpClient {
    let mut client = McpClient::spawn(&[
        "python3".into(),
        "tests/fixtures/resource_bounds_server.py".into(),
        mode.into(),
    ])
    .unwrap();
    client.set_response_timeout(Some(Duration::from_millis(150)));
    client
}

#[test]
fn interleaved_notifications_do_not_extend_a_call_deadline() {
    let mut client = client("interleaved");
    let start = Instant::now();
    let error = client
        .call_tool_observing("read", &json!({}), &mut |_, _| None, 8)
        .unwrap_err();
    assert_eq!(
        TransportFailure::of(&error),
        Some(TransportFailure::Timeout)
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn oversized_server_frames_fail_with_a_content_free_error() {
    let mut client = client("oversized");
    client.set_response_timeout(Some(Duration::from_secs(2)));
    let error = client.raw_request("ping", json!({})).unwrap_err();
    assert_eq!(TransportFailure::of(&error), Some(TransportFailure::Closed));
    assert!(!format!("{error:#}").contains("CANARY"));
}

#[test]
fn notification_floods_cannot_accumulate_without_a_bound() {
    let mut client = client("notifications");
    client.set_response_timeout(Some(Duration::from_secs(2)));
    let error = client.raw_request("ping", json!({})).unwrap_err();
    assert_eq!(TransportFailure::of(&error), Some(TransportFailure::Closed));
}

#[test]
fn a_server_that_does_not_read_cannot_block_request_writes() {
    let mut client = client("no-read");
    let start = Instant::now();
    let error = client
        .raw_request("ping", json!({"padding":"x".repeat(1_000_000)}))
        .unwrap_err();
    assert_eq!(
        TransportFailure::of(&error),
        Some(TransportFailure::Timeout)
    );
    drop(client);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn dropping_the_client_terminates_and_reaps_its_child() {
    let mut client = client("pid");
    client.set_response_timeout(Some(Duration::from_secs(2)));
    let response = client.raw_request("ping", json!({})).unwrap();
    let pid = response["result"]["pid"].as_i64().unwrap() as i32;
    drop(client);
    assert_eq!(unsafe { nix::libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(nix::libc::ESRCH)
    );
}
