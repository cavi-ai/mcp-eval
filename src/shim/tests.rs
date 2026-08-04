use std::io;
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;

use anyhow::anyhow;
use serde_json::{json, Value};

use super::{
    cancel_synchronous_io_until_observed, cancellation_pair, cleanup_after_error,
    forward_or_cancel, join_finished_pump, terminate_and_reap_process, CancellableReader,
    Direction, FrameMetadata, FrameOrder, ProcessControl, PumpHandle,
};
#[cfg(unix)]
use super::{
    complete_frame, forward, handle_event, shutdown_pumps_and_drain, spawn_pump, Event,
    EventSender, ShutdownCancellation,
};
use crate::correlate::Correlator;
#[cfg(unix)]
use crate::store::Store;

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
fn shutdown_drains_completion_emitted_while_input_pump_is_joined() {
    let root = std::env::temp_dir().join(format!("mcpeval-final-event-{}", uuid::Uuid::new_v4()));
    let mut store = Store::open(Some(root.clone())).unwrap();
    let mut correlator = Correlator::new("demo".into(), "session".into());
    let mut frame_order = FrameOrder::default();
    let mut output_finished = false;
    let mut operation_error = None;
    let mut recording_error = None;

    let (tx, rx) = mpsc::channel::<Event>();
    let events = Arc::new(std::sync::Mutex::new(EventSender {
        tx,
        next_sequence: 0,
    }));
    let (input_cancel, mut input_cancellation) = cancellation_pair().unwrap();
    let input_events = Arc::clone(&events);
    let (flushed_tx, flushed_rx) = mpsc::channel();
    let input_thread = spawn_pump(Direction::Outbound, Arc::clone(&events), move || {
        let sequence = super::reserve_frame(
            &input_events,
            metadata(
                Direction::Outbound,
                json!({ "jsonrpc": "2.0", "id": 41, "method": "tools/call", "params": { "name": "late" } }),
                10,
            ),
        )?;
        forward(&mut io::sink(), b"request\n", "writing injected child stdin")?;
        flushed_tx.send(()).unwrap();

        let mut cancellation_byte = [0_u8; 1];
        let cancellation_bytes = input_cancellation.read(&mut cancellation_byte)?;
        if cancellation_bytes != 0 {
            return Err(io::Error::other("unexpected cancellation payload"));
        }
        complete_frame(&input_events, sequence, std::time::Instant::now())
    })
    .unwrap();
    let input_pump = PumpHandle {
        thread: input_thread,
        cancel: input_cancel,
    };

    flushed_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("input frame must flush before the response");
    let (output_cancel, _output_cancellation) = cancellation_pair().unwrap();
    let output_events = Arc::clone(&events);
    let output_thread = spawn_pump(Direction::Inbound, Arc::clone(&events), move || {
        let sequence = super::reserve_frame(
            &output_events,
            metadata(
                Direction::Inbound,
                json!({ "jsonrpc": "2.0", "id": 41, "result": { "ok": true } }),
                20,
            ),
        )?;
        forward(
            &mut io::sink(),
            b"response\n",
            "writing injected agent stdout",
        )?;
        complete_frame(&output_events, sequence, std::time::Instant::now())
    })
    .unwrap();
    let output_pump = PumpHandle {
        thread: output_thread,
        cancel: output_cancel,
    };

    while !output_finished {
        let event = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("output pump must finish");
        if let Some(error) = handle_event(
            event,
            &mut frame_order,
            &mut correlator,
            &mut store,
            &mut output_finished,
            &mut recording_error,
        ) {
            panic!("unexpected coordinator error: {error:#}");
        }
    }
    assert!(
        std::fs::read_dir(root.join("store"))
            .unwrap()
            .next()
            .is_none(),
        "the response must remain blocked behind the paused request"
    );

    shutdown_pumps_and_drain(
        input_pump,
        output_pump,
        events,
        rx,
        ShutdownCancellation {
            input: true,
            output: false,
        },
        &mut operation_error,
        |event| {
            handle_event(
                event,
                &mut frame_order,
                &mut correlator,
                &mut store,
                &mut output_finished,
                &mut recording_error,
            )
        },
    );

    assert!(operation_error.is_none());
    assert!(recording_error.is_none());
    let path = std::fs::read_dir(root.join("store"))
        .unwrap()
        .next()
        .expect("late completion must persist the correlated response")
        .unwrap()
        .path();
    let body = std::fs::read_to_string(path).unwrap();
    let record: serde_json::Value = serde_json::from_str(body.trim_end()).unwrap();
    assert_eq!(record["method"], "tools/call");
    assert_eq!(record["tool"], "late");
    assert_eq!(record["outcome"], "ok");

    std::fs::remove_dir_all(root).unwrap();
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

#[cfg(unix)]
#[test]
fn pump_handle_owns_the_platform_cancellation_interface() {
    let (source, _source_peer) = UnixStream::pair().unwrap();
    let (cancel, cancellation) = cancellation_pair().unwrap();
    let thread = std::thread::spawn(move || {
        let mut reader = CancellableReader::new(source, cancellation);
        let mut byte = [0_u8; 1];
        match reader.read(&mut byte) {
            Err(_) if reader.is_cancelled() => Ok(()),
            result => result.map(|_| ()),
        }
    });
    let pump = PumpHandle { thread, cancel };

    pump.cancel().unwrap();
    assert!(pump.thread.join().unwrap().is_ok());
}

#[test]
fn synchronous_io_cancellation_retries_the_between_reads_race() {
    let attempts = std::cell::Cell::new(0);

    cancel_synchronous_io_until_observed(
        || false,
        || {
            attempts.set(attempts.get() + 1);
            Ok(attempts.get() == 3)
        },
    )
    .unwrap();

    assert_eq!(attempts.get(), 3);
}

#[test]
fn cancellation_suppresses_an_aborted_pump_write() {
    let stopped = forward_or_cancel(
        &mut FailingWriter,
        b"frame",
        "writing injected destination",
        || true,
    )
    .unwrap();
    assert!(!stopped);

    let error = forward_or_cancel(
        &mut FailingWriter,
        b"frame",
        "writing injected destination",
        || false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("WRITE-CANARY"));
}

struct FailingWriter;

impl io::Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("WRITE-CANARY"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
#[test]
fn windows_cfg_exports_run_reader_and_pump_cancellation_contracts() {
    let _: fn(String, Vec<String>) -> anyhow::Result<i32> = super::run;
    let (cancel, cancellation) = cancellation_pair().unwrap();
    let reader = CancellableReader::new(io::empty(), cancellation);
    assert!(!reader.is_cancelled());

    let pump = PumpHandle {
        thread: std::thread::spawn(|| Ok(())),
        cancel,
    };
    pump.cancel().unwrap();
    assert!(pump.thread.join().unwrap().is_ok());
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
