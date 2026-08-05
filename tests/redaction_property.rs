use mcpeval::shape::{self, EnumIndex};
use serde_json::{json, Value};

/// Deterministic pseudo-random generator: tests must reproduce on failure.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn canary(index: usize) -> String {
    format!("CANARY-{index:04x}-8f3ad91b2c7e")
}

fn build_tree(rng: &mut Rng, depth: usize, next_canary: &mut usize) -> Value {
    if depth == 0 || rng.next() % 4 == 0 {
        let index = *next_canary;
        *next_canary += 1;
        return match rng.next() % 3 {
            0 => json!(canary(index)),
            1 => json!(format!("/Users/someone/{}.pdf", canary(index))),
            _ => json!(format!("https://example.com/a?token={}", canary(index))),
        };
    }
    if rng.next() % 2 == 0 {
        let len = (rng.next() % 4) as usize + 1;
        Value::Array((0..len).map(|_| build_tree(rng, depth - 1, next_canary)).collect())
    } else {
        let len = (rng.next() % 4) as usize + 1;
        let mut map = serde_json::Map::new();
        for field in 0..len {
            map.insert(format!("field{field}"), build_tree(rng, depth - 1, next_canary));
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
        assert!(!shaped.contains("/Users/"), "seed {seed} leaked a path: {shaped}");
        assert!(!shaped.contains("token="), "seed {seed} leaked a query: {shaped}");
    }
}
