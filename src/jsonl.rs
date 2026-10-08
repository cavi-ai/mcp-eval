//! Bounded journal records with strict complete-line parsing and tolerant tails.
use anyhow::Context;
use std::io::{BufRead, Read};

pub(crate) const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;

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
