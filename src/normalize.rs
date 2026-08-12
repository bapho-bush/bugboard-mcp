use e1c_element_rpc::bugboard::{self, BugRow, ProjectRow, VersionRow};
use serde_json::{Value, json};

use crate::{errors::ToolFailure, handles::HandleKind, server::BugboardServer};

pub(crate) fn normalize_project_row(
    server: &BugboardServer,
    row: &ProjectRow,
) -> Result<Value, ToolFailure> {
    let handle = server.remember_ref(HandleKind::Project, &row.reference)?;
    let title = row
        .title
        .clone()
        .or_else(|| row.abbreviation.clone())
        .ok_or_else(|| ToolFailure::bugboard_changed("project_list", "missing project title"))?;

    Ok(json!({
        "project_handle": handle,
        "title": title,
        "abbreviation": row.abbreviation,
        "updated_at": row.updated_at,
    }))
}

pub(crate) fn normalize_version_row(row: &VersionRow) -> Result<Value, ToolFailure> {
    let title = row.title.clone().ok_or_else(|| {
        ToolFailure::bugboard_changed("project_get_versions", "missing version title")
    })?;

    Ok(json!({
        "title": title,
        "source_order": row.source_order,
    }))
}

pub(crate) fn normalize_bug_row(
    server: &BugboardServer,
    row: &BugRow,
) -> Result<Value, ToolFailure> {
    let handle = server.remember_ref(HandleKind::Bug, &row.reference)?;

    Ok(json!({
        "bug_handle": handle,
        "number": row.number,
        "title": row.title,
        "status": row.status,
        "status_code": row.status_code,
        "published_at": row.published_at,
        "updated_at": row.updated_at,
    }))
}

pub(crate) fn is_bug_number(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit())
}

pub(crate) fn normalize_bug_details(value: Value) -> Result<Value, ToolFailure> {
    let details = bugboard::decode_bug_details(&value)
        .map_err(|error| ToolFailure::bugboard_changed("bug_get", error))?;

    Ok(json!({
        "number": details.number,
        "title": details.title,
        "status": details.status,
        "status_code": details.status_code,
        "registered_at": details.registered_at,
        "published_at": details.published_at,
        "updated_at": details.updated_at,
        "description": details.description,
        "support_case": details.support_case,
        "url": details.url,
        "history_count": details.history.len(),
    }))
}

pub(crate) fn normalize_bug_history(history: Vec<Value>) -> Value {
    let history = history
        .into_iter()
        .map(|item| scrub_references(&item))
        .collect::<Vec<_>>();

    json!({
        "count": history.len(),
        "history": history,
    })
}

pub(crate) fn scrub_references(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(scrub_references).collect()),
        Value::Object(object) => {
            let mut sanitized = serde_json::Map::new();
            let is_reference = object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|type_name| type_name.ends_with(".Reference"));
            for (key, item) in object {
                if is_reference && key == "value" && item.is_string() {
                    sanitized.insert(
                        key.clone(),
                        Value::String("<redacted-reference>".to_owned()),
                    );
                } else {
                    sanitized.insert(key.clone(), scrub_references(item));
                }
            }
            Value::Object(sanitized)
        }
        Value::String(text) => Value::String(text.chars().take(1000).collect()),
        _ => value.clone(),
    }
}
