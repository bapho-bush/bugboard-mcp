use e1c_element_rpc::{
    DynamicListResponse, HttpMethod, HttpRequest, ModuleCallResponse, Session,
    bugboard::{
        self, BUG_REFERENCE_TYPE, BUGBOARD_BASE_URL, BUGBOARD_LOCALE, BUGBOARD_PUB_LOCALE,
        PROJECT_REFERENCE_TYPE, VERSION_REFERENCE_TYPE,
    },
};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Read;

const PROJECT_SUBSCRIPTION_ROUNDTRIP: &str = "project_subscription_roundtrip";

#[derive(Serialize)]
struct ProbeResult {
    name: &'static str,
    url: String,
    request: Value,
    status: u16,
    ok: bool,
    content_type: String,
    body_bytes: usize,
    json_keys: Vec<String>,
    safe: Value,
}

#[derive(Clone, Copy)]
enum ResponseKind {
    Auth,
    EcsAccess,
    DynamicListInfo,
    ProjectList,
    VersionList,
    BugList,
    ModuleCall,
    ProjectReferenceList,
    BugReferenceList,
    VersionBugList,
    Mutation,
    Entity,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mutation = std::env::var("BUGBOARD_MUTATION").ok();
    if std::env::var_os("BUGBOARD_MUTATION_DRY_RUN").is_some() {
        let mutation = mutation
            .as_deref()
            .ok_or("set BUGBOARD_MUTATION=project_subscription_roundtrip for dry-run")?;
        println!(
            "{}",
            serde_json::to_string(&mutation_dry_run_result(mutation)?)?
        );
        return Ok(());
    }

    let cookie = read_cookie_header()?;
    let bug_ref = std::env::var("BUGBOARD_BUG_REF").ok();
    let client = Client::new();
    let session = Session::from_cookie_header(cookie)?;
    let g5_version = fetch_g5_version(&client, &session)?;

    let add_g5 = |request: HttpRequest| -> Result<HttpRequest, e1c_element_rpc::Error> {
        request.with_header("X-G5-Version", &g5_version)
    };

    let probes = vec![
        (
            "auth_status",
            bugboard::auth_status_request()?,
            ResponseKind::Auth,
        ),
        (
            "ecs_access",
            add_g5(bugboard::ecs_access_request()?)?,
            ResponseKind::EcsAccess,
        ),
        (
            "project_filter_info",
            add_g5(bugboard::project_list_info_request()?)?,
            ResponseKind::DynamicListInfo,
        ),
        (
            "project_list",
            add_g5(bugboard::project_list_request(11)?)?,
            ResponseKind::ProjectList,
        ),
        (
            "bug_filter_info",
            add_g5(bugboard::bug_list_info_request()?)?,
            ResponseKind::DynamicListInfo,
        ),
    ];

    let mut failed = false;
    let mut project_list_rows = None;

    for (name, request, kind) in probes {
        let (result, parsed) = execute(&client, &session, name, request, kind)?;
        failed |= !result.ok;
        println!("{}", serde_json::to_string(&result)?);
        if name == "project_list" {
            project_list_rows = parsed;
        }
    }

    let project_ref = first_project_ref(project_list_rows.as_ref())
        .ok_or("project_list returned no project reference")?;
    let version_title = std::env::var("BUGBOARD_VERSION_TITLE").ok();
    let version_request = match version_title.as_deref() {
        Some(version_title) => bugboard::version_lookup_request(&project_ref, version_title)?,
        None => bugboard::project_versions_request(&project_ref, 50)?,
    };
    let (result, project_versions) = execute(
        &client,
        &session,
        "project_versions",
        add_g5(version_request)?,
        ResponseKind::VersionList,
    )?;
    failed |= !result.ok;
    println!("{}", serde_json::to_string(&result)?);

    let version_ref = first_version_ref(project_versions.as_ref())
        .ok_or("project_versions returned no version reference")?;
    let (result, _) = execute(
        &client,
        &session,
        "version_bugs",
        add_g5(bugboard::version_bugs_request(&project_ref, &version_ref)?)?,
        ResponseKind::VersionBugList,
    )?;
    failed |= !result.ok;
    println!("{}", serde_json::to_string(&result)?);

    let (result, bug_list) = execute(
        &client,
        &session,
        "bug_list",
        add_g5(bugboard::bug_list_request(4)?)?,
        ResponseKind::BugList,
    )?;
    failed |= !result.ok;
    println!("{}", serde_json::to_string(&result)?);

    if std::env::var_os("BUGBOARD_EXPORT_REFS").is_some() {
        println!(
            "{}",
            serde_json::to_string(&reference_export_result(
                project_list_rows.as_ref(),
                bug_list.as_ref(),
            ))?
        );
    }

    let (result, subscribed_bug_list) = execute(
        &client,
        &session,
        "subscribed_bug_list",
        add_g5(bugboard::subscribed_bug_list_request()?)?,
        ResponseKind::BugReferenceList,
    )?;
    failed |= !result.ok;
    println!("{}", serde_json::to_string(&result)?);

