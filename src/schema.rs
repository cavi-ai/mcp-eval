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
