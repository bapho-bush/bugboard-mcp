use rmcp::model::CallToolResult;
use serde_json::{Value, json};
#[derive(Clone, Debug)]
pub(crate) struct ToolFailure {
    code: &'static str,
    message: String,
    details: Value,
}

impl ToolFailure {
    pub(crate) fn new(code: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self {
            code,
            message: message.into(),
            details,
        }
    }

    pub(crate) fn transport(message: impl Into<String>) -> Self {
        Self::new(
            "transport_error",
            "Could not reach bugboard.",
            json!({"source": message.into()}),
        )
    }

    pub(crate) fn invalid_reference(message: impl Into<String>) -> Self {
        Self::new("invalid_reference", message, json!({}))
    }

    pub(crate) fn invalid_arguments(message: impl Into<String>) -> Self {
        Self::new("invalid_arguments", message, json!({}))
    }

    pub(crate) fn empty_result(message: impl Into<String>) -> Self {
        Self::new("empty_result", message, json!({}))
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new("internal_error", message, json!({}))
    }

    pub(crate) fn bugboard_changed(operation: &'static str, error: impl std::fmt::Display) -> Self {
        Self::new(
            "bugboard_changed",
            "Bugboard returned an unexpected response shape.",
            json!({"operation": operation, "source": error.to_string()}),
        )
    }

    pub(crate) fn bugboard_updated(operation: &'static str) -> Self {
        Self::new(
            "bugboard_updated",
            "Bugboard was updated. Retry the operation.",
            json!({"operation": operation, "retryable": true}),
        )
    }

    pub(crate) fn mutation_delivery_is_uncertain(&self) -> bool {
        matches!(self.code, "transport_error" | "bugboard_changed")
    }

    pub(crate) fn into_result(self) -> CallToolResult {
        CallToolResult::structured_error(json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "details": self.details,
            }
        }))
    }
}

impl From<e1c_element_rpc::Error> for ToolFailure {
    fn from(error: e1c_element_rpc::Error) -> Self {
        Self::internal(error.to_string())
    }
}

pub(crate) fn http_failure(
    operation: &'static str,
    status: u16,
    content_type: String,
    body_bytes: usize,
    parsed: Option<&Value>,
) -> ToolFailure {
    if matches!(status, 401 | 403) {
        return ToolFailure::new(
            "not_authenticated",
            "Bugboard rejected the configured session.",
            json!({"operation": operation, "status": status}),
        );
    }

    ToolFailure::new(
        "unknown_bugboard_error",
        "Bugboard returned an unexpected error.",
        json!({
            "operation": operation,
            "status": status,
            "content_type": content_type,
            "body_bytes": body_bytes,
            "json_keys": parsed
                .and_then(Value::as_object)
                .map(|object| object.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default(),
        }),
    )
}
