//! Defect class and server-side fix for a promoted failure group.

use crate::probe::FailureReason;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingClass {
    UnstableErrorCode,
    FalseSuccess,
    BlockedOptimalPath,
    RecoversOnRetry,
    RetryDidNotRecover,
    RecurringError,
}

/// What promotion observed about one failure group.
#[derive(Clone, Copy, Debug, Default)]
pub struct Evidence {
    pub distinct_codes: usize,
    pub false_success: bool,
    pub blocked_optimal_path: bool,
    pub all_retryable: bool,
    pub recovered: bool,
}

const ALL: [FindingClass; 6] = [
    FindingClass::UnstableErrorCode,
    FindingClass::FalseSuccess,
    FindingClass::BlockedOptimalPath,
    FindingClass::RecoversOnRetry,
    FindingClass::RetryDidNotRecover,
    FindingClass::RecurringError,
];

impl FindingClass {
    pub fn classify(evidence: Evidence) -> Self {
        if evidence.distinct_codes > 1 {
            Self::UnstableErrorCode
        } else if evidence.false_success {
            Self::FalseSuccess
        } else if evidence.blocked_optimal_path {
            Self::BlockedOptimalPath
        } else if evidence.all_retryable && evidence.recovered {
            Self::RecoversOnRetry
        } else if evidence.all_retryable {
            Self::RetryDidNotRecover
        } else {
            Self::RecurringError
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnstableErrorCode => "unstable-error-code",
            Self::FalseSuccess => "false-success",
            Self::BlockedOptimalPath => "blocked-optimal-path",
            Self::RecoversOnRetry => "recovers-on-retry",
            Self::RetryDidNotRecover => "retry-did-not-recover",
            Self::RecurringError => "recurring-error",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::UnstableErrorCode => crate::remediation::hint(FailureReason::UnstableErrorCode),
            Self::FalseSuccess => {
                "the call reported success while the agent observed no effect; return a \
                 structured error when the operation did not happen"
            }
            Self::BlockedOptimalPath => {
                "the documented path was refused; make the documented path work or \
                 document the one that does"
            }
            Self::RecoversOnRetry => {
                "the error is transient and recovers on retry, so every occurrence costs \
                 the agent the recorded turns; fix the transient path or answer correctly \
                 the first time"
            }
            Self::RetryDidNotRecover => crate::remediation::hint(FailureReason::RetryDidNotRecover),
            Self::RecurringError => {
                "the same structured error recurs across sessions for this argument shape; \
                 reproduce with the shape, return a stable code, and attach a probe with \
                 `mcpeval generate`"
            }
        }
    }
}

impl FromSql for FindingClass {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let label = value.as_str()?;
        ALL.into_iter()
            .find(|class| class.as_str() == label)
            .ok_or_else(|| FromSqlError::Other(format!("unknown finding class {label:?}").into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_class_follows_from_its_evidence() {
        let cases = [
            (
                Evidence {
                    distinct_codes: 2,
                    ..Evidence::default()
                },
                FindingClass::UnstableErrorCode,
            ),
            (
                Evidence {
                    distinct_codes: 1,
                    false_success: true,
                    ..Evidence::default()
                },
                FindingClass::FalseSuccess,
            ),
            (
                Evidence {
                    blocked_optimal_path: true,
                    ..Evidence::default()
                },
                FindingClass::BlockedOptimalPath,
            ),
            (
                Evidence {
                    all_retryable: true,
                    recovered: true,
                    ..Evidence::default()
                },
                FindingClass::RecoversOnRetry,
            ),
            (
                Evidence {
                    all_retryable: true,
                    ..Evidence::default()
                },
                FindingClass::RetryDidNotRecover,
            ),
            (
                Evidence {
                    distinct_codes: 1,
                    recovered: true,
                    ..Evidence::default()
                },
                FindingClass::RecurringError,
            ),
        ];
        for (evidence, expected) in cases {
            assert_eq!(FindingClass::classify(evidence), expected, "{evidence:?}");
        }
    }

    #[test]
    fn precedence_runs_unstable_false_success_blocked_recovers_then_retry() {
        let everything = Evidence {
            distinct_codes: 3,
            false_success: true,
            blocked_optimal_path: true,
            all_retryable: true,
            recovered: true,
        };
        let ladder = [
            FindingClass::UnstableErrorCode,
            FindingClass::FalseSuccess,
            FindingClass::BlockedOptimalPath,
            FindingClass::RecoversOnRetry,
        ];
        let mut evidence = everything;
        for expected in ladder {
            assert_eq!(FindingClass::classify(evidence), expected);
            if evidence.distinct_codes > 1 {
                evidence.distinct_codes = 1;
            } else if evidence.false_success {
                evidence.false_success = false;
            } else {
                evidence.blocked_optimal_path = false;
            }
        }
        evidence.recovered = false;
        assert_eq!(
            FindingClass::classify(evidence),
            FindingClass::RetryDidNotRecover
        );
    }

    #[test]
    fn labels_round_trip_and_hints_reuse_probe_remediation() {
        for class in ALL {
            let value = rusqlite::types::Value::Text(class.as_str().into());
            assert_eq!(
                FindingClass::column_result(ValueRef::from(&value)).unwrap(),
                class
            );
            assert_eq!(
                serde_json::to_value(class).unwrap(),
                serde_json::json!(class.as_str())
            );
            assert!(!class.hint().is_empty());
        }
        assert_eq!(
            FindingClass::UnstableErrorCode.hint(),
            crate::remediation::hint(FailureReason::UnstableErrorCode)
        );
        assert_eq!(
            FindingClass::RetryDidNotRecover.hint(),
            crate::remediation::hint(FailureReason::RetryDidNotRecover)
        );
        let unknown = rusqlite::types::Value::Text("flaky".into());
        assert!(FindingClass::column_result(ValueRef::from(&unknown)).is_err());
    }
}