    let (result, voted_bug_list) = execute(
        &client,
        &session,
        "voted_bug_list",
        add_g5(bugboard::voted_bug_list_request(
            bugboard::BugVoteKind::FixImportant,
        )?)?,
        ResponseKind::BugReferenceList,
    )?;
    failed |= !result.ok;
    println!("{}", serde_json::to_string(&result)?);

    let auto_bug_ref = bug_ref
        .or_else(|| first_bug_ref(subscribed_bug_list.as_ref()))
        .or_else(|| first_bug_ref(voted_bug_list.as_ref()))
        .or_else(|| first_bug_ref(bug_list.as_ref()));
    if let Some(bug_ref) = auto_bug_ref.as_deref() {
        let (result, _) = execute(
            &client,
            &session,
            "bug_get",
            add_g5(bugboard::bug_get_request(bug_ref)?)?,
            ResponseKind::Entity,
        )?;
        failed |= !result.ok;
        println!("{}", serde_json::to_string(&result)?);
        let (result, _) = execute(
            &client,
            &session,
            "bug_subscription_vote_state",
            add_g5(bugboard::bug_subscription_vote_state_request(bug_ref)?)?,
            ResponseKind::ModuleCall,
        )?;
        failed |= !result.ok;
        println!("{}", serde_json::to_string(&result)?);
    } else {
        eprintln!("skip bug_get: set BUGBOARD_BUG_REF or use a session with bug list rows");
    }

    let probes = vec![(
        "project_subscriptions",
        add_g5(bugboard::project_subscriptions_request()?)?,
    )];

    for (name, request) in probes {
        let (result, _) = execute(
            &client,
            &session,
            name,
            request,
            ResponseKind::ProjectReferenceList,
        )?;
        failed |= !result.ok;
        println!("{}", serde_json::to_string(&result)?);
    }

    if let Some(mutation) = mutation.as_deref() {
        if mutation != PROJECT_SUBSCRIPTION_ROUNDTRIP {
            return Err("BUGBOARD_MUTATION must be project_subscription_roundtrip".into());
        }
        let result = project_subscription_roundtrip(&client, &session, &add_g5)?;
        failed |= !result.ok;
        println!("{}", serde_json::to_string(&result)?);
    }

    if failed {
        std::process::exit(1);
    }

    Ok(())
}

fn fetch_g5_version(
    client: &Client,
    session: &Session,
) -> Result<String, Box<dyn std::error::Error>> {
    let response = client
        .get(bugboard::BUGBOARD_BASE_URL)
        .header(reqwest::header::COOKIE, session.cookie_header())
        .send()?;
    let expected_origin = reqwest::Url::parse(bugboard::BUGBOARD_BASE_URL)?.origin();
    if response.url().origin() != expected_origin {
        return Err("bugboard redirected the configured session to authentication".into());
    }
    if !response.status().is_success() {
        return Err(format!("bugboard shell returned HTTP {}", response.status()).into());
    }

    Ok(bugboard::decode_g5_version_from_shell(&response.text()?)?)
}

fn read_cookie_header() -> Result<String, Box<dyn std::error::Error>> {
    let value = std::env::var("BUGBOARD_COOKIE")
        .map_err(|_| "set BUGBOARD_COOKIE to a ready Cookie header, or set BUGBOARD_COOKIE=- and pipe it on stdin")?;
    cookie_header_from_input(value, || {
        let mut cookie = String::new();
        std::io::stdin().read_to_string(&mut cookie)?;
        Ok(cookie)
    })
}

fn cookie_header_from_input(
    value: String,
    read_stdin: impl FnOnce() -> Result<String, std::io::Error>,
) -> Result<String, Box<dyn std::error::Error>> {
    if value == "-" {
        let cookie = read_stdin()?.trim().to_owned();
        if cookie.is_empty() {
            return Err("stdin cookie header must not be empty".into());
        }
        return Ok(cookie);
    }

    Ok(value)
}

fn first_bug_ref(value: Option<&Value>) -> Option<String> {
    first_reference(value, BUG_REFERENCE_TYPE)
}

fn first_project_ref(value: Option<&Value>) -> Option<String> {
    first_reference(value, "e1c::bugboard::Багборд::Проекты.Reference")
}

fn first_version_ref(value: Option<&Value>) -> Option<String> {
    first_reference(value, VERSION_REFERENCE_TYPE)
}

fn first_reference(value: Option<&Value>, expected_type: &str) -> Option<String> {
    let value = value?;
    match value {
        Value::Array(items) => items
            .iter()
            .find_map(|item| first_reference(Some(item), expected_type)),
        Value::Object(object) => {
            if object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|actual| actual == expected_type)
            {
                return object.get("value")?.as_str().map(ToOwned::to_owned);
            }
            object
                .values()
                .find_map(|item| first_reference(Some(item), expected_type))
        }
        _ => None,
    }
}

fn reference_export_result(project_list: Option<&Value>, bug_list: Option<&Value>) -> ProbeResult {
    ProbeResult {
        name: "reference_export",
        url: String::new(),
        request: json!({"method": null, "headerNames": [], "bodyBytes": 0, "body": null}),
        status: 0,
        ok: true,
        content_type: String::new(),
        body_bytes: 0,
        json_keys: Vec::new(),
        safe: json!({
            "projectRefEnv": "BUGBOARD_PROJECT_REF",
            "projectRef": first_project_ref(project_list),
            "bugRefEnv": "BUGBOARD_BUG_REF",
            "bugRef": first_bug_ref(bug_list),
        }),
    }
}

