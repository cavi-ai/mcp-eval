use std::collections::BTreeMap;
use std::io::{self, BufReader, Read, Write};
#[cfg(unix)]
use std::net::Shutdown;
#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command, ExitStatus, Stdio};
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context};
#[cfg(unix)]
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
#[cfg(windows)]
use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
#[cfg(windows)]
use windows_sys::Win32::System::IO::CancelSynchronousIo;

use crate::correlate::Correlator;
use crate::fingerprint::Salt;
use crate::frame::{read_frame, Frame};
use crate::record::CallRecord;
use crate::store::Store;

const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(any(test, windows))]
fn cancel_synchronous_io_until_observed(
    mut is_finished: impl FnMut() -> bool,
    mut cancel_once: impl FnMut() -> io::Result<bool>,
) -> io::Result<()> {
    loop {
        if is_finished() || cancel_once()? {
            return Ok(());
        }
        thread::yield_now();
    }
}

#[cfg(unix)]
struct CancelHandle {
    sender: UnixStream,
}

#[cfg(unix)]
impl CancelHandle {
    fn cancel(&self) -> io::Result<()> {
        match self.sender.shutdown(Shutdown::Write) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotConnected => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(unix)]
fn cancellation_pair() -> io::Result<(CancelHandle, UnixStream)> {
    let (sender, receiver) = UnixStream::pair()?;
    Ok((CancelHandle { sender }, receiver))
}

#[cfg(unix)]
struct CancellableReader<R> {
    source: R,
    cancellation: UnixStream,
    cancelled: bool,
}

#[cfg(unix)]
impl<R> CancellableReader<R> {
    fn new(source: R, cancellation: UnixStream) -> Self {
        Self {
            source,
            cancellation,
            cancelled: false,
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

#[cfg(unix)]
impl<R: Read + AsFd> Read for CancellableReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            let (source_events, cancellation_events) = {
                let mut descriptors = [
                    PollFd::new(
                        self.cancellation.as_fd(),
                        PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
                    ),
                    PollFd::new(
                        self.source.as_fd(),
                        PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
                    ),
                ];
                match poll(&mut descriptors, PollTimeout::NONE) {
                    Ok(_) => (
                        descriptors[1].revents().unwrap_or_else(PollFlags::empty),
                        descriptors[0].revents().unwrap_or_else(PollFlags::empty),
                    ),
                    Err(nix::errno::Errno::EINTR) => continue,
                    Err(error) => return Err(io::Error::from_raw_os_error(error as i32)),
                }
            };

            if cancellation_events.intersects(
                PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL,
            ) {
                self.cancelled = true;
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "stdio pump cancelled",
                ));
            }
            if source_events.intersects(
                PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL,
            ) {
                return self.source.read(buffer);
            }
        }
    }
}

#[cfg(windows)]
struct CancelHandle {
    cancelled: Arc<AtomicBool>,
}

#[cfg(windows)]
fn cancellation_pair() -> io::Result<(CancelHandle, Arc<AtomicBool>)> {
    let cancelled = Arc::new(AtomicBool::new(false));
    Ok((
        CancelHandle {
            cancelled: Arc::clone(&cancelled),
        },
        cancelled,
    ))
}

#[cfg(windows)]
struct CancellableReader<R> {
    source: R,
    cancellation: Arc<AtomicBool>,
}

#[cfg(windows)]
impl<R> CancellableReader<R> {
    fn new(source: R, cancellation: Arc<AtomicBool>) -> Self {
        Self {
            source,
            cancellation,
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancellation.load(Ordering::Acquire)
    }
}

#[cfg(windows)]
impl<R: Read> Read for CancellableReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.is_cancelled() {
            return Err(cancelled_io_error());
        }
        let result = self.source.read(buffer);
        if self.is_cancelled() {
            return Err(cancelled_io_error());
        }
        result
    }
}

#[cfg(windows)]
fn cancelled_io_error() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "stdio pump cancelled")
}

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

struct FrameMetadata {
    direction: Direction,
    value: Option<serde_json::Value>,
    observed_ms: u64,
}

struct CompletedFrame {
    direction: Direction,
    value: Option<serde_json::Value>,
    observed_ms: u64,
    shim_self_us: u64,
}

struct PendingFrame {
    metadata: Option<FrameMetadata>,
    state: PendingFrameState,
}

enum PendingFrameState {
    Reserved,
    Forwarded(u64),
    Retired,
}

#[derive(Default)]
struct FrameOrder {
    next_reservation: u64,
    next_ready: u64,
    pending: BTreeMap<u64, PendingFrame>,
}

