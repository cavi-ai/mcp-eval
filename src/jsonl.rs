//! Bounded journal records with strict complete-line parsing and tolerant tails.
use anyhow::Context;
use std::io::{BufRead, Read, Write};

pub(crate) const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;

/// Encode a complete bounded record before touching its journal. Count encoded
/// UTF-8 bytes, escaping, and the newline; never truncate an oversized record.
pub(crate) fn encode_record(record: &impl serde::Serialize) -> anyhow::Result<Vec<u8>> {
    let mut buffer = RecordBuffer(Vec::new());
    serde_json::to_writer(&mut buffer, record).context("encoding journal record")?;
    buffer.write_all(b"\n")?;
    Ok(buffer.0)
}

struct RecordBuffer(Vec<u8>);

impl Write for RecordBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_RECORD_BYTES - self.0.len() {
            return Err(std::io::Error::other("journal record exceeds 4 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Read one bounded physical line. Consumers choose their own EOF-tail policy.
pub(crate) fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> anyhow::Result<usize> {
    line.clear();
    let count = reader
        .by_ref()
        .take((MAX_RECORD_BYTES + 1) as u64)
        .read_until(b'\n', line)?;
    anyhow::ensure!(count <= MAX_RECORD_BYTES, "journal record exceeds 4 MiB");
    Ok(count)
}

pub(crate) fn visit<T: serde::de::DeserializeOwned>(
    reader: &mut impl BufRead,
    mut accept: impl FnMut(T) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    visit_complete(reader, &mut accept).map(|_| ())
}

/// Return the consumed complete-line prefix, excluding an unfinished tail.
pub(crate) fn visit_complete<T: serde::de::DeserializeOwned>(
    reader: &mut impl BufRead,
    mut accept: impl FnMut(T) -> anyhow::Result<()>,
) -> anyhow::Result<u64> {
    let mut line = Vec::new();
    let mut number = 0;
    let mut complete = 0;
    loop {
        let count = read_line(reader, &mut line)?;
        if count == 0 {
            return Ok(complete);
        }
        if line.last() != Some(&b'\n') {
            return Ok(complete);
        }
        number += 1;
        complete += count as u64;
        if !line.iter().all(u8::is_ascii_whitespace) {
            let value = serde_json::from_slice(&line)
                .with_context(|| format!("invalid journal record on line {number}"))?;
            accept(value)?;
        }
    }
}
