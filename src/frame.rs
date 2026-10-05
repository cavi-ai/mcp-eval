use std::io::{self, BufRead, Read, Write};
use std::time::Instant;

use serde_json::Value;

use crate::privacy;

/// Maximum stdio frame size, including its delimiter.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn read_line<R: BufRead>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut raw = Vec::new();
    r.take((MAX_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut raw)?;
    if raw.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stdio frame exceeds 4 MiB",
        ));
    }
    Ok((!raw.is_empty()).then_some(raw))
}

pub(crate) fn encode(value: &Value) -> io::Result<Vec<u8>> {
    struct Buffer(Vec<u8>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > MAX_FRAME_BYTES - self.0.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "stdio frame exceeds 4 MiB",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Buffer(Vec::new());
    serde_json::to_writer(&mut buffer, value).map_err(io::Error::other)?;
    buffer.write_all(b"\n")?;
    Ok(buffer.0)
}

#[derive(Debug, Clone)]
pub struct Frame {
    /// Exact bytes read, including the trailing newline when present.
    pub raw: Vec<u8>,
    /// Parsed JSON, or None when the line was not valid JSON.
    pub value: Option<serde_json::Value>,
    /// CPU time spent parsing and structurally validating this frame. Blocking
    /// input time is deliberately excluded.
    pub parse_us: u64,
}

/// Reads one newline-delimited message. Returns Ok(None) at end of input.
pub fn read_frame<R: BufRead>(r: &mut R) -> std::io::Result<Option<Frame>> {
    let Some(raw) = read_line(r)? else {
        return Ok(None);
    };
    let started = Instant::now();
    let value = serde_json::from_slice(&raw)
        .ok()
        .filter(is_json_rpc_message);
    let parse_us = started.elapsed().as_micros() as u64;
    Ok(Some(Frame {
        raw,
        value,
        parse_us,
    }))
}

fn is_json_rpc_message(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return false;
    }
    let has_method = object.contains_key("method");
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    if has_method {
        if has_result || has_error {
            return false;
        }
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return false;
        };
        if !privacy::valid_method(method) {
            return false;
        }
        if object
            .get("params")
            .is_some_and(|params| !params.is_object() && !params.is_array())
        {
            return false;
        }
        return object.get("id").is_none_or(valid_id);
    }
    if has_result == has_error || !object.get("id").is_some_and(valid_id) {
        return false;
    }
    !has_error || valid_error(object.get("error").expect("error present"))
}

fn valid_id(value: &Value) -> bool {
    value.is_string() || value.is_number()
}

fn valid_error(value: &Value) -> bool {
    let Some(error) = value.as_object() else {
        return false;
    };
    error
        .get("code")
        .is_some_and(|code| code.as_i64().is_some())
        && error.get("message").is_some_and(Value::is_string)
}
