//! Curated remediation guidance for fixed failure reasons.
//!
//! A red gate that only names a label is a diagnosis without a
//! prescription. Every reason maps to one concrete fix the operator can
//! apply without reading the source. Hints describe *server-side* fixes;
//! they never assume the probe was wrong.

use crate::probe::FailureReason;

/// One-line prescription for a failing case.
pub fn hint(reason: FailureReason) -> &'static str {
    match reason {
        FailureReason::UnexpectedOutcome => {
            "the call returned the opposite outcome of the expectation; check the \
             tool's contract — a read-only call must succeed for `outcome: ok` and \
             must surface a structured JSON-RPC error for `outcome: error`"
        }
        FailureReason::MissingField => {
            "the response is missing a machine-readable field the manifest declared; \
             return every field from `required_result_fields` as a top-level key of \
             the structured result, not inside prose"
        }
        FailureReason::ValueMismatch => {
            "a result field returned a different value than declared; align the \
             tool's actual output with the manifest's `equals` expectations or fix \
             the tool — do not loosen the expectation to make the gate green"
        }
        FailureReason::ErrorCodeMismatch => {
            "the structured error returned a different numeric code than declared; \
             pick one stable code for each failure class and return exactly that \
             code every time"
        }
        FailureReason::DiscoveryLimitExceeded => {
            "the catalog grew past the declared bounds; merge overlapping tools, \
             drop tools agents never call, or negotiate a larger budget — a sprawling \
             catalog taxes every session's context window"
        }
        FailureReason::TokenBudgetExceeded => {
            "the encoded catalog exceeds the token budget; shorten descriptions to \
             one sentence, keep only decision-relevant parameters in schemas, and \
             reserve long documentation for tool results or an explainer resource"
        }
        FailureReason::InvalidSchema => {
            "the input schema is not a coherent object; declare `type: object`, list \
             every `required` field in `properties`, and never require a field the \
             schema does not describe"
        }
        FailureReason::MissingRequiredArgument => {
            "the schema marks a field required but the manifest's naive arguments do \
             not supply it; either make the field optional with a default, or give \
             the probe a realistic value in the manifest"
        }
        FailureReason::ExpectedError => {
            "the manifest expects this call to fail and the tool succeeded; aim the \
             error-honesty case at a genuinely invalid input, or fix the tool if it \
             is succeeding on input it should reject"
        }
        FailureReason::UnstableErrorCode => {
            "the same failing call returned different error codes across attempts; \
             map each underlying cause to one stable numeric code and return it \
             deterministically"
        }
        FailureReason::RetryabilityMismatch => {
            "the `retryable` flag did not match the declared truth; set `retryable` \
             true only for errors a retry with identical input can plausibly survive"
        }
        FailureReason::RetryDidNotRecover => {
            "a retryable error never recovered within the declared attempts; either \
             fix the transient path or mark the error non-retryable so agents stop \
             burning calls on it"
        }
        FailureReason::FailureNotObserved => {
            "the state-recovery failure call succeeded, so there was nothing to \
             recover from; point `failure_tool` at an input that reliably errors"
        }
        FailureReason::RecoveryFailed => {
            "the recovery call errored; after repairing state the recovery tool must \
             succeed — check that failure_state is actually cleared before recovery \
             returns"
        }
        FailureReason::ValidationFailed => {
            "post-recovery validation errored; the session looked healed but the \
             validation call disagrees — verify the recovered state with the same \
             read a real agent would issue"
        }
        FailureReason::ContendedClientFailed => {
            "a second concurrent client failed while the first was mid-call; check \
             for global locks, single-threaded session assumptions, or shared-state \
             races under parallel access"
        }
        FailureReason::LatencyBudgetExceeded => {
            "a call exceeded the declared latency budget; profile the hot path (cold \
             starts, synchronous I/O, retries inside handlers) or renegotiate the \
             budget to p99 reality"
        }
        FailureReason::PaginationInvalidEntry => {
            "a page contained an entry without a tool-grammar name or an object \
             inputSchema; validate every entry at catalog-build time, not just the \
             first page"
        }
        FailureReason::PaginationDuplicateTool => {
            "the same tool name appeared on more than one page; paginate the catalog \
             without overlap and emit `nextCursor` only when unreturned entries remain"
        }
        FailureReason::PaginationStalledCursor => {
            "the cursor sequence never terminated within `max_pages`; emit no \
             `nextCursor` on the final page and never re-serve a page a cursor \
             already returned"
        }
        FailureReason::PayloadUnhandled => {
            "an oversized argument crashed, hung, or corrupted the transport; bound \
             input sizes at the handler edge and answer oversize with a structured \
             JSON-RPC error instead of dying"
        }
        FailureReason::SurfaceInvalidEnvelope => {
            "a declared resources/prompts surface returned a malformed listing; every \
             declared surface must answer `list` with its item array, even when empty"
        }
        FailureReason::SurfaceStalledCursor => {
            "a declared surface's cursor sequence never terminated; apply the same \
             pagination contract as tools/list to resources and prompts"
        }
        FailureReason::OutputSchemaDeclaredButMissing => {
            "the tool declares `outputSchema` but the response carried no \
             `structuredContent`; either populate it on every success or withdraw the \
             declaration"
        }
        FailureReason::OutputSchemaFieldMissing => {
            "`structuredContent` is missing a field the declared `outputSchema` marks \
             required; return the full declared shape on every success"
        }
        FailureReason::CancellationIgnored => {
            "the server completed the work and returned a result for a request the \
             client had cancelled; check for the cancellation notification, stop the \
             work, and never send a response for the cancelled request id"
        }
        FailureReason::CancellationErrored => {
            "the server answered a cancelled request with a JSON-RPC error; a \
             cancellation must leave the request id unresolved — drop the operation \
             instead of replying"
        }
        FailureReason::CancellationUnsupportedTransport => {
            "the cancellation probe requires a stdio target; Streamable HTTP answers \
             the call POST synchronously and cannot observe a mid-flight cancellation"
        }
    }
}
