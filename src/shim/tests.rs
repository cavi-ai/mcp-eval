use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use serde_json::{json, Value};

use super::{
    cancellation_pair, cleanup_after_error, join_finished_pump, terminate_and_reap_process,
    CancellableReader, Direction, FrameMetadata, FrameOrder, ProcessControl,
};
use crate::correlate::Correlator;

fn metadata(direction: Direction, value: Value, observed_ms: u64) -> FrameMetadata {
    FrameMetadata {
        direction,
        value: Some(value),
        observed_ms,
    }
}

#[test]
fn reused_id_request_cannot_overtake_the_response_that_triggered_it() {
    let mut order = FrameOrder::default();
    let mut correlator = Correlator::new("demo".into(), "session".into());

    order
        .reserve(
            0,
            metadata(
                Direction::Outbound,
                json!({ "jsonrpc": "2.0", "id": 7, "method": "first" }),
                10,
            ),
        )
        .unwrap();
    order.complete(0, 1).unwrap();
    let first = order.drain_ready();
    correlator.on_outbound(first[0].value.as_ref().unwrap(), first[0].observed_ms);

    order
        .reserve(
            1,
            metadata(
                Direction::Inbound,
                json!({ "jsonrpc": "2.0", "id": 7, "result": { "first": true } }),
                20,
            ),
        )
        .unwrap();
    order
        .reserve(
            2,
            metadata(
                Direction::Outbound,
                json!({ "jsonrpc": "2.0", "id": 7, "method": "second" }),
                21,
            ),
        )
        .unwrap();
    order.complete(2, 1).unwrap();
    assert!(
        order.drain_ready().is_empty(),
        "later completion must wait for the causally prior response"
    );

    order.complete(1, 1).unwrap();
    let ready = order.drain_ready();
    assert_eq!(ready.len(), 2);
    assert_eq!(ready[0].direction, Direction::Inbound);
    assert_eq!(ready[1].direction, Direction::Outbound);

    let first_record = correlator
        .on_inbound(ready[0].value.as_ref().unwrap(), ready[0].observed_ms)
        .unwrap();
    assert_eq!(first_record.method, "first");
    correlator.on_outbound(ready[1].value.as_ref().unwrap(), ready[1].observed_ms);
    let second_record = correlator
        .on_inbound(
            &json!({ "jsonrpc": "2.0", "id": 7, "result": { "second": true } }),
            30,
        )
        .unwrap();
    assert_eq!(second_record.method, "second");
}

#[test]
fn tools_list_response_is_applied_before_the_causal_next_tool_call() {
    let mut order = FrameOrder::default();
    let mut correlator = Correlator::new("demo".into(), "session".into());

    order
        .reserve(
            0,
            metadata(
                Direction::Outbound,
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
                10,
            ),
        )
        .unwrap();
    order.complete(0, 1).unwrap();
    let list_request = order.drain_ready().remove(0);
    correlator.on_outbound(
        list_request.value.as_ref().unwrap(),
        list_request.observed_ms,
    );

    order
        .reserve(
            1,
            metadata(
                Direction::Inbound,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "tools": [{
                        "name": "navigate",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "waitUntil": {
                                    "type": "string",
                                    "enum": ["commit", "networkIdle"]
                                }
                            }
                        }
                    }]}
                }),
                20,
            ),
        )
        .unwrap();
    order
        .reserve(
            2,
            metadata(
                Direction::Outbound,
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "navigate",
                        "arguments": { "waitUntil": "networkIdle" }
                    }
                }),
                21,
            ),
        )
        .unwrap();
    order.complete(2, 1).unwrap();
    assert!(order.drain_ready().is_empty());

    order.complete(1, 1).unwrap();
    let ready = order.drain_ready();
    assert_eq!(ready.len(), 2);
    correlator.on_inbound(ready[0].value.as_ref().unwrap(), ready[0].observed_ms);
    correlator.on_outbound(ready[1].value.as_ref().unwrap(), ready[1].observed_ms);

    let record = correlator
        .on_inbound(&json!({ "jsonrpc": "2.0", "id": 2, "result": {} }), 30)
        .unwrap();
    assert_eq!(record.args.unwrap()["waitUntil"], "enum:networkIdle");
}

