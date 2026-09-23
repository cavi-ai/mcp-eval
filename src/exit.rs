//! The CLI exit-code contract, so CI can tell a red server from a broken
//! run:
//!
//! - 0: the command succeeded; every selected case passed.
//! - 1 ([`VERDICT`]): the evaluation completed and at least one case
//!   failed its probe (or a gate such as `diff --fail-on-regression` fired).
//! - 2 ([`USAGE`]): invalid arguments, manifest, or input document; nothing
//!   was evaluated.
//! - 3 ([`INFRASTRUCTURE`]): the evaluation could not complete — the server
//!   was unreachable, a case lost its transport, or local I/O failed. A
//!   probe run that reaches this state still emits its report.

use std::fmt;

pub const VERDICT: i32 = 1;
pub const USAGE: i32 = 2;
pub const INFRASTRUCTURE: i32 = 3;

/// Marks an error as a usage error without changing how it prints: it
/// displays as the wrapped error and continues its cause chain.
#[derive(Debug)]
pub struct Usage(anyhow::Error);

impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for Usage {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

/// Classify `error` as a usage error (exit 2).
pub fn usage(error: anyhow::Error) -> anyhow::Error {
    anyhow::Error::new(Usage(error))
}

/// The exit code for a command that failed with `error`.
pub fn code(error: &anyhow::Error) -> i32 {
    if error.downcast_ref::<Usage>().is_some() {
        USAGE
    } else {
        INFRASTRUCTURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn usage_survives_context_and_prints_like_the_wrapped_error() {
        let inner = anyhow::anyhow!("manifest version must be 1").context("loading manifest");
        let expected = format!("{inner:?}");
        let marked = usage(inner);
        assert_eq!(format!("{marked:?}"), expected);
        let wrapped = Err::<(), _>(marked).context("endpoint a").unwrap_err();
        assert_eq!(code(&wrapped), USAGE);
        assert_eq!(code(&anyhow::anyhow!("spawn failed")), INFRASTRUCTURE);
    }
}
