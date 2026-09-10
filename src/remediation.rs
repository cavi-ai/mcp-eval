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
            "the server answered a cancelled request with an error that shows no \
             cancellation awareness; observe notifications/cancelled for in-flight \
             requests and stop the work instead of replying with an unrelated error"
        }
        FailureReason::NegotiationEchoedUnknown => {
            "the server echoed a protocol version it was never asked to support; \
             per the spec an initialize with an unknown version must be answered \
             with the server's own latest supported version, never the requested one"
        }
        FailureReason::NegotiationInvalidVersion => {
            "the server replied to initialize with a protocol version that is not \
             a date-shaped version (YYYY-MM-DD), or failed to echo the supported \
             version on the plain handshake; return one concrete date-versioned value"
        }
        FailureReason::NegotiationInconsistentSupport => {
            "the server advertised a protocol version it then refused on a second \
             handshake; keep the negotiated-version table in sync with what \
             initialize actually accepts"
        }
        FailureReason::SamplingInvalidRequest => {
            "the tool call failed after the client answered sampling/createMessage; \
             the server's sampling client must send a well-formed createMessage \
             request (messages array, systemPrompt optional) and accept the reply shape"
        }
        FailureReason::SamplingRequestFlood => {
            "the server issued more sampling/createMessage requests during one tool \
             call than the declared bound; batch sampling work, cache repeated \
             prompts, or renegotiate the budget"
        }
        FailureReason::SamplingStalledCall => {
            "the tool call never completed while a sampling request was outstanding; \
             the server must proceed when the client answers sampling/createMessage \
             instead of waiting forever"
        }
        FailureReason::ElicitationInvalidRequest => {
            "the tool call failed after the client answered elicitation/create; the \
             server's elicitation request must carry a message and a requestedSchema \
             object, and must accept the declared action reply shape"
        }
        FailureReason::ElicitationRequestFlood => {
            "the server issued more elicitation/create requests during one tool call \
             than the declared bound; ask once per decision point instead of \
             re-eliciting in a loop"
        }
        FailureReason::ElicitationStalledCall => {
            "the tool call never completed while an elicitation request was \
             outstanding; the server must proceed on the client's action reply \
             (accept/decline/cancel) instead of waiting forever"
        }
        FailureReason::ResourceUnreadable => {
            "the declared resource URI could not be read; list the URI in \
             resources/list and answer resources/read for every URI the server \
             claims to expose"
        }
        FailureReason::SubscriptionRejected => {
            "the server rejected resources/unsubscribe after a successful subscribe; \
             keep the subscription lifecycle symmetric — every accepted subscribe \
             must accept a matching unsubscribe"
        }
        FailureReason::SubscriptionNotificationMissing => {
            "no notifications/resources/updated arrived for the subscribed URI after \
             the trigger tool ran; emit the update notification on state change for \
             every subscribed URI, or do not declare resources.subscribe"
        }
    }
}
