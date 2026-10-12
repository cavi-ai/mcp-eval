//! Shared schema contract. Validation never retrieves external resources.
use serde_json::Value;

struct LocalOnly;
impl jsonschema::Retrieve for LocalOnly {
    fn retrieve(
        &self,
        _: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema references are disabled".into())
    }
}

pub(crate) fn compile(schema: &Value) -> Result<jsonschema::Validator, ()> {
    jsonschema::options()
        .with_retriever(LocalOnly)
        .should_validate_formats(true)
        .build(schema)
        .map_err(|_| ())
}

pub(crate) fn conforms(schema: &Value, value: &Value) -> bool {
    compile(schema).is_ok_and(|validator| validator.is_valid(value))
}

/// First schema violation as `invalid tool arguments[ at <pointer>]: <reason>`,
/// built from the error's instance path and kind only, never the instance value.
/// The path is safe to return only while validated tool schemas declare no
/// schema-valued `additionalProperties`, `patternProperties`, or `propertyNames`;
/// revisit this if one is added.
pub(crate) fn violation(schema: &Value, value: &Value) -> Option<String> {
    use jsonschema::error::{TypeKind, ValidationErrorKind};
    let Ok(validator) = compile(schema) else {
        return Some("invalid tool arguments: input schema unavailable".to_owned());
    };
    let error = validator.validate(value).err()?;
    let reason = match error.kind() {
        ValidationErrorKind::Required { property } => match property.as_str() {
            Some(name) => format!("missing required property {name}"),
            None => "missing required property".to_owned(),
        },
        ValidationErrorKind::Type {
            kind: TypeKind::Single(expected),
        } => format!("expected type {expected}"),
        ValidationErrorKind::Type {
            kind: TypeKind::Multiple(expected),
        } => format!(
            "expected one of types {}",
            expected
                .iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ValidationErrorKind::Enum { .. } => "not one of the allowed values".to_owned(),
        other => format!("violates {}", other.keyword()),
    };
    let path = error.instance_path().as_str();
    let message = if path.is_empty() {
        format!("invalid tool arguments: {reason}")
    } else {
        format!("invalid tool arguments at {path}: {reason}")
    };
    Some(message.chars().take(240).collect())
}

#[cfg(test)]
mod tests {
    use super::violation;
    use serde_json::json;

    fn state_schema() -> serde_json::Value {
        json!({"type":"object","properties":{"state":{"type":"string"}}})
    }

    #[test]
    fn valid_value_has_no_violation() {
        assert_eq!(
            violation(&state_schema(), &json!({"state":"CANARY_Q9"})),
            None
        );
    }

    #[test]
    fn wrong_type_reports_path_and_type() {
        let message = violation(&state_schema(), &json!({"state":5})).unwrap();
        assert!(message.contains("/state"), "{message}");
        assert!(message.contains("string"), "{message}");
    }

    #[test]
    fn missing_required_names_property() {
        let schema = json!({"type":"object","required":["finding_id"]});
        let message = violation(&schema, &json!({})).unwrap();
        assert!(message.contains("finding_id"), "{message}");
    }

    #[test]
    fn wrong_typed_value_is_not_echoed() {
        let message = violation(&state_schema(), &json!({"state":{"x":"CANARY_Q9"}})).unwrap();
        assert!(!message.contains("CANARY_Q9"), "{message}");
    }

    #[test]
    fn enum_violation_hides_value_and_allowed_set() {
        let schema = json!({"type":"object","properties":{"state":{"enum":["open","closed"]}}});
        let message = violation(&schema, &json!({"state":"CANARY_Q9"})).unwrap();
        assert!(message.contains("/state"), "{message}");
        assert!(!message.contains("CANARY_Q9"), "{message}");
        assert!(!message.contains("open"), "{message}");
    }

    #[test]
    fn message_is_length_capped() {
        let message = violation(&state_schema(), &json!({"state":{"x":"CANARY_Q9"}})).unwrap();
        assert!(message.chars().count() <= 240, "{message}");
    }
}