fn project_subscription_roundtrip<F>(
    client: &Client,
    session: &Session,
    add_g5: &F,
) -> Result<ProbeResult, Box<dyn std::error::Error>>
where
    F: Fn(HttpRequest) -> Result<HttpRequest, e1c_element_rpc::Error>,
{
    let project_ref = std::env::var("BUGBOARD_PROJECT_REF")
        .map_err(|_| "set BUGBOARD_PROJECT_REF for project_subscription_roundtrip")?;
    let initial = project_subscription_state(client, session, add_g5, &project_ref)?;
    let (steps, first_request, second_request) = if initial {
        (
            ["project_unsubscribe", "project_subscribe"],
            bugboard::project_unsubscribe_request(&project_ref)?,
            bugboard::project_subscribe_request(&project_ref)?,
        )
    } else {
        (
            ["project_subscribe", "project_unsubscribe"],
            bugboard::project_subscribe_request(&project_ref)?,
            bugboard::project_unsubscribe_request(&project_ref)?,
        )
    };
    let first_request = add_g5(first_request)?;
    let restore_request = add_g5(second_request)?;

    let first = mutation_attempt(execute(
        client,
        session,
        "mutation",
        first_request,
        ResponseKind::Mutation,
    ));
    let after_first = project_subscription_state(client, session, add_g5, &project_ref)
        .map_err(|error| error.to_string());
    // The first request may have reached the server even when its response was invalid.
    let restore = mutation_attempt(execute(
        client,
        session,
        "mutation",
        restore_request,
        ResponseKind::Mutation,
    ));
    let final_state = project_subscription_state(client, session, add_g5, &project_ref)
        .map_err(|error| error.to_string());
    let (ok, safe) = roundtrip_verdict(initial, &first, &after_first, &restore, &final_state);

    Ok(ProbeResult {
        name: "project_subscription_roundtrip",
        url: format!(
            "{BUGBOARD_BASE_URL}/ui/module/call?locale={BUGBOARD_LOCALE}&pubLocale={BUGBOARD_PUB_LOCALE}"
        ),
        request: json!({
            "method": "POST",
            "headerNames": ["Accept", "Content-Type", "Origin", "Referer", "X-G5-Version"],
            "bodyBytes": 0,
            "body": {
                "kind": "projectSubscriptionRoundtrip",
                "moduleName": "e1c::bugboard::ПодпискиИГолосования::ПодпискиИГолосованияКлиентСервер",
                "methodNames": steps,
                "parameterTypes": [PROJECT_REFERENCE_TYPE],
            },
        }),
        status: if ok { 200 } else { 500 },
        ok,
        content_type: String::new(),
        body_bytes: 0,
        json_keys: Vec::new(),
        safe: json!({
            "requested": PROJECT_SUBSCRIPTION_ROUNDTRIP,
            "requiredReferenceEnv": "BUGBOARD_PROJECT_REF",
            "steps": steps,
            "outcome": safe,
        }),
    })
}

#[derive(Debug)]
struct MutationAttempt {
    status: u16,
    ok: bool,
    error: Option<String>,
}

fn mutation_attempt(
    result: Result<(ProbeResult, Option<Value>), Box<dyn std::error::Error>>,
) -> MutationAttempt {
    match result {
        Ok((result, _)) => MutationAttempt {
            status: result.status,
            ok: result.ok,
            error: result.safe["validationError"]
                .as_str()
                .map(ToOwned::to_owned),
        },
        Err(error) => MutationAttempt {
            status: 0,
            ok: false,
            error: Some(error.to_string()),
        },
    }
}

fn roundtrip_verdict(
    initial: bool,
    first: &MutationAttempt,
    after_first: &Result<bool, String>,
    restore: &MutationAttempt,
    final_state: &Result<bool, String>,
) -> (bool, Value) {
    let expected_after_first = !initial;
    let changed = after_first
        .as_ref()
        .is_ok_and(|state| *state == expected_after_first);
    let restored = final_state.as_ref().is_ok_and(|state| *state == initial);
    let ok = first.ok && changed && restore.ok && restored;

    (
        ok,
        json!({
            "initialSubscribed": initial,
            "afterFirstSubscribed": after_first.as_ref().ok(),
            "expectedAfterFirstSubscribed": expected_after_first,
            "finalSubscribed": final_state.as_ref().ok(),
            "restored": restored,
            "steps": [
                {"kind": "mutation", "ok": first.ok, "status": first.status, "error": first.error},
                {"kind": "verify_changed", "ok": changed, "error": after_first.as_ref().err()},
                {"kind": "restore", "ok": restore.ok, "status": restore.status, "error": restore.error},
                {"kind": "verify_restored", "ok": restored, "error": final_state.as_ref().err()},
            ],
        }),
    )
}