#[cfg(unix)]
#[test]
fn cancellation_wakes_and_releases_a_blocked_input_reader() {
    let (source, mut source_peer) = UnixStream::pair().unwrap();
    let (cancel, cancellation) = cancellation_pair().unwrap();
    let active = Arc::new(AtomicBool::new(false));
    let thread_active = Arc::clone(&active);
    let (done_tx, done_rx) = std::sync::mpsc::channel();

    let reader = std::thread::spawn(move || {
        thread_active.store(true, Ordering::Release);
        let mut reader = CancellableReader::new(source, cancellation);
        let mut byte = [0_u8; 1];
        let result = reader.read(&mut byte);
        let cancelled = reader.is_cancelled();
        thread_active.store(false, Ordering::Release);
        done_tx.send((result, cancelled)).unwrap();
    });

    while !active.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    cancel.cancel().unwrap();

    let (result, cancelled) = done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("cancellation must wake the blocked input reader");
    reader.join().unwrap();
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
    assert!(cancelled);
    assert!(!active.load(Ordering::Acquire));
    assert_eq!(
        source_peer.write_all(b"still-owned").unwrap_err().kind(),
        io::ErrorKind::BrokenPipe,
        "the returned reader must not leave an orphan holding its input"
    );
}

struct FakeProcess {
    kill_error: Option<&'static str>,
    wait_error: Option<&'static str>,
    wait_called: bool,
}

impl ProcessControl for FakeProcess {
    type Status = ();

    fn kill(&mut self) -> io::Result<()> {
        match self.kill_error {
            Some(message) => Err(io::Error::other(message)),
            None => Ok(()),
        }
    }

    fn wait(&mut self) -> io::Result<Self::Status> {
        self.wait_called = true;
        match self.wait_error {
            Some(message) => Err(io::Error::other(message)),
            None => Ok(()),
        }
    }
}

#[test]
fn kill_failure_still_attempts_wait_and_preserves_both_errors() {
    let mut process = FakeProcess {
        kill_error: Some("KILL-CANARY"),
        wait_error: Some("WAIT-CANARY"),
        wait_called: false,
    };

    let error = terminate_and_reap_process(&mut process).unwrap_err();

    assert!(process.wait_called, "wait must run even when kill fails");
    let message = format!("{error:#}");
    assert!(message.contains("KILL-CANARY"), "{message}");
    assert!(message.contains("WAIT-CANARY"), "{message}");
}

#[test]
fn child_status_and_cleanup_errors_are_both_preserved() {
    let mut process = FakeProcess {
        kill_error: Some("CLEANUP-KILL-CANARY"),
        wait_error: Some("CLEANUP-WAIT-CANARY"),
        wait_called: false,
    };

    let (status, error) = cleanup_after_error(
        &mut process,
        anyhow!("STATUS-CANARY"),
        "cleaning up after child status failure",
    );
    let message = format!("{error:#}");

    assert!(status.is_none());
    assert!(process.wait_called);
    assert!(message.contains("STATUS-CANARY"), "{message}");
    assert!(message.contains("CLEANUP-KILL-CANARY"), "{message}");
    assert!(message.contains("CLEANUP-WAIT-CANARY"), "{message}");
}

#[test]
fn pump_join_failure_does_not_replace_the_primary_error() {
    let handle = std::thread::spawn(|| Err(io::Error::other("JOIN-CANARY")));
    let mut operation_error = Some(anyhow!("PRIMARY-CANARY"));

    join_finished_pump(handle, "injected", &mut operation_error);

    let message = format!("{:#}", operation_error.unwrap());
    assert!(message.contains("PRIMARY-CANARY"), "{message}");
    assert!(message.contains("JOIN-CANARY"), "{message}");
}
