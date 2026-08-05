use std::sync::LazyLock;

use regex::Regex;

const TEMPLATE: &str = "{message}";

static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
        .expect("valid uuid regex")
});
// The double-quote arm is unrestricted: a stray `"` mid-word is not a thing
// this domain's messages produce. The single-quote arm is restricted to an
// opening quote at a token boundary (start-of-string or whitespace) so a
// contraction like "can't" is not mistaken for an opening quote that then
// swallows everything up to the next contraction's apostrophe (e.g. in
// "can't connect, won't retry"). The boundary is captured in group 1 so it
// can be preserved in the replacement rather than eaten by the match.
static QUOTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""[^"]*"|(^|[\s(\[{])'[^']*'"#).expect("valid quoted-run regex")
});
static SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[a-z][a-z0-9+.\-]*://\S+").expect("valid scheme regex"));
// Requires a token boundary before the slash (start-of-string, whitespace, or
// an opening quote/bracket/paren) so a namespaced method or path-shaped word
// like "tools/call" is not mistaken for an absolute path — unanchored, that
// collapsed "tools/call failed" and "tools/list failed" to the same
// skeleton. The boundary is captured in group 1 and preserved in the
// replacement.
static ABS_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(^|[\s"'(\[])/\S+"#).expect("valid path regex"));
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("valid digit regex"));
static WHITESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("valid whitespace regex"));

/// Returns a stable, content-free template for any human error message.
pub fn normalize(_message: &str) -> String {
    TEMPLATE.to_owned()
}

/// Collapses variable content out of an error message so that two messages
/// describing the same defect produce the same skeleton: lowercase the
/// message, then replace UUIDs, quoted runs, `scheme://…` runs, absolute
/// paths, and digit runs with single-character placeholders, then collapse
/// whitespace runs and trim.
///
/// Used only as input to `fingerprint::template_id`; the skeleton itself is
/// never stored.
pub fn skeleton(message: &str) -> String {
    let lowered = message.to_lowercase();
    let step = UUID.replace_all(&lowered, "u");
    // "$1" re-inserts the captured token boundary (empty for the
    // double-quote arm, which has no such group) so the match itself is
    // replaced without eating the whitespace/bracket that justified it.
    let step = QUOTED.replace_all(&step, "${1}q");
    let step = SCHEME.replace_all(&step, "l");
    let step = ABS_PATH.replace_all(&step, "${1}p");
    let step = DIGITS.replace_all(&step, "0");
    let step = WHITESPACE.replace_all(&step, " ");
    step.trim().to_string()
}