impl FrameOrder {
    fn reserve(&mut self, sequence: u64, metadata: FrameMetadata) -> anyhow::Result<()> {
        if sequence != self.next_reservation {
            anyhow::bail!(
                "frame reservation out of order: expected {}, got {sequence}",
                self.next_reservation
            );
        }
        self.pending.insert(
            sequence,
            PendingFrame {
                metadata: Some(metadata),
                state: PendingFrameState::Reserved,
            },
        );
        self.next_reservation += 1;
        Ok(())
    }

    fn complete(&mut self, sequence: u64, shim_self_us: u64) -> anyhow::Result<()> {
        let pending = self
            .pending
            .get_mut(&sequence)
            .with_context(|| format!("completing unknown frame reservation {sequence}"))?;
        match pending.state {
            PendingFrameState::Reserved => {
                pending.state = PendingFrameState::Forwarded(shim_self_us);
            }
            PendingFrameState::Forwarded(_) => {
                anyhow::bail!("frame reservation {sequence} completed twice");
            }
            PendingFrameState::Retired => {
                anyhow::bail!("frame reservation {sequence} completed after retirement");
            }
        }
        Ok(())
    }

    fn retire_unfinished(&mut self, direction: Direction) {
        for pending in self.pending.values_mut() {
            if matches!(pending.state, PendingFrameState::Reserved)
                && pending
                    .metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.direction == direction)
            {
                pending.metadata = None;
                pending.state = PendingFrameState::Retired;
            }
        }
    }

    fn drain_ready(&mut self) -> Vec<CompletedFrame> {
        let mut ready = Vec::new();
        while let Some(pending) = self.pending.get(&self.next_ready) {
            let shim_self_us = match pending.state {
                PendingFrameState::Reserved => break,
                PendingFrameState::Forwarded(shim_self_us) => Some(shim_self_us),
                PendingFrameState::Retired => None,
            };
            let pending = self
                .pending
                .remove(&self.next_ready)
                .expect("ready frame must remain pending");
            self.next_ready += 1;
            if let Some(shim_self_us) = shim_self_us {
                let metadata = pending
                    .metadata
                    .expect("forwarded frame must retain its metadata");
                ready.push(CompletedFrame {
                    direction: metadata.direction,
                    value: metadata.value,
                    observed_ms: metadata.observed_ms,
                    shim_self_us,
                });
            }
        }
        ready
    }
}

enum Event {
    Reserved {
        sequence: u64,
        metadata: FrameMetadata,
    },
    Forwarded {
        sequence: u64,
        shim_self_us: u64,
    },
    Finished {
        direction: Direction,
        failure: Option<PumpFailure>,
    },
}

struct EventSender {
    tx: Sender<Event>,
    next_sequence: u64,
}

impl EventSender {
    fn reserve(&mut self, metadata: FrameMetadata) -> io::Result<u64> {
        let sequence = self.next_sequence;
        self.tx
            .send(Event::Reserved { sequence, metadata })
            .map_err(|_| event_receiver_closed())?;
        self.next_sequence += 1;
        Ok(sequence)
    }

    fn forwarded(&mut self, sequence: u64, shim_self_us: u64) -> io::Result<()> {
        self.tx
            .send(Event::Forwarded {
                sequence,
                shim_self_us,
            })
            .map_err(|_| event_receiver_closed())
    }

    fn finished(&mut self, direction: Direction, failure: Option<PumpFailure>) -> io::Result<()> {
        self.tx
            .send(Event::Finished { direction, failure })
            .map_err(|_| event_receiver_closed())
    }
}

#[cfg(any(unix, windows))]
struct PumpHandle {
    thread: JoinHandle<io::Result<()>>,
    cancel: CancelHandle,
}

#[cfg(any(unix, windows))]
struct ShutdownCancellation {
    input: bool,
    output: bool,
}