fn project_subscription_state<F>(
    client: &Client,
    session: &Session,
    add_g5: &F,
    project_ref: &str,
) -> Result<bool, Box<dyn std::error::Error>>
where
    F: Fn(HttpRequest) -> Result<HttpRequest, e1c_element_rpc::Error>,
{
    let (result, parsed) = execute(
        client,
        session,
        "project_subscriptions",
        add_g5(bugboard::project_subscriptions_request()?)?,
        ResponseKind::ProjectReferenceList,
    )?;
    if !result.ok {
        return Err(format!(
            "project_subscriptions failed validation: {}",
            result.safe["validationError"]
                .as_str()
                .unwrap_or("unknown response error")
        )
        .into());
    }

    project_subscription_state_from_value(
        parsed
            .as_ref()
            .ok_or("project_subscriptions returned no JSON")?,
        project_ref,
    )
    .map_err(Into::into)
}

fn project_subscription_state_from_value(
    value: &Value,
    project_ref: &str,
) -> Result<bool, &'static str> {
    let items = value
        .get("result")
        .and_then(|result| result.get("value"))
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array)
        .ok_or("project subscription result must contain value.items array")?;
    if items.iter().any(|item| {
        item.get("type").and_then(Value::as_str).is_none()
            || item.get("value").and_then(Value::as_str).is_none()
    }) {
        return Err("project subscription result contains an invalid reference");
    }

    Ok(items.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some(PROJECT_REFERENCE_TYPE)
            && item.get("value").and_then(Value::as_str) == Some(project_ref)
    }))
}

fn mutation_dry_run_result(mutation: &str) -> Result<ProbeResult, Box<dyn std::error::Error>> {
    if mutation != PROJECT_SUBSCRIPTION_ROUNDTRIP {
        return Err("BUGBOARD_MUTATION must be project_subscription_roundtrip".into());
    }

    project_subscription_roundtrip_dry_run_result(
        std::env::var_os("BUGBOARD_PROJECT_REF").is_some(),
    )
}

fn project_subscription_roundtrip_dry_run_result(
    has_reference_env: bool,
) -> Result<ProbeResult, Box<dyn std::error::Error>> {
    Ok(ProbeResult {
        name: "mutation_dry_run",
        url: "https://bugboard.1c.ru/ui/module/call?locale=ru-RU&pubLocale=ru_RU".to_owned(),
        request: json!({
            "method": "POST",
            "headerNames": ["Accept", "Content-Type", "Origin", "Referer", "X-G5-Version"],
            "bodyBytes": 0,
            "body": {
                "kind": "projectSubscriptionRoundtrip",
                "moduleName": "e1c::bugboard::ПодпискиИГолосования::ПодпискиИГолосованияКлиентСервер",
                "methodNames": ["ПодписатьсяНаПроект", "ОтменитьПодпискуНаПроект"],
                "parameterTypes": [PROJECT_REFERENCE_TYPE],
            },
        }),
        status: 0,
        ok: true,
        content_type: String::new(),
        body_bytes: 0,
        json_keys: Vec::new(),
        safe: json!({
            "requested": PROJECT_SUBSCRIPTION_ROUNDTRIP,
            "requiredReferenceEnv": "BUGBOARD_PROJECT_REF",
            "hasReferenceEnv": has_reference_env,
            "referenceType": PROJECT_REFERENCE_TYPE,
            "willExecute": false,
            "willRestoreInitialState": true,
        }),
    })
}

