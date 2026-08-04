use std::io::{self, BufReader, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context};

use crate::correlate::Correlator;
use crate::frame::{read_frame, Frame};
use crate::record::CallRecord;
use crate::store::Store;

const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Outbound,
    Inbound,
}

impl Direction {
    fn label(self) -> &'static str {
        match self {
            Self::Outbound => "outbound",
            Self::Inbound => "inbound",
        }
    }
}

#[derive(Debug)]
struct PumpFailure {
    direction: Direction,
    kind: io::ErrorKind,
    message: String,
}

enum Event {
    Frame {
        direction: Direction,
        value: Option<serde_json::Value>,
        observed_ms: u64,
        shim_self_us: u64,
    },
    Finished {
        direction: Direction,
        failure: Option<PumpFailure>,
    },
}

pub fn run(server: String, cmd: Vec<String>) -> anyhow::Result<i32> {
    let (program, args) = cmd.split_first().context("empty server command")?;
    let mut store = Store::open(None).context("opening recording store")?;
    let session =
        std::env::var("MCPEVAL_SESSION").unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());

    let mut child: Child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawning {program}"))?;

    let child_stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            return Err(cleanup_setup_failure(
                &mut child,
                anyhow!("child stdin pipe unavailable"),
            ));
        }
    };
    let child_stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            return Err(cleanup_setup_failure(
                &mut child,
                anyhow!("child stdout pipe unavailable"),
            ));
        }
    };
    let (tx, rx) = mpsc::channel::<Event>();
    let event_order = Arc::new(Mutex::new(()));

    let output_pump = match spawn_output_pump(child_stdout, tx.clone(), Arc::clone(&event_order)) {
        Ok(handle) => handle,
        Err(error) => {
            return Err(cleanup_setup_failure(
                &mut child,
                anyhow::Error::from(error).context("spawning child stdout pump"),
            ));
        }
    };
    let input_pump = match spawn_input_pump(child_stdin, tx, Arc::clone(&event_order)) {
        Ok(handle) => handle,
        Err(error) => {
            let setup_error = cleanup_setup_failure(
                &mut child,
                anyhow::Error::from(error).context("spawning agent stdin pump"),
            );
            let _ = output_pump.join();
            return Err(setup_error);
        }
    };

    let mut correlator = Correlator::new(server, session);
    let mut input_finished = false;
    let mut output_finished = false;
    let mut child_status = None;
    let mut operation_error = None;
    let mut recording_error = None;
    let mut kill_requested = false;

    loop {
        if child_status.is_none() {
            child_status = match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    operation_error.get_or_insert_with(|| {
                        anyhow::Error::from(error).context("checking child process status")
                    });
                    Some(
                        terminate_and_reap(&mut child)
                            .context("cleaning up after child status failure")?,
                    )
                }
            };
        }

        if child_status.is_some() && output_finished {
            let pending_events = {
                let _guard = lock_event_order(&event_order);
                rx.try_iter().collect::<Vec<_>>()
            };
            for event in pending_events {
                if let Some(failure) = handle_event(
                    event,
                    &mut correlator,
                    &mut store,
                    &mut input_finished,
                    &mut output_finished,
                    &mut recording_error,
                ) {
                    remember_pump_failure(&mut operation_error, failure);
                }
            }
            break;
        }

        match rx.recv_timeout(CHILD_POLL_INTERVAL) {
            Ok(event) => {
                if let Some(failure) = handle_event(
                    event,
                    &mut correlator,
                    &mut store,
                    &mut input_finished,
                    &mut output_finished,
                    &mut recording_error,
                ) {
                    remember_pump_failure(&mut operation_error, failure);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !output_finished {
                    operation_error.get_or_insert_with(|| {
                        anyhow!("stdio pump event channel closed before child stdout completed")
                    });
                    output_finished = true;
                }
            }
        }

        if operation_error.is_some() && child_status.is_none() && !kill_requested {
            match child.kill() {
                Ok(()) => kill_requested = true,
                Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                    kill_requested = true;
                }
                Err(error) => {
                    operation_error = Some(anyhow!(
                        "{}; also failed to terminate child: {error}",
                        operation_error.as_ref().unwrap()
                    ));
                    kill_requested = true;
                }
            }
        }
    }

    join_finished_pump(output_pump, "child stdout", &mut operation_error);
    if input_finished {
        join_finished_pump(input_pump, "agent stdin", &mut operation_error);
    } else {
        // Process stdin cannot be cancelled portably. The child has exited, so
        // returning is preferable to hanging while an agent keeps stdin open.
        // The binary exits immediately with the child's status, which tears down
        // this now-inert reader thread without changing proxied bytes.
        drop(input_pump);
    }

    if let Some(error) = operation_error {
        return Err(error);
    }
    if let Some(error) = recording_error {
        return Err(error);
    }

    child_exit_code(child_status.context("child exited without a status")?)
}

fn spawn_input_pump(
    child_stdin: std::process::ChildStdin,
    tx: Sender<Event>,
    event_order: Arc<Mutex<()>>,
) -> io::Result<JoinHandle<io::Result<()>>> {
    spawn_pump(
        Direction::Outbound,
        tx.clone(),
        event_order.clone(),
        move || {
            let stdin = io::stdin();
            let mut source = BufReader::new(stdin.lock());
            let mut destination = child_stdin;

            while let Some(frame) = read_frame(&mut source)
                .map_err(|error| contextual_io(error, "reading agent stdin"))?
            {
                let started = Instant::now();
                let observed_ms = now_ms();
                let _guard = lock_event_order(&event_order);
                forward(&mut destination, &frame, "writing child stdin")?;
                send_frame(&tx, Direction::Outbound, frame, observed_ms, started)?;
            }
            Ok(())
        },
    )
}