#[cfg(any(unix, windows))]
impl PumpHandle {
    fn cancel(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.cancel.cancel()
        }
        #[cfg(windows)]
        {
            self.cancel.cancelled.store(true, Ordering::Release);
            cancel_synchronous_io_until_observed(
                || self.thread.is_finished(),
                || {
                    // SAFETY: JoinHandle owns this valid thread handle for the
                    // duration of the call, and the API only borrows it.
                    let cancelled =
                        unsafe { CancelSynchronousIo(self.thread.as_raw_handle() as _) };
                    if cancelled != 0 {
                        return Ok(true);
                    }
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                        Ok(false)
                    } else {
                        Err(error)
                    }
                },
            )
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub fn run(_server: String, _cmd: Vec<String>) -> anyhow::Result<i32> {
    anyhow::bail!("the recording stdio shim is unsupported on this target")
}

#[cfg(any(unix, windows))]
pub fn run(server: String, cmd: Vec<String>) -> anyhow::Result<i32> {
    if !crate::privacy::valid_server(&server) {
        anyhow::bail!("server must be a 1-128 character label using only ASCII letters, digits, '.', '_', '-', or ':'");
    }
    let (program, args) = cmd.split_first().context("empty server command")?;
    let mut store = Store::open(None).context("opening recording store")?;
    let salt = Salt::load(store.root()).context("loading fingerprint salt")?;
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
    let events = Arc::new(Mutex::new(EventSender {
        tx,
        next_sequence: 0,
    }));

    let output_pump = match spawn_output_pump(child_stdout, Arc::clone(&events)) {
        Ok(handle) => handle,
        Err(error) => {
            return Err(cleanup_setup_failure(
                &mut child,
                anyhow::Error::from(error).context("spawning child stdout pump"),
            ));
        }
    };
    let input_pump = match spawn_input_pump(child_stdin, Arc::clone(&events)) {
        Ok(handle) => handle,
        Err(error) => {
            let mut setup_error =
                Some(anyhow::Error::from(error).context("spawning agent stdin pump"));
            cancel_pump(&output_pump, "child stdout", &mut setup_error);
            let setup_error = cleanup_setup_failure(
                &mut child,
                setup_error.expect("setup error must be retained"),
            );
            let mut operation_error = Some(setup_error);
            join_finished_pump(output_pump.thread, "child stdout", &mut operation_error);
            return Err(operation_error.expect("setup failure must be retained"));
        }
    };

    let mut correlator = Correlator::new(server, session, salt);
    let mut output_finished = false;
    let mut child_status = None;
    let mut operation_error = None;
    let mut recording_error = None;
    let mut input_cancelled = false;
    let mut output_cancelled = false;
    let mut frame_order = FrameOrder::default();

    loop {
        if child_status.is_none() {
            match child.try_wait() {
                Ok(status) => child_status = status,
                Err(error) => {
                    remember_operation_error(
                        &mut operation_error,
                        anyhow::Error::from(error).context("checking child process status"),
                    );
                }
            }
        }

        if child_status.is_some() && !input_cancelled {
            input_cancelled = true;
            cancel_pump(&input_pump, "agent stdin", &mut operation_error);
        }

        if child_status.is_some() && output_finished {
            for event in rx.try_iter() {
                if let Some(error) = handle_event(
                    event,
                    &mut frame_order,
                    &mut correlator,
                    &mut store,
                    &mut output_finished,
                    &mut recording_error,
                ) {
                    remember_operation_error(&mut operation_error, error);
                }
            }
            break;
        }

        if operation_error.is_some() {
            if !input_cancelled {
                input_cancelled = true;
                cancel_pump(&input_pump, "agent stdin", &mut operation_error);
            }
            if !output_cancelled {
                output_cancelled = true;
                cancel_pump(&output_pump, "child stdout", &mut operation_error);
            }
            if child_status.is_none() {
                let primary = operation_error
                    .take()
                    .expect("operation error was checked above");
                let (status, error) =
                    cleanup_after_error(&mut child, primary, "cleaning up after operation failure");
                child_status = status;
                operation_error = Some(error);
            }
            break;
        }

        match rx.recv_timeout(CHILD_POLL_INTERVAL) {
            Ok(event) => {
                if let Some(error) = handle_event(
                    event,
                    &mut frame_order,
                    &mut correlator,
                    &mut store,
                    &mut output_finished,
                    &mut recording_error,
                ) {
                    remember_operation_error(&mut operation_error, error);
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
    }

    let cancel_input = !input_cancelled;
    let cancel_output = operation_error.is_some() && !output_cancelled;
    shutdown_pumps_and_drain(
        input_pump,
        output_pump,
        events,
        rx,
        ShutdownCancellation {
            input: cancel_input,
            output: cancel_output,
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

    if let Some(error) = operation_error {
        return Err(error);
    }
    if let Some(error) = recording_error {
        return Err(error);
    }

    child_exit_code(child_status.context("child exited without a status")?)
}

#[cfg(any(unix, windows))]
fn spawn_input_pump(
    child_stdin: std::process::ChildStdin,
    events: Arc<Mutex<EventSender>>,
) -> io::Result<PumpHandle> {
    let (cancel, cancellation) = cancellation_pair()?;
    let thread = spawn_pump(Direction::Outbound, events.clone(), move || {
        let stdin = io::stdin();
        let reader = CancellableReader::new(stdin.lock(), cancellation);
        let mut source = BufReader::new(reader);
        let mut destination = child_stdin;

        loop {
            let frame = match read_frame(&mut source) {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(_) if source.get_ref().is_cancelled() => break,
                Err(error) => return Err(contextual_io(error, "reading agent stdin")),
            };
            let started = Instant::now();
            let observed_ms = now_ms();
            let Frame {
                raw,
                value,
                parse_us,
            } = frame;
            let sequence = reserve_frame(
                &events,
                FrameMetadata {
                    direction: Direction::Outbound,
                    value,
                    observed_ms,
                },
            )?;
            if !forward_or_cancel(&mut destination, &raw, "writing child stdin", || {
                source.get_ref().is_cancelled()
            })? {
                break;
            }
            complete_frame(&events, sequence, started, parse_us)?;
        }
        Ok(())
    })?;
    Ok(PumpHandle { thread, cancel })
}

#[cfg(any(unix, windows))]
fn spawn_output_pump(
    child_stdout: std::process::ChildStdout,
    events: Arc<Mutex<EventSender>>,
) -> io::Result<PumpHandle> {
    let (cancel, cancellation) = cancellation_pair()?;
    let thread = spawn_pump(Direction::Inbound, events.clone(), move || {
        let reader = CancellableReader::new(child_stdout, cancellation);
        let mut source = BufReader::new(reader);
        let stdout = io::stdout();
        let mut destination = stdout.lock();

        loop {
            let frame = match read_frame(&mut source) {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(_) if source.get_ref().is_cancelled() => break,
                Err(error) => return Err(contextual_io(error, "reading child stdout")),
            };
            let started = Instant::now();
            let observed_ms = now_ms();
            let Frame {
                raw,
                value,
                parse_us,
            } = frame;
            let sequence = reserve_frame(
                &events,
                FrameMetadata {
                    direction: Direction::Inbound,
                    value,
                    observed_ms,
                },
            )?;
            if !forward_or_cancel(&mut destination, &raw, "writing agent stdout", || {
                source.get_ref().is_cancelled()
            })? {
                break;
            }
            complete_frame(&events, sequence, started, parse_us)?;
        }
        Ok(())
    })?;
    Ok(PumpHandle { thread, cancel })
}

fn spawn_pump<F>(
    direction: Direction,
    events: Arc<Mutex<EventSender>>,
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
            let _ = lock_events(&events).finished(direction, failure);
            result
        })
}

fn forward<W: Write>(destination: &mut W, raw: &[u8], context: &str) -> io::Result<()> {
    destination
        .write_all(raw)
        .map_err(|error| contextual_io(error, context))?;
    destination
        .flush()
        .map_err(|error| contextual_io(error, context))
}

fn forward_or_cancel<W: Write>(
    destination: &mut W,
    raw: &[u8],
    context: &str,
    cancelled: impl FnOnce() -> bool,
) -> io::Result<bool> {
    match forward(destination, raw, context) {
        Ok(()) => Ok(true),
        Err(_) if cancelled() => Ok(false),
        Err(error) => Err(error),
    }
}

fn reserve_frame(events: &Mutex<EventSender>, metadata: FrameMetadata) -> io::Result<u64> {
    lock_events(events).reserve(metadata)
}

fn complete_frame(
    events: &Mutex<EventSender>,
    sequence: u64,
    started: Instant,
    parse_us: u64,
) -> io::Result<()> {
    lock_events(events).forwarded(
        sequence,
        parse_us.saturating_add(started.elapsed().as_micros() as u64),
    )
}

fn handle_event(
    event: Event,
    frame_order: &mut FrameOrder,
    correlator: &mut Correlator,
    store: &mut Store,
    output_finished: &mut bool,
    recording_error: &mut Option<anyhow::Error>,
) -> Option<anyhow::Error> {
    let event_error = match event {
        Event::Reserved { sequence, metadata } => frame_order.reserve(sequence, metadata).err(),
        Event::Forwarded {
            sequence,
            shim_self_us,
        } => frame_order.complete(sequence, shim_self_us).err(),
        Event::Finished { direction, failure } => {
            frame_order.retire_unfinished(direction);
            if direction == Direction::Inbound {
                *output_finished = true;
            }
            failure.map(pump_failure_error)
        }
    };

    for frame in frame_order.drain_ready() {
        let recording_started = Instant::now();
        let record = match (frame.direction, frame.value) {
            (Direction::Outbound, Some(value)) => {
                correlator.on_outbound_with_overhead(&value, frame.observed_ms, frame.shim_self_us);
                None
            }
            (Direction::Inbound, Some(value)) => correlator.on_inbound(&value, frame.observed_ms),
            (direction, None) => Some(correlator.on_unparsed(direction.label(), frame.observed_ms)),
        };
        if let Some(mut record) = record {
            record.shim_self_us = record
                .shim_self_us
                .saturating_add(frame.shim_self_us)
                .saturating_add(recording_started.elapsed().as_micros() as u64);
            append_record(store, &record, recording_error);
        }
    }

    event_error
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

fn remember_operation_error(target: &mut Option<anyhow::Error>, error: anyhow::Error) {
    *target = Some(match target.take() {
        Some(primary) => combine_errors(primary, "handling another operation failure", error),
        None => error,
    });
}

fn pump_failure_error(failure: PumpFailure) -> anyhow::Error {
    anyhow!(
        "{} stdio pump failed ({:?}): {}",
        failure.direction.label(),
        failure.kind,
        failure.message
    )
}

#[cfg(any(unix, windows))]
fn cancel_pump(pump: &PumpHandle, label: &str, operation_error: &mut Option<anyhow::Error>) {
    if let Err(error) = pump.cancel() {
        remember_operation_error(
            operation_error,
            anyhow::Error::from(error).context(format!("cancelling {label} pump")),
        );
    }
}

#[cfg(any(unix, windows))]
fn shutdown_pumps_and_drain<F>(
    input_pump: PumpHandle,
    output_pump: PumpHandle,
    events: Arc<Mutex<EventSender>>,
    rx: Receiver<Event>,
    cancellation: ShutdownCancellation,
    operation_error: &mut Option<anyhow::Error>,
    mut handle_remaining_event: F,
) where
    F: FnMut(Event) -> Option<anyhow::Error>,
{
    if cancellation.input {
        cancel_pump(&input_pump, "agent stdin", operation_error);
    }
    if cancellation.output {
        cancel_pump(&output_pump, "child stdout", operation_error);
    }

    join_finished_pump(output_pump.thread, "child stdout", operation_error);
    join_finished_pump(input_pump.thread, "agent stdin", operation_error);

    drop(events);
    for event in rx {
        if let Some(error) = handle_remaining_event(event) {
            remember_operation_error(operation_error, error);
        }
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
        remember_operation_error(
            operation_error,
            error.context(format!("joining {label} pump")),
        );
    }
}

fn cleanup_setup_failure(child: &mut Child, setup_error: anyhow::Error) -> anyhow::Error {
    match terminate_and_reap(child) {
        Ok(_) => setup_error,
        Err(cleanup_error) => {
            combine_errors(setup_error, "terminating and reaping child", cleanup_error)
        }
    }
}

fn terminate_and_reap(child: &mut Child) -> anyhow::Result<ExitStatus> {
    terminate_and_reap_process(child)
}

trait ProcessControl {
    type Status;

    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<Self::Status>;
}

impl ProcessControl for Child {
    type Status = ExitStatus;

    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }

    fn wait(&mut self) -> io::Result<Self::Status> {
        Child::wait(self)
    }
}

fn terminate_and_reap_process<P: ProcessControl>(process: &mut P) -> anyhow::Result<P::Status> {
    let kill_error = match process.kill() {
        Ok(()) => None,
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => None,
        Err(error) => Some(anyhow::Error::from(error).context("terminating child")),
    };
    let wait_result = process.wait().context("reaping child");

    match (kill_error, wait_result) {
        (None, result) => result,
        (Some(error), Ok(_)) => Err(error),
        (Some(kill_error), Err(wait_error)) => Err(combine_errors(
            kill_error,
            "reaping child after termination failure",
            wait_error,
        )),
    }
}

fn cleanup_after_error<P: ProcessControl>(
    process: &mut P,
    primary: anyhow::Error,
    cleanup_context: &str,
) -> (Option<P::Status>, anyhow::Error) {
    match terminate_and_reap_process(process) {
        Ok(status) => (Some(status), primary),
        Err(cleanup_error) => (
            None,
            combine_errors(primary, cleanup_context, cleanup_error),
        ),
    }
}

fn combine_errors(
    primary: anyhow::Error,
    secondary_context: &str,
    secondary: anyhow::Error,
) -> anyhow::Error {
    anyhow!("{primary:#}; also failed while {secondary_context}: {secondary:#}")
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

fn lock_events(events: &Mutex<EventSender>) -> MutexGuard<'_, EventSender> {
    events
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn event_receiver_closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "recording event receiver closed")
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

#[cfg(test)]
mod tests;
