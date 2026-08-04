use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const READ_TIMEOUT: Duration = Duration::from_secs(2);
const EXIT_TIMEOUT: Duration = Duration::from_secs(2);

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

    fn round_trips(&mut self, n: usize) -> Result<Vec<u128>, String> {
        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let msg = format!(
                r#"{{"jsonrpc":"2.0","id":{i},"method":"tools/call","params":{{"name":"navigate","arguments":{{"waitUntil":"commit"}}}}}}"#
            );
            let started = Instant::now();
            let stdin = self
                .stdin
                .as_mut()
                .ok_or_else(|| "child stdin was already closed".to_owned())?;
            writeln!(stdin, "{msg}").map_err(|error| format!("writing request {i}: {error}"))?;
            stdin
                .flush()
                .map_err(|error| format!("flushing request {i}: {error}"))?;

            let line = match self.lines.recv_timeout(READ_TIMEOUT) {
                Ok(ReaderEvent::Line(line)) if !line.is_empty() => line,
                Ok(ReaderEvent::Line(_)) => return Err(format!("response {i} was empty")),
                Ok(ReaderEvent::Eof) => return Err(format!("response {i} reached EOF")),
                Ok(ReaderEvent::Error(error)) => {
                    return Err(format!("reading response {i}: {error}"));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!("response {i} exceeded {READ_TIMEOUT:?}"));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(format!("response reader disconnected before response {i}"));
                }
            };
            validate_response(&line, i)?;
            samples.push(started.elapsed().as_micros());
        }
        Ok(samples)
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
                let kill_error = self.child.kill().err();
                let reap_result = self.child.wait();
                return Err(match (kill_error, reap_result) {
                    (None, Ok(status)) => {
                        format!("child did not exit within {EXIT_TIMEOUT:?}; killed and reaped {status}")
                    }
                    (Some(kill_error), Ok(status)) => format!(
                        "child did not exit within {EXIT_TIMEOUT:?}; kill failed ({kill_error}), reaped {status}"
                    ),
                    (None, Err(reap_error)) => format!(
                        "child did not exit within {EXIT_TIMEOUT:?}; kill succeeded but reap failed ({reap_error})"
                    ),
                    (Some(kill_error), Err(reap_error)) => format!(
                        "child did not exit within {EXIT_TIMEOUT:?}; kill failed ({kill_error}) and reap failed ({reap_error})"
                    ),
                });
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
    let baseline = p95(direct.round_trips(200).expect("direct round trips"));
    direct
        .finish()
        .expect("direct child must exit successfully");

    let mut shimmed = ChildProcess::spawn(false, home.path());
    let through = p95(shimmed.round_trips(200).expect("shimmed round trips"));
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
