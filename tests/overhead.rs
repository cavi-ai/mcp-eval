use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const READ_TIMEOUT: Duration = Duration::from_secs(2);
const EXIT_TIMEOUT: Duration = Duration::from_secs(2);
const TIMED_SAMPLES: usize = 200;
const WARM_UP_ID: usize = TIMED_SAMPLES;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn p95(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    let idx = (samples.len() * 95).div_ceil(100) - 1;
    samples[idx]
}

#[test]
fn p95_uses_nearest_rank() {
    assert_eq!(p95((0..200).collect()), 189);
}

fn validate_response(line: &str, expected_id: usize) -> Result<(), String> {
    let response: serde_json::Value =
        serde_json::from_str(line).map_err(|error| format!("response was not JSON: {error}"))?;
    let actual_id = response
        .get("id")
        .ok_or_else(|| "response had no id".to_owned())?;
    let expected_id = serde_json::Value::from(expected_id as u64);
    if actual_id != &expected_id {
        return Err(format!(
            "response id was {actual_id}, expected {expected_id}"
        ));
    }
    Ok(())
}

#[test]
fn response_validation_requires_json_with_the_matching_id() {
    assert!(validate_response(r#"{"jsonrpc":"2.0","id":7,"result":{}}"#, 7).is_ok());
    assert!(validate_response("not json", 7).is_err());
    assert!(validate_response(r#"{"jsonrpc":"2.0","id":8,"result":{}}"#, 7).is_err());
}

#[test]
fn warm_up_id_does_not_overlap_timed_samples() {
    let timed_ids: Vec<_> = (0..TIMED_SAMPLES).collect();
    assert!(!timed_ids.contains(&WARM_UP_ID));
}

struct ExitAfterTryWait {
    kill_calls: usize,
    wait_calls: usize,
}

impl TimeoutProcess for ExitAfterTryWait {
    type Status = &'static str;

    fn kill(&mut self) -> std::io::Result<()> {
        self.kill_calls += 1;
        Err(std::io::Error::from(std::io::ErrorKind::InvalidInput))
    }

    fn wait(&mut self) -> std::io::Result<Self::Status> {
        self.wait_calls += 1;
        Ok("success")
    }
}

#[test]
fn timeout_cleanup_accepts_a_child_that_exited_before_kill() {
    let mut process = ExitAfterTryWait {
        kill_calls: 0,
        wait_calls: 0,
    };

    assert_eq!(
        kill_and_reap_after_timeout(&mut process).unwrap(),
        "success"
    );
    assert_eq!(process.kill_calls, 1);
    assert_eq!(process.wait_calls, 1);
}

struct TestHome(PathBuf);

impl TestHome {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mcpeval-bench-{}", uuid::Uuid::new_v4()));
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

enum ReaderEvent {
    Line(String),
    Eof,
    Error(String),
}

trait TimeoutProcess {
    type Status;

    fn kill(&mut self) -> std::io::Result<()>;
    fn wait(&mut self) -> std::io::Result<Self::Status>;
}

impl TimeoutProcess for Child {
    type Status = ExitStatus;

    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }

    fn wait(&mut self) -> std::io::Result<Self::Status> {
        Child::wait(self)
    }
}

fn kill_and_reap_after_timeout<P: TimeoutProcess>(process: &mut P) -> Result<P::Status, String> {
    let kill_error = match process.kill() {
        Ok(()) => None,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => None,
        Err(error) => Some(error),
    };
    let status = process
        .wait()
        .map_err(|error| format!("reaping child after timeout: {error}"))?;
    match kill_error {
        None => Ok(status),
        Some(error) => Err(format!("killing child after timeout: {error}")),
    }
}

struct ChildProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<ReaderEvent>,
    reader: Option<thread::JoinHandle<()>>,
}

impl ChildProcess {
    fn spawn(direct: bool, home: &Path) -> Self {
        let mut command = if direct {
            Command::new("python3")
        } else {
            Command::new(bin())
        };
        if direct {
            command.arg("tests/fixtures/echo_server.py");
        } else {
            command
                .args([
                    "shim",
                    "--server",
                    "bench",
                    "--",
                    "python3",
                    "tests/fixtures/echo_server.py",
                ])
                .env("MCPEVAL_HOME", home);
        }
        command.stdin(Stdio::piped()).stdout(Stdio::piped());

        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match stdout.read_line(&mut line) {
                    Ok(0) => {
                        let _ = sender.send(ReaderEvent::Eof);
                        return;
                    }
                    Ok(_) => {
                        if sender.send(ReaderEvent::Line(line)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(ReaderEvent::Error(error.to_string()));
                        return;
                    }
                }
            }
        });

        Self {
            child,
            stdin: Some(stdin),
            lines,
            reader: Some(reader),
        }
    }

    fn round_trip(&mut self, id: usize) -> Result<u128, String> {
        let msg = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"navigate","arguments":{{"waitUntil":"commit"}}}}}}"#
        );
        self.round_trip_message(id, &msg)
    }

    fn round_trip_message(&mut self, id: usize, msg: &str) -> Result<u128, String> {
        let started = Instant::now();
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "child stdin was already closed".to_owned())?;
        writeln!(stdin, "{msg}").map_err(|error| format!("writing request {id}: {error}"))?;
        stdin
            .flush()
            .map_err(|error| format!("flushing request {id}: {error}"))?;

        let line = match self.lines.recv_timeout(READ_TIMEOUT) {
            Ok(ReaderEvent::Line(line)) if !line.is_empty() => line,
            Ok(ReaderEvent::Line(_)) => return Err(format!("response {id} was empty")),
            Ok(ReaderEvent::Eof) => return Err(format!("response {id} reached EOF")),
            Ok(ReaderEvent::Error(error)) => return Err(format!("reading response {id}: {error}")),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(format!("response {id} exceeded {READ_TIMEOUT:?}"));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(format!("response reader disconnected before response {id}"));
            }
        };
        validate_response(&line, id)?;
        Ok(started.elapsed().as_micros())
    }

    fn round_trips(&mut self, n: usize) -> Result<Vec<u128>, String> {
        (0..n).map(|id| self.round_trip(id)).collect()
    }

    fn wait_for_exit(&mut self) -> Result<ExitStatus, String> {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) => {}
                Err(error) => return Err(format!("checking child status: {error}")),
            }
            if Instant::now() >= deadline {
                return kill_and_reap_after_timeout(&mut self.child);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(&mut self) -> Result<(), String> {
        self.stdin.take();
        let status = self.wait_for_exit()?;
        let _ = self.reader.take();
        if status.success() {
            Ok(())
        } else {
            Err(format!("child exited unsuccessfully: {status}"))
        }
    }
}

