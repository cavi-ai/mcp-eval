use sha2::{Digest, Sha256};

const MAX_LABEL: usize = 128;

pub fn opaque_session(value: &str) -> String {
    if value.len() == 72
        && value.starts_with("session:")
        && value[8..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return value.to_ascii_lowercase();
    }
    let digest = Sha256::digest(value.as_bytes());
    format!("session:{digest:x}")
}

pub fn valid_server(value: &str) -> bool {
    valid_segments(value, false)
}

pub fn valid_method(value: &str) -> bool {
    valid_segments(value, true)
}

pub fn valid_tool(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LABEL
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}

const MAX_IDENTIFIER: usize = 64;

/// An identifier-shaped error code: non-empty, at most 64 bytes, starting
/// with a letter and containing only `[A-Za-z0-9_.:-]` afterward. These are
/// symbolic labels rather than prose, so they may be kept verbatim instead
/// of being reduced to a length bucket.
pub fn valid_identifier(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_IDENTIFIER {
        return false;
    }
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}

fn valid_segments(value: &str, slash: bool) -> bool {
    if value.is_empty() || value.len() > MAX_LABEL {
        return false;
    }
    value.split(if slash { '/' } else { '\0' }).all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
    })
}
