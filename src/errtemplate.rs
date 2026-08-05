const TEMPLATE: &str = "{message}";

/// Returns a stable, content-free template for any human error message.
pub fn normalize(_message: &str) -> String {
    TEMPLATE.to_owned()
}