fn spawn_output_pump(
    child_stdout: std::process::ChildStdout,
    tx: Sender<Event>,
    event_order: Arc<Mutex<()>>,
) -> io::Result<JoinHandle<io::Result<()>>> {
    spawn_pump(
        Direction::Inbound,
        tx.clone(),
        event_order.clone(),
        move || {
            let mut source = BufReader::new(child_stdout);
            let stdout = io::stdout();
            let mut destination = stdout.lock();

            while let Some(frame) = read_frame(&mut source)
                .map_err(|error| contextual_io(error, "reading child stdout"))?
            {
                let started = Instant::now();
                let observed_ms = now_ms();
                forward(&mut destination, &frame, "writing agent stdout")?;
                let _guard = lock_event_order(&event_order);
                send_frame(&tx, Direction::Inbound, frame, observed_ms, started)?;
            }
            Ok(())
        },
    )
}

fn spawn_pump<F>(
    direction: Direction,
    tx: Sender<Event>,
    event_order: Arc<Mutex<()>>,
    pump: F,
) -> io::Result<JoinHandle<io::Result<()>>>
where
    F: FnOnce() -> io::Result<()> + Send + 'static,
{
    thread::Builder::new()
        .name(format!("mcpeval-{}-pump", direction.label()))
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(pump))
                .unwrap_or_else(|_| {
                    Err(io::Error::other(format!(
                        "{} stdio pump panicked",
                        direction.label()
                    )))
                });
            let failure = result.as_ref().err().map(|error| PumpFailure {
                direction,
                kind: error.kind(),
                message: error.to_string(),
            });
            let _guard = lock_event_order(&event_order);
            let _ = tx.send(Event::Finished { direction, failure });
            result
        })
}

fn forward<W: Write>(destination: &mut W, frame: &Frame, context: &str) -> io::Result<()> {
    destination
        .write_all(&frame.raw)
        .map_err(|error| contextual_io(error, context))?;
    destination
        .flush()
        .map_err(|error| contextual_io(error, context))
}

fn send_frame(
    tx: &Sender<Event>,
    direction: Direction,
    frame: Frame,
    observed_ms: u64,
    started: Instant,
) -> io::Result<()> {
    tx.send(Event::Frame {
        direction,
        value: frame.value,
        observed_ms,
        shim_self_us: started.elapsed().as_micros() as u64,
    })
    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "recording event receiver closed"))
}

fn handle_event(
    event: Event,
    correlator: &mut Correlator,
    store: &mut Store,
    input_finished: &mut bool,
    output_finished: &mut bool,
    recording_error: &mut Option<anyhow::Error>,
) -> Option<PumpFailure> {
    match event {
        Event::Frame {
            direction,
            value,
            observed_ms,
            shim_self_us,
        } => {
            let record = match (direction, value) {
                (Direction::Outbound, Some(value)) => {
                    correlator.on_outbound(&value, observed_ms);
                    None
                }
                (Direction::Inbound, Some(value)) => correlator.on_inbound(&value, observed_ms),
                (direction, None) => Some(correlator.on_unparsed(direction.label(), observed_ms)),
            };
            if let Some(mut record) = record {
                record.shim_self_us = shim_self_us;
                append_record(store, &record, recording_error);
            }
            None
        }
        Event::Finished { direction, failure } => {
            match direction {
                Direction::Outbound => *input_finished = true,
                Direction::Inbound => *output_finished = true,
            }
            failure
        }
    }
}

fn append_record(
    store: &mut Store,
    record: &CallRecord,
    recording_error: &mut Option<anyhow::Error>,
) {
    if let Err(error) = store.append(record).context("appending recording") {
        if recording_error.is_none() {
            *recording_error = Some(error);
        }
    }
}

fn remember_pump_failure(target: &mut Option<anyhow::Error>, failure: PumpFailure) {
    if target.is_none() {
        *target = Some(anyhow!(
            "{} stdio pump failed ({:?}): {}",
            failure.direction.label(),
            failure.kind,
            failure.message
        ));
    }
}

fn join_finished_pump(
    handle: JoinHandle<io::Result<()>>,
    label: &str,
    operation_error: &mut Option<anyhow::Error>,
) {
    let result = match handle.join() {
        Ok(result) => result.map_err(anyhow::Error::from),
        Err(_) => Err(anyhow!("{label} pump thread panicked while joining")),
    };
    if let Err(error) = result {
        if operation_error.is_none() {
            *operation_error = Some(error.context(format!("joining {label} pump")));
        }
    }
}

fn cleanup_setup_failure(child: &mut Child, setup_error: anyhow::Error) -> anyhow::Error {
    match terminate_and_reap(child) {
        Ok(_) => setup_error,
        Err(cleanup_error) => {
            anyhow!("{setup_error:#}; also failed to terminate and reap child: {cleanup_error:#}")
        }
    }
}

fn terminate_and_reap(child: &mut Child) -> anyhow::Result<ExitStatus> {
    match child.kill() {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
        Err(error) => return Err(error).context("terminating child"),
    }
    child.wait().context("reaping child")
}

fn child_exit_code(status: ExitStatus) -> anyhow::Result<i32> {
    if let Some(code) = status.code() {
        return Ok(code);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return Ok(128 + signal);
        }
    }

    Err(anyhow!("child terminated without an exit code: {status}"))
}

fn lock_event_order(mutex: &Mutex<()>) -> MutexGuard<'_, ()> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn contextual_io(error: io::Error, context: &str) -> io::Error {
    io::Error::new(error.kind(), format!("{context}: {error}"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
