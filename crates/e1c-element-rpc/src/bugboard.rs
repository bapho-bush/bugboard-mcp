use serde_json::{Value, json};
use std::collections::HashSet;

use crate::{
    DynamicListInfoRequest, DynamicListRequest, DynamicListResponse, DynamicListRow,
    EntityReadRequest, Error, HttpRequest, ModuleCall, Parameter, Reference, RemoteCallerInfo,
    encode_component,
};

pub const BUGBOARD_BASE_URL: &str = "https://bugboard.1c.ru";
pub const BUGBOARD_LOCALE: &str = "ru-RU";
pub const BUGBOARD_PUB_LOCALE: &str = "ru_RU";
pub const BUG_REFERENCE_TYPE: &str = "e1c::bugboard::Багборд::Ошибки.Reference";
pub const PROJECT_REFERENCE_TYPE: &str = "e1c::bugboard::Багборд::Проекты.Reference";
pub const VERSION_REFERENCE_TYPE: &str = "e1c::bugboard::Багборд::Версии.Reference";
pub const SUBSCRIPTION_MODULE: &str =
    "e1c::bugboard::ПодпискиИГолосования::ПодпискиИГолосованияКлиентСервер";
pub const BUG_FORM_MODULE: &str = "e1c::bugboard::Основное::ФормаОшибки";
pub const FULL_TEXT_SEARCH_MODULE: &str = "e1c::bugboard::Основное::ОкноПолнотекстовогоПоиска";
pub const VERSION_MODULE: &str = "e1c::bugboard::Багборд::Версии";
pub const REMOTE_CALLER_ID: &str = "00000000-0000-0000-0000-000000000000";
const BUG_VOTE_KIND_TYPE: &str = "e1c::bugboard::ПодпискиИГолосования::ВидГолосаПоОшибке";
const VERSION_BUG_LIST_TYPE: &str = "e1c::bugboard::Багборд::МассивыОшибокДляСписков";
const BUG_REFERENCE_ARRAY_TYPE: &str =
    "Std::Collections::Array<e1c::bugboard::Багборд::Ошибки.Reference>";
const BUG_HISTORY_ARRAY_TYPE: &str =
    "Std::Collections::Array<e1c::bugboard::Багборд::Ошибки.ИсторияЖизни>";
const BUG_HISTORY_ENTRY_TYPE: &str = "e1c::bugboard::Багборд::Ошибки.ИсторияЖизни";
const APP_HASH_VARIABLE: &str = "__gSrv_APP_HASH";
const SERVER_VERSION_VARIABLE: &str = "__gSrv_SRV_VERSION";

pub fn decode_g5_version_from_shell(shell: &str) -> Result<String, Error> {
    let app_hash = shell_string_assignment(shell, APP_HASH_VARIABLE).ok_or(
        Error::UnexpectedResponse("bugboard shell did not contain the application hash"),
    )?;
    let server_version = shell_string_assignment(shell, SERVER_VERSION_VARIABLE).ok_or(
        Error::UnexpectedResponse("bugboard shell did not contain the server version"),
    )?;

    Ok(encode_component(&format!("{app_hash},{server_version}")))
}

