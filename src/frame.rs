use std::io::BufRead;

#[derive(Debug, Clone)]
pub struct Frame {
    /// Exact bytes read, including the trailing newline when present.
    pub raw: Vec<u8>,
    /// Parsed JSON, or None when the line was not valid JSON.
    pub value: Option<serde_json::Value>,
}

/// Reads one newline-delimited message. Returns Ok(None) at end of input.
pub fn read_frame<R: BufRead>(r: &mut R) -> std::io::Result<Option<Frame>> {
    let mut raw = Vec::new();
    let n = r.read_until(b'\n', &mut raw)?;
    if n == 0 {
        return Ok(None);
    }
    let value = serde_json::from_slice(&raw).ok();
    Ok(Some(Frame { raw, value }))
}
