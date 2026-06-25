//! Rendering helpers shared by `qn sql query` and `qn sql schema`.

use serde_json::Value;

/// Stringifies a JSON value for a table cell.
///
/// Query rows are arbitrary JSON objects, so cell values can be any JSON type.
/// Scalars render bare (strings without quotes); `null` renders as `—` to match
/// the [`opt_cell`](crate::output::opt_cell) convention; arrays and objects fall
/// back to compact JSON.
pub(crate) fn json_cell(v: &Value) -> String {
    match v {
        Value::Null => "—".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => v.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scalars_render_bare() {
        assert_eq!(
            json_cell(&json!("SystemSpotSendAction")),
            "SystemSpotSendAction"
        );
        assert_eq!(json_cell(&json!(42)), "42");
        assert_eq!(json_cell(&json!(true)), "true");
    }

    #[test]
    fn null_renders_as_dash() {
        assert_eq!(json_cell(&Value::Null), "—");
    }

    #[test]
    fn nested_renders_as_compact_json() {
        assert_eq!(json_cell(&json!(["a", "b"])), r#"["a","b"]"#);
        assert_eq!(json_cell(&json!({"k": 1})), r#"{"k":1}"#);
    }
}
