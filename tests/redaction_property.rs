use mcpeval::shape::{self, EnumIndex};
use serde_json::{json, Value};

/// Deterministic pseudo-random generator: tests must reproduce on failure.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn canary(index: usize) -> String {
    format!("CANARY-{index:04x}-8f3ad91b2c7e")
}

fn build_tree(rng: &mut Rng, depth: usize, next_canary: &mut usize) -> Value {
    if depth == 0 || rng.next().is_multiple_of(4) {
        let index = *next_canary;
        *next_canary += 1;
        return match rng.next() % 3 {
            0 => json!(canary(index)),
            1 => json!(format!("/Users/someone/{}.pdf", canary(index))),
            _ => json!(format!("https://example.com/a?token={}", canary(index))),
        };
    }
    if rng.next().is_multiple_of(2) {
        let len = (rng.next() % 4) as usize + 1;
        Value::Array(
            (0..len)
                .map(|_| build_tree(rng, depth - 1, next_canary))
                .collect(),
        )
    } else {
        let len = (rng.next() % 4) as usize + 1;
        let mut map = serde_json::Map::new();
        for field in 0..len {
            map.insert(
                format!("field{field}"),
                build_tree(rng, depth - 1, next_canary),
            );
        }
        Value::Object(map)
    }
}

#[test]
fn no_generated_canary_survives_shaping() {
    let enums = EnumIndex::new();
    for seed in 0..200u64 {
        let mut rng = Rng(seed);
        let mut next_canary = 0;
        let tree = build_tree(&mut rng, 4, &mut next_canary);
        let shaped = shape::of(&tree, "tool", &enums).to_string();
        assert!(
            !shaped.contains("CANARY"),
            "seed {seed} leaked a canary: {shaped}"
        );
        assert!(
            !shaped.contains("/Users/"),
            "seed {seed} leaked a path: {shaped}"
        );
        assert!(
            !shaped.contains("token="),
            "seed {seed} leaked a query: {shaped}"
        );
    }
}

/// The property test above only ever proves the negative (canary *values*
/// never survive). Object keys and numeric leaves are retained verbatim by
/// public privacy contract in README.md, which makes them
/// the highest-value things to pin down with a positive assertion — a
/// regression here silently over-redacts, not under-redacts, so the
/// negative-only property test above would never catch it.
#[test]
fn object_keys_are_retained_verbatim_at_any_nesting_depth() {
    let enums = EnumIndex::new();
    let tree = json!({
        "CANARY-KEY-top-8f3ad91b": {
            "CANARY-KEY-nested-2c7e": "irrelevant value"
        }
    });
    let shaped = shape::of(&tree, "tool", &enums);
    assert!(
        shaped.get("CANARY-KEY-top-8f3ad91b").is_some(),
        "a top-level object key must be retained verbatim: {shaped}"
    );
    assert!(
        shaped["CANARY-KEY-top-8f3ad91b"]
            .get("CANARY-KEY-nested-2c7e")
            .is_some(),
        "a nested object key must be retained verbatim: {shaped}"
    );
}

#[test]
fn numeric_leaves_are_retained_verbatim() {
    let enums = EnumIndex::new();
    let tree = json!({ "n": 84317, "f": -12.5, "items": [1, 2, 3] });
    let shaped = shape::of(&tree, "tool", &enums);
    assert_eq!(shaped["n"], "num:84317");
    assert_eq!(shaped["f"], "num:-12.5");
    assert_eq!(shaped["items"]["array"], 3);
    assert_eq!(shaped["items"]["items"], "num:1");
}