fn execute(
    client: &Client,
    session: &Session,
    name: &'static str,
    request: HttpRequest,
    kind: ResponseKind,
) -> Result<(ProbeResult, Option<Value>), Box<dyn std::error::Error>> {
    let request = request.with_session(session)?;
    let url = request.url().to_owned();
    let request_safe = summarize_request(&request);
    let (method, _, headers, body) = request.into_parts();
    let method = match method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
    };
    let mut request = client.request(method, &url);
    for header in headers {
        let (name, value) = header.into_parts();
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.body(body);
    }
    let response = request.send()?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .unwrap_or_default();
    let body = response.bytes()?.to_vec();
    let (parsed, validation_error) = validate_response(status, kind, &body);
    let json_keys = parsed
        .as_ref()
        .and_then(Value::as_object)
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    let mut safe = match name {
        "auth_status" => parsed
            .as_ref()
            .map(|value| {
                json!({
                    "authenticationMethod": value
                        .get("authenticationMethod")
                        .and_then(Value::as_str),
                    "isAuthenticated": value
                        .get("isAuthenticated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .unwrap_or_else(|| json!({})),
        "project_list" => summarize_project_list(parsed.as_ref()),
        "project_versions" => summarize_version_list(parsed.as_ref()),
        "project_filter_info" => summarize_project_filter_info(parsed.as_ref()),
        "bug_filter_info" => summarize_bug_filter_info(parsed.as_ref()),
        "bug_list" => summarize_bug_list_probe(parsed.as_ref()),
        "subscribed_bug_list" | "voted_bug_list" => summarize_module_call(parsed.as_ref()),
        "version_bugs" => summarize_bug_reference_call(parsed.as_ref()),
        "bug_get" => summarize_bug_get(parsed.as_ref()),
        "bug_subscription_vote_state" | "project_subscriptions" | "mutation" => {
            summarize_module_call(parsed.as_ref())
        }
        _ => json!({}),
    };
    if let (Some(object), Some(error)) = (safe.as_object_mut(), validation_error.as_ref()) {
        object.insert("validationError".to_owned(), json!(error));
    }

    Ok((
        ProbeResult {
            name,
            url,
            request: request_safe,
            status,
            ok: validation_error.is_none(),
            content_type,
            body_bytes: body.len(),
            json_keys,
            safe,
        },
        parsed,
    ))
}

fn validate_response(
    status: u16,
    kind: ResponseKind,
    body: &[u8],
) -> (Option<Value>, Option<String>) {
    let parsed = match serde_json::from_slice::<Value>(body) {
        Ok(value) => Some(value),
        Err(error) => return (None, Some(format!("invalid JSON response: {error}"))),
    };
    if !(200..300).contains(&status) {
        return (parsed, Some(format!("HTTP status {status}")));
    }

    let validation = match kind {
        ResponseKind::Auth => parsed
            .as_ref()
            .and_then(|value| value.get("isAuthenticated"))
            .and_then(Value::as_bool)
            .ok_or("auth response must contain boolean isAuthenticated")
            .and_then(|authenticated| {
                authenticated
                    .then_some(())
                    .ok_or("auth response reports an unauthenticated session")
            }),
        ResponseKind::EcsAccess => parsed
            .as_ref()
            .and_then(Value::as_object)
            .filter(|object| !object.is_empty())
            .map(|_| ())
            .ok_or("ECS access response must be a non-empty JSON object"),
        ResponseKind::DynamicListInfo => parsed
            .as_ref()
            .and_then(Value::as_object)
            .filter(|object| !object.is_empty())
            .map(|_| ())
            .ok_or("dynamic-list info response must be a non-empty JSON object"),
        ResponseKind::Entity => parsed
            .as_ref()
            .ok_or("entity response must be JSON")
            .and_then(|value| {
                bugboard::decode_bug_details(value)
                    .map(|_| ())
                    .map_err(|_| "invalid bug entity response")
            }),
        ResponseKind::ProjectList => DynamicListResponse::from_slice(body)
            .and_then(|response| bugboard::decode_project_rows(&response))
            .map(|_| ())
            .map_err(|_| "invalid project list response"),
        ResponseKind::VersionList => DynamicListResponse::from_slice(body)
            .and_then(|response| bugboard::decode_version_rows(&response))
            .map(|_| ())
            .map_err(|_| "invalid version list response"),
        ResponseKind::BugList => DynamicListResponse::from_slice(body)
            .and_then(|response| bugboard::decode_bug_rows(&response))
            .map(|_| ())
            .map_err(|_| "invalid bug list response"),
        ResponseKind::ModuleCall => ModuleCallResponse::<Value>::from_slice(body)
            .and_then(ModuleCallResponse::into_result)
            .map(|_| ())
            .map_err(|_| "invalid module call response"),
        ResponseKind::ProjectReferenceList => ModuleCallResponse::<Value>::from_slice(body)
            .and_then(ModuleCallResponse::into_result)
            .and_then(|result| bugboard::decode_project_references(&result))
            .map(|_| ())
            .map_err(|_| "invalid project reference list response"),
        ResponseKind::BugReferenceList => ModuleCallResponse::<Value>::from_slice(body)
            .and_then(ModuleCallResponse::into_result)
            .and_then(|result| bugboard::decode_bug_references(&result))
            .map(|_| ())
            .map_err(|_| "invalid bug reference list response"),
        ResponseKind::VersionBugList => ModuleCallResponse::<Value>::from_slice(body)
            .and_then(ModuleCallResponse::into_result)
            .and_then(|result| bugboard::decode_version_bug_references(&result))
            .map(|_| ())
            .map_err(|_| "invalid version bug list response"),
        ResponseKind::Mutation => ModuleCallResponse::<bool>::from_slice(body)
            .and_then(ModuleCallResponse::into_result)
            .map(|_| ())
            .map_err(|_| "invalid mutation response"),
    };

    (parsed, validation.err().map(ToOwned::to_owned))
}

fn summarize_request(request: &HttpRequest) -> Value {
    let body = request
        .body()
        .and_then(|body| serde_json::from_str::<Value>(body).ok());
    json!({
        "method": match request.method() {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
        },
        "headerNames": request
            .headers()
            .iter()
            .filter(|header| !header.name().eq_ignore_ascii_case("cookie"))
            .map(|header| header.name())
            .collect::<Vec<_>>(),
        "bodyBytes": request.body().map_or(0, str::len),
        "body": summarize_request_body(body.as_ref()),
    })
}

fn summarize_request_body(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return json!(null);
    };

    if let Some(dynamic_list) = value.get("dynamicList") {
        return json!({
            "kind": "dynamicList",
            "table": dynamic_list
                .get("MainTable")
                .and_then(|table| table.get("value"))
                .and_then(|value| value.get("Table"))
                .and_then(Value::as_str),
            "limit": value.get("limit").and_then(Value::as_u64),
            "showDeleted": value.get("showDeleted").and_then(Value::as_bool),
            "useHierarchy": value.get("useHierarchy").and_then(Value::as_bool),
        });
    }

    if let Some(reference) = value.get("reference") {
        return json!({
            "kind": "entityRead",
            "referenceType": reference.get("type").and_then(Value::as_str),
            "hasReferenceValue": reference.get("value").is_some(),
        });
    }

    if value.get("moduleName").is_some() || value.get("methodName").is_some() {
        return json!({
            "kind": "moduleCall",
            "moduleName": value.get("moduleName").and_then(Value::as_str),
            "methodName": value.get("methodName").and_then(Value::as_str),
            "parameterTypes": value
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|parameter| parameter.get("type").and_then(Value::as_str))
                .collect::<Vec<_>>(),
        });
    }

    json!({"kind": "json"})
}

fn summarize_project_list(value: Option<&Value>) -> Value {
    let rows = value
        .and_then(|value| serde_json::to_vec(value).ok())
        .and_then(|body| DynamicListResponse::from_slice(body).ok())
        .and_then(|response| bugboard::decode_project_rows(&response).ok())
        .unwrap_or_default();
    json!({
        "filters": [
            {"field": "ПометкаУдаления", "op": "eq", "value": false},
        ],
        "sorting": ["ПорядокГруппы asc", "Порядок asc"],
        "rows": {
            "rowCount": rows.len(),
            "sample": rows.iter().take(3).map(|row| json!({
                "name": row.title,
                "code": row.abbreviation,
            })).collect::<Vec<_>>(),
        },
    })
}

fn summarize_project_filter_info(value: Option<&Value>) -> Value {
    json!({
        "table": "e1c::bugboard::Багборд::Проекты",
        "hasMetadata": value.is_some(),
    })
}

fn summarize_version_list(value: Option<&Value>) -> Value {
    let rows = value
        .and_then(|value| serde_json::to_vec(value).ok())
        .and_then(|body| DynamicListResponse::from_slice(body).ok())
        .and_then(|response| bugboard::decode_version_rows(&response).ok())
        .unwrap_or_default();
    let duplicate_title_count = rows
        .iter()
        .enumerate()
        .filter(|(index, row)| {
            row.title.as_ref().is_some_and(|title| {
                rows[..*index]
                    .iter()
                    .any(|candidate| candidate.title.as_ref() == Some(title))
            })
        })
        .count();

    json!({
        "rows": {
            "rowCount": rows.len(),
            "duplicateTitleCount": duplicate_title_count,
            "sample": rows.iter().take(3).map(|row| json!({
                "title": row.title,
                "sourceOrder": row.source_order,
            })).collect::<Vec<_>>(),
        },
    })
}

fn summarize_bug_list_probe(value: Option<&Value>) -> Value {
    let rows = value
        .and_then(|value| serde_json::to_vec(value).ok())
        .and_then(|body| DynamicListResponse::from_slice(body).ok())
        .and_then(|response| bugboard::decode_bug_rows(&response).ok())
        .unwrap_or_default();
    json!({
        "filters": [
            {"field": "КУдалению", "op": "eq", "value": false},
        ],
        "sorting": ["ДатаПоследнегоОбновления asc"],
        "rows": {
            "rowCount": rows.len(),
            "sample": rows.iter().take(3).map(|row| json!({
                "number": row.number,
                "title": row.title,
                "status": row.status,
                "publishedAt": row.published_at,
                "updatedAt": row.updated_at,
            })).collect::<Vec<_>>(),
        },
    })
}

fn summarize_bug_filter_info(value: Option<&Value>) -> Value {
    json!({
        "table": "e1c::bugboard::Багборд::Ошибки",
        "hasMetadata": value.is_some(),
    })
}

fn summarize_module_call(value: Option<&Value>) -> Value {
    value
        .map(|value| {
            json!({
                "debugExitReason": value.get("debugExitReason"),
                "hasResult": value.get("result").is_some(),
            })
        })
        .unwrap_or_else(|| json!({}))
}

fn summarize_bug_reference_call(value: Option<&Value>) -> Value {
    let result = value.and_then(|value| value.get("result"));
    let items = result.and_then(|result| {
        result
            .as_array()
            .or_else(|| result.pointer("/value/items").and_then(Value::as_array))
    });
    let mut item_types = items
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("type").and_then(Value::as_str))
        .collect::<Vec<_>>();
    item_types.sort_unstable();
    item_types.dedup();
    let fields = result
        .and_then(|result| result.get("value"))
        .and_then(Value::as_object)
        .map(|fields| {
            fields
                .iter()
                .map(|(name, field)| {
                    let items = field.pointer("/value/items").and_then(Value::as_array);
                    let mut item_types = items
                        .into_iter()
                        .flatten()
                        .filter_map(|item| item.get("type").and_then(Value::as_str))
                        .collect::<Vec<_>>();
                    item_types.sort_unstable();
                    item_types.dedup();
                    json!({
                        "name": name,
                        "type": field.get("type").and_then(Value::as_str),
                        "itemCount": items.map_or(0, Vec::len),
                        "itemTypes": item_types,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    json!({
        "debugExitReason": value.and_then(|value| value.get("debugExitReason")),
        "resultShape": match result {
            Some(Value::Array(_)) => "array",
            Some(result) if result.pointer("/value/items").is_some() => "value.items",
            Some(result) if result.get("value").is_some_and(Value::is_object) => "typed_object",
            Some(_) => "other",
            None => "missing",
        },
        "resultType": result.and_then(|result| result.get("type")).and_then(Value::as_str),
        "itemCount": items.map_or(0, Vec::len),
        "itemTypes": item_types,
        "fields": fields,
    })
}

fn summarize_bug_get(value: Option<&Value>) -> Value {
    let object = value.and_then(|value| value.get("object")).or(value);

    value
        .and_then(|value| bugboard::decode_bug_details(value).ok())
        .map(|details| {
            json!({
                "number": details.number,
                "title": details.title,
                "status": details.status,
                "publishedAt": details.published_at,
                "updatedAt": details.updated_at,
                "linkFragmentPresent": object.is_some_and(|object| object.get("ФрагментСсылки").is_some()),
                "historyCount": details.history.len(),
            })
        })
        .unwrap_or_else(|| json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use e1c_element_rpc::bugboard::SUBSCRIPTION_MODULE;

    #[test]
    fn first_bug_ref_reads_first_field_value_reference() {
        let value = json!({
            "rows": [{
                "fieldValues": [{
                    "type": "e1c::bugboard::Багборд::Ошибки.Reference",
                    "value": "bug-ref",
                }],
            }],
        });

        assert_eq!(first_bug_ref(Some(&value)).as_deref(), Some("bug-ref"));
    }

    #[test]
    fn reference_export_reports_opt_in_refs_for_mutation_envs() {
        let projects = json!({
            "rows": [{
                "fieldValues": [{
                    "type": "e1c::bugboard::Багборд::Проекты.Reference",
                    "value": "project-ref",
                }],
            }],
        });
        let bugs = json!({
            "rows": [{
                "fieldValues": [{
                    "type": "e1c::bugboard::Багборд::Ошибки.Reference",
                    "value": "bug-ref",
                }],
            }],
        });

        let result = reference_export_result(Some(&projects), Some(&bugs));

        assert_eq!(result.name, "reference_export");
        assert_eq!(result.safe["projectRefEnv"], "BUGBOARD_PROJECT_REF");
        assert_eq!(result.safe["projectRef"], "project-ref");
        assert_eq!(result.safe["bugRefEnv"], "BUGBOARD_BUG_REF");
        assert_eq!(result.safe["bugRef"], "bug-ref");
    }

    #[test]
    fn cookie_input_accepts_env_value() {
        assert_eq!(
            cookie_header_from_input(
                "session=value".to_owned(),
                || -> Result<String, std::io::Error> { unreachable!() }
            )
            .unwrap()
            .as_str(),
            "session=value"
        );
    }

    #[test]
    fn cookie_input_reads_dash_from_stdin() {
        assert_eq!(
            cookie_header_from_input("-".to_owned(), || Ok("session=value\r\n".to_owned()))
                .unwrap()
                .as_str(),
            "session=value"
        );
    }

    #[test]
    fn request_summary_redacts_cookie_and_reference_values() {
        let request = bugboard::bug_get_request("secret-ref")
            .unwrap()
            .with_cookie_header("session=value")
            .unwrap();

        let summary = summarize_request(&request);
        let rendered = serde_json::to_string(&summary).unwrap();

        assert!(!rendered.contains("session=value"));
        assert!(!rendered.contains("secret-ref"));
        assert_eq!(summary["body"]["kind"], "entityRead");
        assert_eq!(
            summary["body"]["referenceType"],
            "e1c::bugboard::Багборд::Ошибки.Reference"
        );
        assert_eq!(summary["body"]["hasReferenceValue"], true);
    }

    #[test]
    fn project_subscription_roundtrip_dry_run_describes_restore_plan() {
        let result = project_subscription_roundtrip_dry_run_result(false).unwrap();
        let rendered = serde_json::to_string(&result).unwrap();

        assert_eq!(result.name, "mutation_dry_run");
        assert!(result.ok);
        assert_eq!(result.safe["requested"], PROJECT_SUBSCRIPTION_ROUNDTRIP);
        assert_eq!(result.safe["requiredReferenceEnv"], "BUGBOARD_PROJECT_REF");
        assert_eq!(result.safe["willRestoreInitialState"], true);
        assert_eq!(
            result.request["body"]["kind"],
            "projectSubscriptionRoundtrip"
        );
        assert!(!rendered.contains("project-ref"));
    }

    #[test]
    fn project_subscription_state_requires_a_typed_reference_list() {
        let value = json!({
            "debugExitReason": "NONE",
            "result": {
                "type": "Std::Collections::Array<e1c::bugboard::Багборд::Проекты.Reference>",
                "value": {
                    "items": [
                        {"type": PROJECT_REFERENCE_TYPE, "value": "project-ref"},
                    ],
                },
            },
        });

        assert!(project_subscription_state_from_value(&value, "project-ref").unwrap());
        assert!(!project_subscription_state_from_value(&value, "other-project").unwrap());
        assert!(
            project_subscription_state_from_value(&json!({"result": {}}), "project-ref").is_err()
        );
    }

    #[test]
    fn current_user_bug_list_requests_use_module_calls() {
        let subscribed_request = bugboard::subscribed_bug_list_request().unwrap();
        let subscribed: Value = serde_json::from_str(subscribed_request.body().unwrap()).unwrap();
        let voted_request =
            bugboard::voted_bug_list_request(bugboard::BugVoteKind::FixImportant).unwrap();
        let voted: Value = serde_json::from_str(voted_request.body().unwrap()).unwrap();

        assert_eq!(subscribed["moduleName"], SUBSCRIPTION_MODULE);
        assert_eq!(subscribed["methodName"], "ПолучитьПодпискиНаОшибки");
        assert_eq!(subscribed["parameters"].as_array().unwrap().len(), 0);
        assert_eq!(voted["moduleName"], SUBSCRIPTION_MODULE);
        assert_eq!(
            voted["methodName"],
            "ОшибкиПоКоторымЕстьГолосДляМеняИсправлениеВажно"
        );
        assert_eq!(voted["parameters"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn first_bug_ref_reads_nested_module_call_result() {
        let value = json!({
            "debugExitReason": "NONE",
            "result": {
                "type": "Std::Collections::Array<e1c::bugboard::Багборд::Ошибки.Reference>",
                "value": {
                    "items": [
                        {"type": BUG_REFERENCE_TYPE, "value": "bug-ref"},
                    ],
                },
            },
        });

        assert_eq!(first_bug_ref(Some(&value)).as_deref(), Some("bug-ref"));
    }

    #[test]
    fn first_bug_ref_ignores_missing_reference() {
        assert_eq!(first_bug_ref(Some(&json!({}))), None);
        assert_eq!(first_bug_ref(Some(&json!({"rows": "not-array"}))), None);
        assert_eq!(first_bug_ref(Some(&json!({"rows": []}))), None);
        assert_eq!(first_bug_ref(Some(&json!({"rows": ["not-object"]}))), None);
        assert_eq!(
            first_bug_ref(Some(&json!({"rows": [{"fieldValues": "not-array"}]}))),
            None
        );
        assert_eq!(
            first_bug_ref(Some(&json!({"rows": [{"fieldValues": []}]}))),
            None
        );
        assert_eq!(
            first_bug_ref(Some(&json!({"rows": [{"fieldValues": [{"type": "T"}]}]}))),
            None
        );
        assert_eq!(
            first_bug_ref(Some(&json!({
                "rows": [{
                    "fieldValues": [{
                        "type": "e1c::bugboard::Багборд::Проекты.Reference",
                        "value": "project-ref",
                    }],
                }],
            }))),
            None
        );
    }

    #[test]
    fn project_list_summary_uses_the_bugboard_decoder_without_exporting_refs() {
        let value = json!({
            "rows": [{
                "fieldValues": [
                    {"type": "e1c::bugboard::Багборд::Проекты.Reference", "value": "project-ref"},
                    {"type": "Std::String", "value": "CODE"},
                    {"type": "Std::String", "value": "Project name"},
                    false,
                    "2026-07-10",
                    1,
                    2
                ],
            }],
        });

        let summary = summarize_project_list(Some(&value));
        let rendered = serde_json::to_string(&summary).unwrap();
        assert_eq!(summary["rows"]["rowCount"], 1);
        assert_eq!(summary["rows"]["sample"][0]["name"], "Project name");
        assert!(!rendered.contains("project-ref"));
    }

    #[test]
    fn malformed_success_responses_fail_validation() {
        for (kind, body) in [
            (ResponseKind::Auth, br#"{}"#.as_slice()),
            (
                ResponseKind::Auth,
                br#"{"isAuthenticated":false}"#.as_slice(),
            ),
            (ResponseKind::EcsAccess, br#"{}"#.as_slice()),
            (ResponseKind::DynamicListInfo, br#"{}"#.as_slice()),
            (ResponseKind::ProjectList, br#"{}"#.as_slice()),
            (
                ResponseKind::ModuleCall,
                br#"{"debugExitReason":null}"#.as_slice(),
            ),
            (
                ResponseKind::BugReferenceList,
                br#"{"debugExitReason":null,"result":{}}"#.as_slice(),
            ),
            (
                ResponseKind::Mutation,
                br#"{"debugExitReason":null,"result":{}}"#.as_slice(),
            ),
            (
                ResponseKind::ModuleCall,
                br#"{"debugExitReason":"ERROR","result":{}}"#.as_slice(),
            ),
            (ResponseKind::EcsAccess, b"not-json".as_slice()),
            (ResponseKind::Entity, br#"{}"#.as_slice()),
        ] {
            let (_, error) = validate_response(200, kind, body);
            assert!(error.is_some());
        }
    }

    #[test]
    fn compensation_failure_is_reported_after_first_step_validation_fails() {
        let first = MutationAttempt {
            status: 200,
            ok: false,
            error: Some("invalid module call response".to_owned()),
        };
        let restore = MutationAttempt {
            status: 503,
            ok: false,
            error: Some("HTTP status 503".to_owned()),
        };
        let (ok, safe) = roundtrip_verdict(
            false,
            &first,
            &Err("state validation failed".to_owned()),
            &restore,
            &Ok(false),
        );

        assert!(!ok);
        assert_eq!(safe["steps"][0]["error"], "invalid module call response");
        assert_eq!(safe["steps"][2]["kind"], "restore");
        assert_eq!(safe["steps"][2]["error"], "HTTP status 503");
        assert_eq!(safe["restored"], true);
    }
}