fn shell_string_assignment<'a>(shell: &'a str, variable: &str) -> Option<&'a str> {
    let (_, assignment) = shell.split_once(variable)?;
    let value = assignment.trim_start().strip_prefix('=')?.trim_start();
    let quote = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))?;
    let value = value.strip_prefix(quote)?;
    let (value, _) = value.split_once(quote)?;
    (!value.is_empty()).then_some(value)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRow {
    pub reference: String,
    pub abbreviation: Option<String>,
    pub title: Option<String>,
    pub deleted: Option<bool>,
    pub updated_at: Option<String>,
    pub group_order: Option<i64>,
    pub order: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionRow {
    pub reference: String,
    pub title: Option<String>,
    pub project_reference: String,
    pub source_order: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BugRow {
    pub reference: String,
    pub number: Option<String>,
    pub title: Option<String>,
    pub updated_at: Option<String>,
    pub published_at: Option<String>,
    pub status: Option<String>,
    pub status_code: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BugDetails {
    pub number: Option<String>,
    pub title: Option<String>,
    pub status: Option<String>,
    pub status_code: Option<u64>,
    pub registered_at: Option<String>,
    pub published_at: Option<String>,
    pub updated_at: Option<String>,
    pub description: Option<String>,
    pub support_case: Option<String>,
    pub url: Option<String>,
    pub history: Vec<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BugVoteKind {
    ManifestsForMe,
    FixImportant,
}

impl BugVoteKind {
    fn code(self) -> u8 {
        match self {
            Self::ManifestsForMe => 0,
            Self::FixImportant => 1,
        }
    }

    fn voted_bugs_method(self) -> &'static str {
        match self {
            Self::ManifestsForMe => "ОшибкиПоКоторымЕстьГолосУМеняОшибкаПроявляется",
            Self::FixImportant => "ОшибкиПоКоторымЕстьГолосДляМеняИсправлениеВажно",
        }
    }
}

pub fn auth_status_request() -> Result<HttpRequest, Error> {
    bugboard_get("/sys/auth/status")
}

pub fn ecs_access_request() -> Result<HttpRequest, Error> {
    bugboard_get(&format!(
        "/ui/ecs-application/getEcsAccess?locale={BUGBOARD_LOCALE}&pubLocale={BUGBOARD_PUB_LOCALE}"
    ))?
    .with_header("Content-Type", "application/json;charset=UTF-8")
}

pub fn project_list_request(limit: u32) -> Result<HttpRequest, Error> {
    require_limit(limit)?;
    dynamic_list_request(DynamicListRequest::new(project_list_dynamic_list(), limit))
}

pub fn project_list_info_request() -> Result<HttpRequest, Error> {
    dynamic_list_info_request(DynamicListInfoRequest::new(project_list_dynamic_list()))
}

pub fn project_versions_request(project_ref: &str, limit: u32) -> Result<HttpRequest, Error> {
    require_reference(project_ref)?;
    require_limit(limit)?;
    dynamic_list_request(DynamicListRequest::new(
        project_versions_dynamic_list(project_ref, None),
        limit,
    ))
}

pub fn version_lookup_request(
    project_ref: &str,
    version_title: &str,
) -> Result<HttpRequest, Error> {
    require_reference(project_ref)?;
    require_argument("version_title", version_title)?;
    dynamic_list_request(DynamicListRequest::new(
        project_versions_dynamic_list(project_ref, Some(version_title)),
        2,
    ))
}

pub fn version_bugs_request(project_ref: &str, version_ref: &str) -> Result<HttpRequest, Error> {
    require_reference(project_ref)?;
    require_reference(version_ref)?;
    module_call_request(
        VERSION_MODULE,
        "ПолучитьМассивОшибокВВерсии",
        vec![
            Parameter::typed(PROJECT_REFERENCE_TYPE, project_ref)?,
            Parameter::typed(VERSION_REFERENCE_TYPE, version_ref)?,
        ],
    )
}

pub fn project_subscriptions_request() -> Result<HttpRequest, Error> {
    module_call_request(SUBSCRIPTION_MODULE, "ПолучитьПодпискиНаПроекты", Vec::new())
}

pub fn project_subscribe_request(project_ref: &str) -> Result<HttpRequest, Error> {
    project_subscription_request("ПодписатьсяНаПроект", project_ref, false)
}

pub fn project_unsubscribe_request(project_ref: &str) -> Result<HttpRequest, Error> {
    project_subscription_request("ОтменитьПодпискуНаПроект", project_ref, true)
}

pub fn bug_list_request(limit: u32) -> Result<HttpRequest, Error> {
    require_limit(limit)?;
    dynamic_list_request(DynamicListRequest::new(bug_list_dynamic_list(), limit))
}

pub fn bug_list_info_request() -> Result<HttpRequest, Error> {
    dynamic_list_info_request(DynamicListInfoRequest::new(bug_list_dynamic_list()))
}

pub fn bug_lookup_request(number: &str) -> Result<HttpRequest, Error> {
    require_argument("number", number)?;
    dynamic_list_request(DynamicListRequest::new(bug_lookup_dynamic_list(number), 1))
}

pub fn subscribed_bug_list_request() -> Result<HttpRequest, Error> {
    module_call_request(SUBSCRIPTION_MODULE, "ПолучитьПодпискиНаОшибки", Vec::new())
}

pub fn voted_bug_list_request(vote_kind: BugVoteKind) -> Result<HttpRequest, Error> {
    module_call_request(
        SUBSCRIPTION_MODULE,
        vote_kind.voted_bugs_method(),
        Vec::new(),
    )
}

pub fn bug_get_request(bug_ref: &str) -> Result<HttpRequest, Error> {
    require_reference(bug_ref)?;
    EntityReadRequest::new(Reference::new(BUG_REFERENCE_TYPE, bug_ref)?)
        .into_http_request_without_session(BUGBOARD_BASE_URL, BUGBOARD_LOCALE, BUGBOARD_PUB_LOCALE)
        .and_then(with_bugboard_headers)
}

pub fn bug_subscription_vote_state_request(bug_ref: &str) -> Result<HttpRequest, Error> {
    require_reference(bug_ref)?;
    module_call_request(
        BUG_FORM_MODULE,
        "ПолучитьПодпискиИГолосованияПоОшибке",
        vec![Parameter::typed(BUG_REFERENCE_TYPE, bug_ref)?],
    )
}

pub fn bug_subscribe_request(bug_ref: &str) -> Result<HttpRequest, Error> {
    bug_subscription_request("ПодписатьсяНаОшибку", bug_ref, false)
}

pub fn bug_unsubscribe_request(bug_ref: &str) -> Result<HttpRequest, Error> {
    bug_subscription_request("ОтменитьПодпискуНаОшибку", bug_ref, true)
}

pub fn bug_vote_request(bug_ref: &str, vote_kind: BugVoteKind) -> Result<HttpRequest, Error> {
    bug_vote_change_request("Проголосовать", bug_ref, vote_kind, false)
}

pub fn bug_unvote_request(bug_ref: &str, vote_kind: BugVoteKind) -> Result<HttpRequest, Error> {
    bug_vote_change_request("ОтменитьГолос", bug_ref, vote_kind, true)
}

pub fn bug_full_text_search_request(query: &str) -> Result<HttpRequest, Error> {
    require_argument("query", query)?;
    module_call_request(
        FULL_TEXT_SEARCH_MODULE,
        "ВыполнитьПоискПоОшибкам",
        vec![Parameter::typed("Std::String", query)?],
    )
}

pub fn decode_project_rows(response: &DynamicListResponse) -> Result<Vec<ProjectRow>, Error> {
    response
        .rows()
        .iter()
        .map(|row| {
            let fields = required_fields(row, 7)?;
            Ok(ProjectRow {
                reference: required_reference(fields, 0, PROJECT_REFERENCE_TYPE)?,
                abbreviation: string_value_at(fields, 1),
                title: string_value_at(fields, 2),
                deleted: bool_value_at(fields, 3),
                updated_at: date_value_at(fields, 4),
                group_order: integer_value_at(fields, 5),
                order: integer_value_at(fields, 6),
            })
        })
        .collect()
}

pub fn decode_version_rows(response: &DynamicListResponse) -> Result<Vec<VersionRow>, Error> {
    response
        .rows()
        .iter()
        .map(|row| {
            let fields = required_fields(row, 4)?;
            Ok(VersionRow {
                reference: required_reference(fields, 0, VERSION_REFERENCE_TYPE)?,
                title: string_value_at(fields, 1),
                project_reference: required_reference(fields, 2, PROJECT_REFERENCE_TYPE)?,
                source_order: integer_value_at(fields, 3),
            })
        })
        .collect()
}

pub fn decode_bug_rows(response: &DynamicListResponse) -> Result<Vec<BugRow>, Error> {
    response
        .rows()
        .iter()
        .map(|row| {
            let fields = required_fields(row, 6)?;
            let status_value = &fields[5];
            let status_code = bug_status_code(status_value);
            Ok(BugRow {
                reference: required_reference(fields, 0, BUG_REFERENCE_TYPE)?,
                number: string_value_at(fields, 1),
                title: string_value_at(fields, 2),
                updated_at: date_value_at(fields, 3),
                published_at: date_value_at(fields, 4),
                status: string_value(status_value).or_else(|| match status_code {
                    Some(8) => Some("Отклонена".to_owned()),
                    _ => None,
                }),
                status_code,
            })
        })
        .collect()
}

pub fn decode_project_references(value: &Value) -> Result<Vec<String>, Error> {
    decode_reference_list(value, PROJECT_REFERENCE_TYPE)
}

pub fn decode_bug_references(value: &Value) -> Result<Vec<String>, Error> {
    decode_reference_list(value, BUG_REFERENCE_TYPE)
}

pub fn decode_version_bug_references(value: &Value) -> Result<Vec<String>, Error> {
    if value.get("type").and_then(Value::as_str) != Some(VERSION_BUG_LIST_TYPE) {
        return Err(Error::UnexpectedResponse(
            "version bug result contains an unexpected type",
        ));
    }
    let all_bugs = value
        .pointer("/value/ВсеОшибки")
        .ok_or(Error::UnexpectedResponse(
            "version bug result must contain ВсеОшибки",
        ))?;
    if all_bugs.get("type").and_then(Value::as_str) != Some(BUG_REFERENCE_ARRAY_TYPE) {
        return Err(Error::UnexpectedResponse(
            "ВсеОшибки contains an unexpected type",
        ));
    }
    decode_bug_references(all_bugs)
}

pub fn decode_full_text_bug_references(value: &Value) -> Result<Vec<String>, Error> {
    let (mut references, recognized_shape) = match decode_bug_references(value) {
        Ok(references) => (references, true),
        Err(_) => {
            let mut references = Vec::new();
            collect_typed_references(value, BUG_REFERENCE_TYPE, &mut references);
            (references, is_empty_full_text_result(value))
        }
    };
    if references.is_empty() && !recognized_shape {
        return Err(Error::UnexpectedResponse(
            "full-text result contains no bug references",
        ));
    }

    let mut seen = HashSet::new();
    references.retain(|reference| seen.insert(reference.clone()));
    Ok(references)
}

fn is_empty_full_text_result(value: &Value) -> bool {
    value
        .get("items")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
        || value
            .pointer("/result/items")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
}

fn decode_reference_list(value: &Value, expected_type: &str) -> Result<Vec<String>, Error> {
    let items = value
        .as_array()
        .or_else(|| value.pointer("/value/items").and_then(Value::as_array))
        .ok_or(Error::UnexpectedResponse(
            "reference list must be an array or contain value.items",
        ))?;

    items
        .iter()
        .map(|item| {
            if item.get("type").and_then(Value::as_str) != Some(expected_type) {
                return Err(Error::UnexpectedResponse(
                    "reference list contains an unexpected type",
                ));
            }
            let reference = item
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default();
            require_reference(reference)?;
            Ok(reference.to_owned())
        })
        .collect()
}

fn collect_typed_references(value: &Value, expected_type: &str, references: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_typed_references(item, expected_type, references);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some(expected_type)
                && let Some(reference) = object.get("value").and_then(Value::as_str)
                && !reference.trim().is_empty()
            {
                references.push(reference.to_owned());
            }
            for item in object.values() {
                collect_typed_references(item, expected_type, references);
            }
        }
        _ => {}
    }
}

pub fn decode_bug_details(response: &Value) -> Result<BugDetails, Error> {
    let object = bug_object(response);
    let number = string_field(object, "Наименование")
        .or_else(|| string_field(object, "Name"))
        .or_else(|| string_field(object, "Code"));
    let title = string_field(object, "Заголовок");
    if number.is_none() && title.is_none() {
        return Err(Error::UnexpectedResponse(
            "bug details contain neither number nor title",
        ));
    }

    let status_value = object.get("РассчитанныйСтатусБагборда");
    let status_code = status_value.and_then(bug_status_code);
    let fragment = string_field(object, "ФрагментСсылки");

    Ok(BugDetails {
        number,
        title,
        status: status_value.and_then(|value| {
            string_value(value).or_else(|| match status_code {
                Some(8) => Some("Отклонена".to_owned()),
                _ => None,
            })
        }),
        status_code,
        registered_at: date_field(object, "ДатаРегистрации"),
        published_at: date_field(object, "ДатаПубликации"),
        updated_at: date_field(object, "ДатаПоследнегоОбновления"),
        description: string_field(object, "Описание"),
        support_case: string_field(object, "КодОбращения"),
        url: fragment.map(|fragment| {
            if fragment.starts_with("http://") || fragment.starts_with("https://") {
                fragment
            } else {
                format!("{BUGBOARD_BASE_URL}/#{fragment}")
            }
        }),
        history: decode_bug_history_entries(object.get("ИсторияЖизни").ok_or(
            Error::UnexpectedResponse("bug details must contain ИсторияЖизни"),
        )?)?,
    })
}

pub fn decode_bug_history_entries(value: &Value) -> Result<Vec<Value>, Error> {
    if value.get("type").and_then(Value::as_str) != Some(BUG_HISTORY_ARRAY_TYPE) {
        return Err(Error::UnexpectedResponse(
            "bug history contains an unexpected type",
        ));
    }

    let items = value
        .pointer("/value/items")
        .and_then(Value::as_array)
        .ok_or(Error::UnexpectedResponse(
            "bug history must contain value.items",
        ))?;

    items
        .iter()
        .map(|item| {
            if item.get("type").and_then(Value::as_str) != Some(BUG_HISTORY_ENTRY_TYPE) {
                return Err(Error::UnexpectedResponse(
                    "bug history contains an unexpected entry type",
                ));
            }

            item.get("value")
                .filter(|value| value.is_object())
                .cloned()
                .ok_or(Error::UnexpectedResponse(
                    "bug history entries must contain an object value",
                ))
        })
        .collect()
}

fn project_subscription_request(
    method: &'static str,
    project_ref: &str,
    with_undefined: bool,
) -> Result<HttpRequest, Error> {
    require_reference(project_ref)?;
    let mut params = vec![Parameter::typed(PROJECT_REFERENCE_TYPE, project_ref)?];
    if with_undefined {
        params.push(Parameter::undefined());
    }
    module_call_request(SUBSCRIPTION_MODULE, method, params)
}

fn bug_subscription_request(
    method: &'static str,
    bug_ref: &str,
    with_undefined: bool,
) -> Result<HttpRequest, Error> {
    require_reference(bug_ref)?;
    let mut params = vec![Parameter::typed(BUG_REFERENCE_TYPE, bug_ref)?];
    if with_undefined {
        params.push(Parameter::undefined());
    }
    module_call_request(SUBSCRIPTION_MODULE, method, params)
}

fn bug_vote_change_request(
    method: &'static str,
    bug_ref: &str,
    vote_kind: BugVoteKind,
    with_undefined: bool,
) -> Result<HttpRequest, Error> {
    require_reference(bug_ref)?;
    let mut params = vec![
        Parameter::typed(BUG_REFERENCE_TYPE, bug_ref)?,
        Parameter::typed(BUG_VOTE_KIND_TYPE, vote_kind.code())?,
    ];
    if with_undefined {
        params.push(Parameter::undefined());
    }
    module_call_request(SUBSCRIPTION_MODULE, method, params)
}

fn require_reference(value: &str) -> Result<(), Error> {
    if value.trim().is_empty() {
        Err(Error::EmptyReferenceValue)
    } else {
        Ok(())
    }
}

fn require_argument(name: &'static str, value: &str) -> Result<(), Error> {
    if value.trim().is_empty() {
        Err(Error::EmptyArgument(name))
    } else {
        Ok(())
    }
}

fn require_limit(limit: u32) -> Result<(), Error> {
    if limit == 0 {
        Err(Error::ZeroLimit)
    } else {
        Ok(())
    }
}

fn required_fields(row: &DynamicListRow, count: usize) -> Result<&[Value], Error> {
    let fields = row.field_values();
    if fields.len() != count {
        Err(Error::UnexpectedResponse(
            "dynamic-list row has an unexpected field count",
        ))
    } else {
        Ok(fields)
    }
}

fn required_reference(
    fields: &[Value],
    index: usize,
    expected_type: &str,
) -> Result<String, Error> {
    reference_value_at(fields, index, expected_type).ok_or(Error::UnexpectedResponse(
        "dynamic-list row has an invalid reference field",
    ))
}

fn reference_value_at(fields: &[Value], index: usize, expected_type: &str) -> Option<String> {
    let field = fields.get(index)?;
    if field.get("type")?.as_str()? != expected_type {
        return None;
    }
    field
        .get("value")?
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn string_value_at(fields: &[Value], index: usize) -> Option<String> {
    string_value(fields.get(index)?)
}

fn string_field(fields: &Value, name: &str) -> Option<String> {
    string_value(fields.get(name)?)
}

fn string_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .or_else(|| value.get("value").and_then(Value::as_str))
        .or_else(|| value.get("presentation").and_then(Value::as_str))
        .or_else(|| value.get("Presentation").and_then(Value::as_str))
        .or_else(|| {
            value
                .get("value")
                .and_then(|value| value.get("Presentation"))
                .and_then(Value::as_str)
        })
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn date_value_at(fields: &[Value], index: usize) -> Option<String> {
    string_value_at(fields, index).filter(|value| !value.starts_with("0001-01-01"))
}

fn date_field(fields: &Value, name: &str) -> Option<String> {
    string_field(fields, name).filter(|value| !value.starts_with("0001-01-01"))
}

fn bug_status_code(value: &Value) -> Option<u64> {
    if value.get("type")?.as_str()? != "e1c::bugboard::Багборд::СтатусыОшибок" {
        return None;
    }
    value.get("value")?.as_u64()
}

fn bug_object(value: &Value) -> &Value {
    let object = value.get("object").unwrap_or(value);
    object
        .get("value")
        .and_then(Value::as_object)
        .map(|_| &object["value"])
        .unwrap_or(object)
}

fn bool_value_at(fields: &[Value], index: usize) -> Option<bool> {
    let value = fields.get(index)?;
    value
        .as_bool()
        .or_else(|| value.get("value").and_then(Value::as_bool))
}

fn integer_value_at(fields: &[Value], index: usize) -> Option<i64> {
    let value = fields.get(index)?;
    value
        .as_i64()
        .or_else(|| value.get("value").and_then(Value::as_i64))
}

fn project_list_dynamic_list() -> Value {
    json!({
        "MainTable": table("e1c::bugboard::Багборд::Проекты"),
        "Fields": fields(&[
            ("Ссылка", "Ссылка"),
            ("Аббревиатура", "Аббревиатура"),
            ("Наименование", "Наименование"),
            ("ПометкаУдаления", "ПометкаУдаления"),
            ("ДатаПоследнегоОбновления", "ДатаПоследнегоОбновления"),
            ("ГруппаПроектов.Порядок", "ПорядокГруппы"),
            ("Порядок", "Порядок"),
        ]),
        "Filter": filter_group(vec![filter_item(
            "ПометкаУдаления",
            json!({"type": "Std::Boolean", "value": false}),
        )]),
        "Sorting": sorting(&["ПорядокГруппы", "Порядок"], 0),
    })
}

fn project_versions_dynamic_list(project_ref: &str, version_title: Option<&str>) -> Value {
    let mut filters = vec![filter_item(
        "Проект",
        json!({"type": PROJECT_REFERENCE_TYPE, "value": project_ref}),
    )];
    if let Some(version_title) = version_title {
        filters.push(filter_item(
            "Наименование",
            json!({"type": "Std::String", "value": version_title}),
        ));
    }

    json!({
        "MainTable": table("e1c::bugboard::Багборд::Версии"),
        "Fields": fields(&[
            ("Ссылка", "Ссылка"),
            ("Наименование", "Наименование"),
            ("Проект", "Проект"),
            ("ПорядокВИсточнике", "ПорядокВИсточнике"),
        ]),
        "Filter": filter_group(filters),
        "Sorting": sorting(&["ПорядокВИсточнике"], 1),
    })
}

fn bug_list_dynamic_list() -> Value {
    bug_dynamic_list(vec![filter_item(
        "КУдалению",
        json!({"type": "Std::Boolean", "value": false}),
    )])
}

fn bug_lookup_dynamic_list(number: &str) -> Value {
    bug_dynamic_list(vec![
        filter_item("КУдалению", json!({"type": "Std::Boolean", "value": false})),
        filter_item(
            "Наименование",
            json!({"type": "Std::String", "value": number}),
        ),
    ])
}

fn bug_dynamic_list(filters: Vec<Value>) -> Value {
    json!({
        "MainTable": table("e1c::bugboard::Багборд::Ошибки"),
        "Fields": fields(&[
            ("Ссылка", "Ссылка"),
            ("Наименование", "Наименование"),
            ("Заголовок", "Заголовок"),
            ("ДатаПоследнегоОбновления", "ДатаПоследнегоОбновления"),
            ("ДатаПубликации", "ДатаПубликации"),
            ("РассчитанныйСтатусБагборда", "РассчитанныйСтатусБагборда"),
        ]),
        "Filter": filter_group(filters),
        "Sorting": sorting(&["ДатаПоследнегоОбновления"], 0),
    })
}

fn module_call_request(
    module: &str,
    method: &str,
    params: Vec<Parameter>,
) -> Result<HttpRequest, Error> {
    let mut call = ModuleCall::new(module, method)?.with_remote_caller_info(remote_caller_info()?);
    for param in params {
        call = call.with_parameter(param);
    }
    call.into_http_request_without_session(BUGBOARD_BASE_URL, BUGBOARD_LOCALE, BUGBOARD_PUB_LOCALE)
        .and_then(with_bugboard_headers)
}

fn remote_caller_info() -> Result<RemoteCallerInfo, Error> {
    RemoteCallerInfo::disabled(REMOTE_CALLER_ID)
}

fn dynamic_list_request(body: DynamicListRequest) -> Result<HttpRequest, Error> {
    body.into_http_request_without_session(BUGBOARD_BASE_URL, BUGBOARD_LOCALE, BUGBOARD_PUB_LOCALE)
        .and_then(with_bugboard_headers)
}

fn dynamic_list_info_request(body: DynamicListInfoRequest) -> Result<HttpRequest, Error> {
    body.into_http_request_without_session(BUGBOARD_BASE_URL, BUGBOARD_LOCALE, BUGBOARD_PUB_LOCALE)
        .and_then(with_bugboard_headers)
}

fn bugboard_get(path_and_query: &str) -> Result<HttpRequest, Error> {
    with_bugboard_headers(HttpRequest::get(format!(
        "{BUGBOARD_BASE_URL}{path_and_query}"
    ))?)
}

fn with_bugboard_headers(request: HttpRequest) -> Result<HttpRequest, Error> {
    request
        .with_header("Origin", BUGBOARD_BASE_URL)?
        .with_header("Referer", format!("{BUGBOARD_BASE_URL}/"))
}

fn table(name: &str) -> Value {
    json!({
        "type": "Std::Interface::DataSources::DynamicList::DynamicListTable",
        "value": {
            "Table": name,
            "Alias": "",
            "Arguments": {
                "type": "Std::Collections::Array<Std::Interface::DataSources::TableArgument|Std::Interface::DataSources::TableArgumentExpression>",
                "value": {"items": []},
            },
        },
    })
}

fn fields(items: &[(&str, &str)]) -> Value {
    json!({
        "type": "Std::Collections::Array<Std::Interface::DataSources::DynamicList::DynamicListField>",
        "value": {
            "items": items
                .iter()
                .map(|(expression, alias)| {
                    json!({
                        "type": "Std::Interface::DataSources::DynamicList::DynamicListField",
                        "value": {
                            "Expression": expression,
                            "Alias": alias,
                            "Presentation": "",
                            "DisplayInFiltersSettings": {"type": "Std::Auto"},
                            "DisplayInSortingSettings": true,
                            "IncludeInRowData": true,
                            "DisplayInSimpleFilters": {"type": "Std::Auto"},
                            "AllowedFilterValuesLimitation": {
                                "LimitationFilterFieldAttribute": "",
                                "LimitationFilterValueExpression": "",
                            },
                            "InSearchUsage": {"type": "Std::Auto"},
                        },
                    })
                })
                .collect::<Vec<_>>(),
        },
    })
}

fn filter_group(items: Vec<Value>) -> Value {
    json!({
        "type": "Std::Interface::Filters::FilterItemGroup",
        "value": {
            "GroupKind": 0,
            "Items": {
                "type": "Std::Collections::Array<Std::Interface::Filters::FilterItem|Std::Interface::Filters::FilterItemExpression|Std::Interface::Filters::FilterItemFieldCollection|Std::Interface::Filters::FilterItemGroup>",
                "value": {
                    "items": [{
                        "type": "Std::Interface::Filters::FilterItemGroup",
                        "value": {
                            "GroupKind": 0,
                            "Items": {
                                "type": "Std::Collections::Array<Std::Interface::Filters::FilterItem|Std::Interface::Filters::FilterItemExpression|Std::Interface::Filters::FilterItemFieldCollection|Std::Interface::Filters::FilterItemGroup>",
                                "value": {"items": items},
                            },
                            "Use": true,
                        },
                    }],
                },
            },
            "Use": true,
        },
    })
}

fn filter_item(field: &str, value: Value) -> Value {
    json!({
        "type": "Std::Interface::Filters::FilterItem",
        "value": {
            "Field": field,
            "ComparisonKind": 1,
            "Value": value,
            "Use": true,
            "Hierarchy": "",
        },
    })
}

fn sorting(fields: &[&str], direction: u8) -> Value {
    json!({
        "type": "Std::Collections::Array<Std::Interface::Filters::SortingItem>",
        "value": {
            "items": fields
                .iter()
                .map(|field| {
                    json!({
                        "type": "Std::Interface::Filters::SortingItem",
                        "value": {"Field": field, "SortingDirection": direction},
                    })
                })
                .collect::<Vec<_>>(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_body(request: HttpRequest) -> Value {
        serde_json::from_str(request.body().expect("JSON request body"))
            .expect("valid JSON request body")
    }

    #[test]
    fn g5_version_is_decoded_from_bugboard_shell() {
        let shell = r#"
            <script>
                var __gSrv_APP_HASH = 'app_hash';
                var __gSrv_SRV_VERSION = "9.2.9-12";
            </script>
        "#;

        assert_eq!(
            decode_g5_version_from_shell(shell).unwrap(),
            "app_hash%2C9.2.9-12"
        );
    }

    #[test]
    fn g5_version_decoder_fails_closed_on_missing_or_empty_values() {
        let missing = "var __gSrv_APP_HASH = 'app_hash';";
        let empty = "var __gSrv_APP_HASH = ''; var __gSrv_SRV_VERSION = '1';";

        assert!(decode_g5_version_from_shell(missing).is_err());
        assert!(decode_g5_version_from_shell(empty).is_err());
    }

    #[test]
    fn named_mutations_own_the_captured_wire_signatures() {
        let subscribe = request_body(bug_subscribe_request("bug-ref").unwrap());
        let unsubscribe = request_body(bug_unsubscribe_request("bug-ref").unwrap());
        let vote = request_body(bug_vote_request("bug-ref", BugVoteKind::FixImportant).unwrap());
        let unvote =
            request_body(bug_unvote_request("bug-ref", BugVoteKind::FixImportant).unwrap());

        assert_eq!(subscribe["methodName"], "ПодписатьсяНаОшибку");
        assert_eq!(subscribe["parameters"].as_array().unwrap().len(), 1);
        assert_eq!(unsubscribe["methodName"], "ОтменитьПодпискуНаОшибку");
        assert_eq!(unsubscribe["parameters"][1]["type"], "Std::Undefined");
        assert_eq!(vote["methodName"], "Проголосовать");
        assert_eq!(vote["parameters"][1]["value"], 1);
        assert_eq!(unvote["methodName"], "ОтменитьГолос");
        assert_eq!(unvote["parameters"][2]["type"], "Std::Undefined");
    }

    #[test]
    fn version_bugs_owns_the_captured_wire_signature() {
        let request = request_body(version_bugs_request("project-ref", "version-ref").unwrap());

        assert_eq!(request["moduleName"], VERSION_MODULE);
        assert_eq!(request["methodName"], "ПолучитьМассивОшибокВВерсии");
        assert_eq!(request["parameters"][0]["type"], PROJECT_REFERENCE_TYPE);
        assert_eq!(request["parameters"][0]["value"], "project-ref");
        assert_eq!(request["parameters"][1]["type"], VERSION_REFERENCE_TYPE);
        assert_eq!(request["parameters"][1]["value"], "version-ref");
    }

    #[test]
    fn info_and_row_requests_share_the_same_dynamic_list_specs() {
        let project_rows = request_body(project_list_request(1).unwrap());
        let project_info = request_body(project_list_info_request().unwrap());
        let bug_rows = request_body(bug_list_request(1).unwrap());
        let bug_info = request_body(bug_list_info_request().unwrap());

        assert_eq!(project_rows["dynamicList"], project_info["dynamicList"]);
        assert_eq!(bug_rows["dynamicList"], bug_info["dynamicList"]);
    }

    #[test]
    fn project_versions_are_requested_newest_first() {
        let request = request_body(project_versions_request("project-ref", 50).unwrap());

        assert_eq!(
            request["dynamicList"]["Sorting"]["value"]["items"][0]["value"]["SortingDirection"],
            1
        );
    }

    #[test]
    fn version_lookup_filters_by_project_and_exact_title() {
        let request = request_body(version_lookup_request("project-ref", "release-title").unwrap());
        let filters = request["dynamicList"]["Filter"]
            .pointer("/value/Items/value/items/0/value/Items/value/items")
            .and_then(Value::as_array)
            .unwrap();

        assert_eq!(request["limit"], 2);
        assert_eq!(filters[0]["value"]["Field"], "Проект");
        assert_eq!(filters[0]["value"]["Value"]["type"], PROJECT_REFERENCE_TYPE);
        assert_eq!(filters[1]["value"]["Field"], "Наименование");
        assert_eq!(filters[1]["value"]["Value"]["type"], "Std::String");
        assert_eq!(filters[1]["value"]["Value"]["value"], "release-title");
    }

    #[test]
    fn decodes_the_same_bug_fields_the_request_selects() {
        let response = DynamicListResponse::from_slice(
            serde_json::to_vec(&json!({
                "rows": [{
                    "fieldValues": [
                        {"type": BUG_REFERENCE_TYPE, "value": "bug-ref"},
                        "123456",
                        "Crash on start",
                        "2026-07-01",
                        "0001-01-01T00:00:00",
                        {"type": "e1c::bugboard::Багборд::СтатусыОшибок", "value": 8}
                    ]
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(
            decode_bug_rows(&response).unwrap(),
            vec![BugRow {
                reference: "bug-ref".to_owned(),
                number: Some("123456".to_owned()),
                title: Some("Crash on start".to_owned()),
                updated_at: Some("2026-07-01".to_owned()),
                published_at: None,
                status: Some("Отклонена".to_owned()),
                status_code: Some(8),
            }]
        );
    }

    #[test]
    fn decodes_project_and_version_fields_with_raw_references() {
        let projects = DynamicListResponse::from_slice(
            serde_json::to_vec(&json!({
                "rows": [{"fieldValues": [
                    {"type": PROJECT_REFERENCE_TYPE, "value": "project-ref"},
                    "ERP", "1C:ERP", false, "2026-07-01", 2, 3
                ]}]
            }))
            .unwrap(),
        )
        .unwrap();
        let versions = DynamicListResponse::from_slice(
            serde_json::to_vec(&json!({
                "rows": [{"fieldValues": [
                    {"type": VERSION_REFERENCE_TYPE, "value": "version-ref"},
                    "8.3.27",
                    {"type": PROJECT_REFERENCE_TYPE, "value": "project-ref"},
                    27
                ]}]
            }))
            .unwrap(),
        )
        .unwrap();

        let project = decode_project_rows(&projects).unwrap().remove(0);
        let version = decode_version_rows(&versions).unwrap().remove(0);

        assert_eq!(project.reference, "project-ref");
        assert_eq!(project.title.as_deref(), Some("1C:ERP"));
        assert_eq!(version.reference, "version-ref");
        assert_eq!(version.project_reference, "project-ref");
        assert_eq!(version.source_order, Some(27));
    }

    #[test]
    fn decodes_typed_bug_details_and_keeps_nested_values_raw() {
        let response = json!({
            "object": {
                "type": "e1c::bugboard::Багборд::Ошибки",
                "value": {
                    "Наименование": "60024806",
                    "Заголовок": "Crash",
                    "РассчитанныйСтатусБагборда": {
                        "type": "e1c::bugboard::Багборд::СтатусыОшибок",
                        "value": 8
                    },
                    "ДатаРегистрации": "2026-01-01",
                    "ДатаПубликации": "0001-01-01T00:00:00",
                    "ДатаПоследнегоОбновления": "2026-01-02",
                    "Описание": "Details",
                    "КодОбращения": "CASE-1",
                    "ФрагментСсылки": "bug/60024806",
                    "ИсторияЖизни": {
                        "type": BUG_HISTORY_ARRAY_TYPE,
                        "value": {
                            "items": [{
                                "type": BUG_HISTORY_ENTRY_TYPE,
                                "value": {
                                    "author": {"type": "User.Reference", "value": "raw-user-ref"},
                                    "event": "created"
                                }
                            }]
                        }
                    }
                }
            }
        });

        let details = decode_bug_details(&response).unwrap();

        assert_eq!(details.number.as_deref(), Some("60024806"));
        assert_eq!(details.status.as_deref(), Some("Отклонена"));
        assert_eq!(details.status_code, Some(8));
        assert_eq!(details.registered_at.as_deref(), Some("2026-01-01"));
        assert_eq!(details.published_at, None);
        assert_eq!(
            details.url.as_deref(),
            Some("https://bugboard.1c.ru/#bug/60024806")
        );
        assert_eq!(details.history[0]["author"]["value"], "raw-user-ref");
        assert_eq!(details.history[0]["event"], "created");
    }

    #[test]
    fn bug_history_decoder_is_strict_and_allows_empty_results() {
        assert_eq!(
            decode_bug_history_entries(&json!({
                "type": BUG_HISTORY_ARRAY_TYPE,
                "value": {"items": []}
            }))
            .unwrap(),
            Vec::<Value>::new()
        );
        assert_eq!(
            decode_bug_history_entries(&json!({
                "type": BUG_HISTORY_ARRAY_TYPE,
                "value": {"items": [{
                    "type": BUG_HISTORY_ENTRY_TYPE,
                    "value": {"event": "created"}
                }]}
            }))
            .unwrap()[0]["event"],
            "created"
        );
    }

    #[test]
    fn bug_history_decoder_fails_closed_on_missing_or_malformed_shape() {
        for value in [
            json!({}),
            json!({"type": BUG_HISTORY_ARRAY_TYPE}),
            json!({"type": BUG_HISTORY_ARRAY_TYPE, "value": {}}),
            json!({"type": "wrong", "value": {"items": []}}),
            json!({"type": BUG_HISTORY_ARRAY_TYPE, "value": {"items": [{
                "type": "wrong",
                "value": {}
            }]}}),
            json!({"type": BUG_HISTORY_ARRAY_TYPE, "value": {"items": [{
                "type": BUG_HISTORY_ENTRY_TYPE,
                "value": []
            }]}}),
        ] {
            assert!(decode_bug_history_entries(&value).is_err());
        }
    }

    #[test]
    fn bug_details_require_a_number_or_title() {
        assert!(matches!(
            decode_bug_details(&json!({"object": {"value": {"Описание": "only details"}}})),
            Err(Error::UnexpectedResponse(_))
        ));
    }

    #[test]
    fn row_decoder_fails_closed_on_wire_shape_changes() {
        let response = DynamicListResponse::from_slice(
            br#"{"rows":[{"fieldValues":[{"type":"wrong.Reference","value":"ref"},null,null,null,null,null]}]}"#,
        )
        .unwrap();

        assert!(matches!(
            decode_bug_rows(&response),
            Err(Error::UnexpectedResponse(_))
        ));
    }

    #[test]
    fn reference_lists_are_strict_and_allow_empty_results() {
        assert!(decode_bug_references(&json!({"value": {"items": []}})).is_ok());
        assert_eq!(
            decode_bug_references(&json!({
                "value": {"items": [{"type": BUG_REFERENCE_TYPE, "value": "bug-ref"}]}
            }))
            .unwrap(),
            ["bug-ref"]
        );
        assert!(
            decode_bug_references(&json!({
                "value": {"items": [{"type": PROJECT_REFERENCE_TYPE, "value": "project-ref"}]}
            }))
            .is_err()
        );
    }

    #[test]
    fn version_bug_references_own_the_captured_structured_result() {
        let result = json!({
            "type": VERSION_BUG_LIST_TYPE,
            "value": {
                "ВсеОшибки": {
                    "type": BUG_REFERENCE_ARRAY_TYPE,
                    "value": {"items": [
                        {"type": BUG_REFERENCE_TYPE, "value": "bug-a"},
                        {"type": BUG_REFERENCE_TYPE, "value": "bug-b"}
                    ]}
                }
            }
        });

        assert_eq!(
            decode_version_bug_references(&result).unwrap(),
            ["bug-a", "bug-b"]
        );
        assert_eq!(
            decode_version_bug_references(&json!({
                "type": VERSION_BUG_LIST_TYPE,
                "value": {"ВсеОшибки": {
                    "type": BUG_REFERENCE_ARRAY_TYPE,
                    "value": {"items": []}
                }}
            }))
            .unwrap(),
            Vec::<String>::new()
        );
        assert!(decode_version_bug_references(&json!({"type": "wrong"})).is_err());
    }

    #[test]
    fn full_text_reference_decoder_owns_nested_result_shape() {
        assert_eq!(
            decode_full_text_bug_references(&json!({
                "items": [{"bug": {"type": BUG_REFERENCE_TYPE, "value": "bug-ref"}}]
            }))
            .unwrap(),
            ["bug-ref"]
        );
        assert!(decode_full_text_bug_references(&json!({})).is_err());
    }

    #[test]
    fn full_text_reference_decoder_allows_an_empty_nested_result() {
        assert_eq!(
            decode_full_text_bug_references(&json!({"result": {"items": []}})).unwrap(),
            Vec::<String>::new()
        );
        assert!(
            decode_full_text_bug_references(&json!({"result": {"items": "not-an-array"}})).is_err()
        );
    }

    #[test]
    fn full_text_reference_decoder_deduplicates_in_response_order() {
        assert_eq!(
            decode_full_text_bug_references(&json!({
                "items": [
                    {"bug": {"type": BUG_REFERENCE_TYPE, "value": "bug-b"}},
                    {"bug": {"type": BUG_REFERENCE_TYPE, "value": "bug-a"}},
                    {"bug": {"type": BUG_REFERENCE_TYPE, "value": "bug-b"}},
                    {"bug": {"type": BUG_REFERENCE_TYPE, "value": "bug-c"}},
                    {"bug": {"type": BUG_REFERENCE_TYPE, "value": "bug-a"}}
                ]
            }))
            .unwrap(),
            ["bug-b", "bug-a", "bug-c"]
        );
    }

    #[test]
    fn rejects_empty_inputs_and_zero_limits() {
        assert!(matches!(project_list_request(0), Err(Error::ZeroLimit)));
        assert!(matches!(bug_list_request(0), Err(Error::ZeroLimit)));
        assert!(matches!(
            project_versions_request(" ", 1),
            Err(Error::EmptyReferenceValue)
        ));
        assert!(matches!(
            bug_lookup_request(" "),
            Err(Error::EmptyArgument("number"))
        ));
        assert!(matches!(
            bug_full_text_search_request(" "),
            Err(Error::EmptyArgument("query"))
        ));
    }
}
