use std::sync::LazyLock;

use regex::Regex;

const TEMPLATE: &str = "{message}";

static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
        .expect("valid uuid regex")
});
static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""[^"]*"|'[^']*'"#).expect("valid quoted-run regex"));
static SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[a-z][a-z0-9+.\-]*://\S+").expect("valid scheme regex"));
static ABS_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/\S+").expect("valid path regex"));
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
    let step = QUOTED.replace_all(&step, "q");
    let step = SCHEME.replace_all(&step, "l");
    let step = ABS_PATH.replace_all(&step, "p");
    let step = DIGITS.replace_all(&step, "0");
    let step = WHITESPACE.replace_all(&step, " ");
    step.trim().to_string()
}