#[cfg(not(debug_assertions))]
#[test]
fn representative_large_frame_stays_under_the_same_two_millisecond_budget() {
    let home = TestHome::new();
    let padding = "x".repeat(32 * 1024);
    let message = format!(
        r#"{{"jsonrpc":"2.0","id":901,"method":"tools/call","params":{{"name":"navigate","arguments":{{"note":"{padding}"}}}}}}"#
    );

    let mut direct = ChildProcess::spawn(true, home.path());
    direct.round_trip(WARM_UP_ID).unwrap();
    let baseline = direct.round_trip_message(901, &message).unwrap();
    direct.finish().unwrap();

    let mut shimmed = ChildProcess::spawn(false, home.path());
    shimmed.round_trip(WARM_UP_ID).unwrap();
    let through = shimmed.round_trip_message(901, &message).unwrap();
    shimmed.finish().unwrap();

    let added = through.saturating_sub(baseline);
    assert!(
        added < 2_000,
        "large-frame shim added {added}us (baseline {baseline}us, shimmed {through}us)"
    );
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.stdin.take();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = self.reader.take();
    }
}

#[test]
fn shim_adds_under_two_milliseconds_at_p95() {
    let home = TestHome::new();

    let mut direct = ChildProcess::spawn(true, home.path());
    direct
        .round_trip(WARM_UP_ID)
        .expect("direct warm-up round trip");
    let baseline = p95(direct
        .round_trips(TIMED_SAMPLES)
        .expect("direct timed round trips"));
    direct
        .finish()
        .expect("direct child must exit successfully");

    let mut shimmed = ChildProcess::spawn(false, home.path());
    shimmed
        .round_trip(WARM_UP_ID)
        .expect("shimmed warm-up round trip");
    let through = p95(shimmed
        .round_trips(TIMED_SAMPLES)
        .expect("shimmed timed round trips"));
    shimmed
        .finish()
        .expect("shimmed child must exit successfully");

    let added = through.saturating_sub(baseline);
    eprintln!("overhead p95: baseline={baseline}us shimmed={through}us added={added}us");
    assert!(
        added < 2_000,
        "shim added {added}us at p95 (baseline {baseline}us, shimmed {through}us)"
    );
}
