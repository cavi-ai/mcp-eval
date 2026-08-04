use mcpeval::shape::{self, EnumIndex};
use serde_json::json;

fn enums_with_wait_until() -> EnumIndex {
    let mut idx = EnumIndex::new();
    idx.learn(
        "navigate",
        &json!({
            "type": "object",
            "properties": {
                "waitUntil": { "type": "string", "enum": ["commit", "networkIdle"] },
                "url": { "type": "string" }
            }
        }),
    );
    idx
}

#[test]
fn scalars_and_strings_are_shaped_not_stored() {
    let idx = EnumIndex::new();
    let out = shape::of(
        &json!({ "note": "call me at 555-0100", "count": 3, "ok": true, "nothing": null }),
        "anything",
        &idx,
    );
    assert_eq!(out["note"], "str<32");
    assert_eq!(out["count"], "num:3");
    assert_eq!(out["ok"], "bool:true");
    assert_eq!(out["nothing"], "null");
    let text = out.to_string();
    assert!(!text.contains("555"), "payload leaked into shape: {text}");
}

#[test]
fn schema_declared_enums_keep_their_value() {
    let idx = enums_with_wait_until();
    let out = shape::of(&json!({ "waitUntil": "networkIdle" }), "navigate", &idx);
    assert_eq!(out["waitUntil"], "enum:networkIdle");
}

#[test]
fn a_string_that_matches_an_enum_of_a_different_tool_is_not_kept() {
    let idx = enums_with_wait_until();
    let out = shape::of(&json!({ "waitUntil": "networkIdle" }), "click", &idx);
    assert_eq!(out["waitUntil"], "str<32");
}

#[test]
fn urls_keep_only_the_domain() {
    let idx = EnumIndex::new();
    let out = shape::of(
        &json!({ "url": "https://www.example.com/a/secret/path?token=abc" }),
        "navigate",
        &idx,
    );
    assert_eq!(out["url"], "url:example.com");
    let text = out.to_string();
    assert!(!text.contains("secret") && !text.contains("token"));
}

#[test]
fn uuids_are_labelled_not_stored() {
    let idx = EnumIndex::new();
    let out = shape::of(
        &json!({ "sessionId": "0be9b59c-af70-47b0-9169-d9de92330600" }),
        "click",
        &idx,
    );
    assert_eq!(out["sessionId"], "uuid");
}

#[test]
fn nested_objects_and_arrays_recurse() {
    let idx = EnumIndex::new();
    let out = shape::of(
        &json!({ "target": { "role": "button", "ordinal": 1 }, "paths": ["/a/b.pdf", "/c.pdf"] }),
        "click",
        &idx,
    );
    assert_eq!(out["target"]["role"], "str<8");
    assert_eq!(out["target"]["ordinal"], "num:1");
    assert_eq!(out["paths"]["array"], 2);
    assert_eq!(out["paths"]["items"], "str<8");
}

#[test]
fn long_strings_bucket_upward() {
    let idx = EnumIndex::new();
    let long = "x".repeat(200);
    let out = shape::of(&json!({ "essay": long }), "t", &idx);
    assert_eq!(out["essay"], "str<512");
}
