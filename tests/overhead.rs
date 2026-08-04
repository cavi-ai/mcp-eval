use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn p95(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    let idx = ((samples.len() as f64 * 0.95) as usize).min(samples.len() - 1);
    samples[idx]
}

fn round_trips(stdin: &mut ChildStdin, stdout: &mut BufReader<ChildStdout>, n: usize) -> Vec<u128> {
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let msg = format!(
            r#"{{"jsonrpc":"2.0","id":{i},"method":"tools/call","params":{{"name":"navigate","arguments":{{"waitUntil":"commit"}}}}}}"#
        );
        let started = Instant::now();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        samples.push(started.elapsed().as_micros());
    }
    samples
}

fn spawn(direct: bool, home: &std::path::Path) -> Child {
    if direct {
        Command::new("python3")
            .args(["tests/fixtures/echo_server.py"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    } else {
        Command::new(bin())
            .args([
                "shim",
                "--server",
                "bench",
                "--",
                "python3",
                "tests/fixtures/echo_server.py",
            ])
            .env("MCPEVAL_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    }
}

#[test]
fn shim_adds_under_two_milliseconds_at_p95() {
    let home = std::env::temp_dir().join(format!("mcpeval-bench-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();

    let mut direct = spawn(true, &home);
    let mut d_in = direct.stdin.take().unwrap();
    let mut d_out = BufReader::new(direct.stdout.take().unwrap());
    let baseline = p95(round_trips(&mut d_in, &mut d_out, 200));
    drop(d_in);
    let _ = direct.wait();

    let mut shimmed = spawn(false, &home);
    let mut s_in = shimmed.stdin.take().unwrap();
    let mut s_out = BufReader::new(shimmed.stdout.take().unwrap());
    let through = p95(round_trips(&mut s_in, &mut s_out, 200));
    drop(s_in);
    let _ = shimmed.wait();

    let added = through.saturating_sub(baseline);
    eprintln!("overhead p95: baseline={baseline}us shimmed={through}us added={added}us");
    assert!(
        added < 2_000,
        "shim added {added}us at p95 (baseline {baseline}us, shimmed {through}us)"
    );
}
