use std::time::Duration;

use e1c_element_rpc::{
    DynamicListResponse, HttpMethod, HttpRequest, ModuleCallResponse, Session, bugboard,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    config::SessionConfig,
    errors::{ToolFailure, http_failure},
};

const INCONSISTENT_VERSION_CONTENT_TYPE: &str = "application/vnd.e1c.g5rt.inconsistent_version";

pub(crate) struct BugboardClient {
    client: reqwest::Client,
    session: Session,
    base_url: reqwest::Url,
    g5_version: Mutex<Option<String>>,
}

impl BugboardClient {
    pub(crate) fn new(config: SessionConfig) -> Result<Self, ToolFailure> {
        Self::with_base_url(config, bugboard::BUGBOARD_BASE_URL)
    }

    fn with_base_url(config: SessionConfig, base_url: &str) -> Result<Self, ToolFailure> {
        let session = Session::from_cookie_header(config.cookie).map_err(|error| {
            ToolFailure::new(
                "config_error",
                "BUGBOARD_COOKIE is not a valid Cookie header.",
                json!({"source": error.to_string()}),
            )
        })?;
        let base_url = reqwest::Url::parse(base_url)
            .map_err(|error| ToolFailure::internal(error.to_string()))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| ToolFailure::internal(error.to_string()))?;

        Ok(Self {
            client,
            session,
            base_url,
            g5_version: Mutex::new(None),
        })
    }

    pub(crate) async fn execute(
        &self,
        operation: &'static str,
        request: HttpRequest,
    ) -> Result<Value, ToolFailure> {
        let body = self.execute_read_body(operation, request).await?;
        decode_json(operation, &body)
    }

    pub(crate) async fn execute_unversioned(
        &self,
        operation: &'static str,
        request: HttpRequest,
    ) -> Result<Value, ToolFailure> {
        let body = self
            .execute_body(operation, request, None)
            .await
            .map_err(|error| error.into_tool_failure(operation))?;
        decode_json(operation, &body)
    }

    async fn execute_read_body(
        &self,
        operation: &'static str,
        request: HttpRequest,
    ) -> Result<Vec<u8>, ToolFailure> {
        let g5_version = self.g5_version().await?;
        match self
            .execute_body(operation, request.clone(), Some(&g5_version))
            .await
        {
            Ok(body) => Ok(body),
            Err(RequestFailure::InconsistentVersion) => {
                let refreshed = self.refresh_g5_version(&g5_version).await?;
                self.execute_body(operation, request, Some(&refreshed))
                    .await
                    .map_err(|error| error.into_tool_failure(operation))
            }
            Err(error) => Err(error.into_tool_failure(operation)),
        }
    }

    async fn execute_mutation_body(
        &self,
        operation: &'static str,
        request: HttpRequest,
    ) -> Result<Vec<u8>, ToolFailure> {
        let g5_version = self.g5_version().await?;
        match self
            .execute_body(operation, request, Some(&g5_version))
            .await
        {
            Ok(body) => Ok(body),
            Err(RequestFailure::InconsistentVersion) => {
                self.refresh_g5_version(&g5_version).await?;
                Err(ToolFailure::bugboard_updated(operation))
            }
            Err(error) => Err(error.into_tool_failure(operation)),
        }
    }

    async fn g5_version(&self) -> Result<String, ToolFailure> {
        let mut cached = self.g5_version.lock().await;
        if let Some(version) = cached.as_ref() {
            return Ok(version.clone());
        }

        let version = self.fetch_g5_version().await?;
        *cached = Some(version.clone());
        Ok(version)
    }

    async fn refresh_g5_version(&self, stale: &str) -> Result<String, ToolFailure> {
        let mut cached = self.g5_version.lock().await;
        if let Some(version) = cached.as_ref().filter(|version| version.as_str() != stale) {
            return Ok(version.clone());
        }

        *cached = None;
        let version = self.fetch_g5_version().await?;
        *cached = Some(version.clone());
        Ok(version)
    }

    async fn fetch_g5_version(&self) -> Result<String, ToolFailure> {
        let response = self
            .client
            .get(self.base_url.clone())
            .header(reqwest::header::COOKIE, self.session.cookie_header())
            .send()
            .await
            .map_err(|error| ToolFailure::transport(error.to_string()))?;
        if response.url().origin() != self.base_url.origin() {
            return Err(ToolFailure::new(
                "not_authenticated",
                "Bugboard redirected the configured session to authentication.",
                json!({"operation": "g5_bootstrap"}),
            ));
        }

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = response
            .bytes()
            .await
            .map_err(|error| ToolFailure::transport(error.to_string()))?
            .to_vec();
        if !(200..300).contains(&status) {
            let parsed = serde_json::from_slice::<Value>(&body).ok();
            return Err(http_failure(
                "g5_bootstrap",
                status,
                content_type,
                body.len(),
                parsed.as_ref(),
            ));
        }

        let shell = std::str::from_utf8(&body)
            .map_err(|error| ToolFailure::bugboard_changed("g5_bootstrap", error))?;
        match bugboard::decode_g5_version_from_shell(shell) {
            Ok(version) => Ok(version),
            Err(error) => {
                if !self.session_is_authenticated().await? {
                    return Err(ToolFailure::new(
                        "not_authenticated",
                        "Bugboard rejected the configured session.",
                        json!({"operation": "g5_bootstrap"}),
                    ));
                }
                Err(ToolFailure::bugboard_changed("g5_bootstrap", error))
            }
        }
    }

    async fn session_is_authenticated(&self) -> Result<bool, ToolFailure> {
        let url = self
            .base_url
            .join("/sys/auth/status")
            .map_err(|error| ToolFailure::internal(error.to_string()))?;
        let request = HttpRequest::get(url.to_string())?;
        let body = self
            .execute_body("g5_auth_status", request, None)
            .await
            .map_err(|error| error.into_tool_failure("g5_auth_status"))?;
        let status = decode_json("g5_auth_status", &body)?;
        status
            .get("isAuthenticated")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                ToolFailure::new(
                    "bugboard_changed",
                    "Bugboard auth status did not contain isAuthenticated.",
                    json!({"operation": "g5_auth_status"}),
                )
            })
    }

    async fn execute_body(
        &self,
        operation: &'static str,
        request: HttpRequest,
        g5_version: Option<&str>,
    ) -> Result<Vec<u8>, RequestFailure> {
        let request = match g5_version {
            Some(version) => request
                .with_header("X-G5-Version", version)
                .map_err(|error| ToolFailure::internal(error.to_string()))?,
            None => request,
        };
        let request = request
            .with_session(&self.session)
            .map_err(|error| ToolFailure::internal(error.to_string()))?;
        let (method, url, headers, body) = request.into_parts();
        let method = match method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
        };
        let mut builder = self.client.request(method, url);
        for header in headers {
            let (name, value) = header.into_parts();
            builder = builder.header(name, value);
        }
        if let Some(body) = body {
            builder = builder.body(body);
        }

        let response = builder
            .send()
            .await
            .map_err(|error| ToolFailure::transport(error.to_string()))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = response
            .bytes()
            .await
            .map_err(|error| ToolFailure::transport(error.to_string()))?
            .to_vec();

        if !(200..300).contains(&status) {
            let parsed = serde_json::from_slice::<Value>(&body).ok();
            return Err(
                http_failure(operation, status, content_type, body.len(), parsed.as_ref()).into(),
            );
        }

        if is_inconsistent_version(&content_type) {
            return Err(RequestFailure::InconsistentVersion);
        }

        Ok(body)
    }

    #[cfg(test)]
    async fn cached_g5_version(&self) -> Option<String> {
        self.g5_version.lock().await.clone()
    }
}

enum RequestFailure {
    Failure(ToolFailure),
    InconsistentVersion,
}

impl RequestFailure {
    fn into_tool_failure(self, operation: &'static str) -> ToolFailure {
        match self {
            Self::Failure(error) => error,
            Self::InconsistentVersion => ToolFailure::bugboard_updated(operation),
        }
    }
}

impl From<ToolFailure> for RequestFailure {
    fn from(error: ToolFailure) -> Self {
        Self::Failure(error)
    }
}

fn is_inconsistent_version(content_type: &str) -> bool {
    content_type.split(';').next().is_some_and(|value| {
        value
            .trim()
            .eq_ignore_ascii_case(INCONSISTENT_VERSION_CONTENT_TYPE)
    })
}

fn decode_json(operation: &'static str, body: &[u8]) -> Result<Value, ToolFailure> {
    serde_json::from_slice(body).map_err(|_| {
        ToolFailure::new(
            "bugboard_changed",
            "Bugboard returned non-JSON data.",
            json!({"operation": operation, "body_bytes": body.len()}),
        )
    })
}

impl BugboardClient {
    pub(crate) async fn execute_dynamic_list(
        &self,
        operation: &'static str,
        request: HttpRequest,
    ) -> Result<DynamicListResponse, ToolFailure> {
        let body = self.execute_read_body(operation, request).await?;
        DynamicListResponse::from_slice(&body).map_err(|error| {
            ToolFailure::new(
                "bugboard_changed",
                "Bugboard returned an unexpected dynamic-list response.",
                json!({"operation": operation, "source": error.to_string()}),
            )
        })
    }

    pub(crate) async fn execute_module_call(
        &self,
        operation: &'static str,
        request: HttpRequest,
    ) -> Result<Value, ToolFailure> {
        let body = self.execute_read_body(operation, request).await?;
        ModuleCallResponse::<Value>::from_slice(&body)
            .and_then(ModuleCallResponse::into_result)
            .map_err(|error| ToolFailure::bugboard_changed(operation, error))
    }

    pub(crate) async fn execute_module_call_bool(
        &self,
        operation: &'static str,
        request: HttpRequest,
    ) -> Result<bool, ToolFailure> {
        let body = self.execute_mutation_body(operation, request).await?;
        decode_mutation_response(operation, &body)
    }
}

fn decode_mutation_response(operation: &'static str, body: &[u8]) -> Result<bool, ToolFailure> {
    let response = ModuleCallResponse::<bool>::from_slice(body)
        .map_err(|error| ToolFailure::bugboard_changed(operation, error))?;
    if response.debug_exit_reason().is_some() {
        return Err(ToolFailure::new(
            "mutation_rejected",
            "Bugboard rejected the mutation.",
            json!({"operation": operation}),
        ));
    }

    response
        .into_result()
        .map_err(|error| ToolFailure::bugboard_changed(operation, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{
        Router,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use tokio::task::{JoinHandle, JoinSet};

    const TEST_SHELL: &str = r#"
        <script>
            var __gSrv_APP_HASH = 'app_hash';
            var __gSrv_SRV_VERSION = '9.2.9-12';
        </script>
    "#;
    const STALE_SHELL: &str = r#"
        <script>
            var __gSrv_APP_HASH = 'stale_hash';
            var __gSrv_SRV_VERSION = '1';
        </script>
    "#;
    const FRESH_SHELL: &str = r#"
        <script>
            var __gSrv_APP_HASH = 'fresh_hash';
            var __gSrv_SRV_VERSION = '2';
        </script>
    "#;

    async fn spawn_test_server(router: Router) -> (String, JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (base_url, task)
    }

    #[tokio::test]
    async fn concurrent_ui_requests_share_cookie_only_g5_bootstrap() {
        let bootstrap_count = Arc::new(AtomicUsize::new(0));
        let bootstrap_cookie = Arc::new(StdMutex::new(None));
        let ui_headers = Arc::new(StdMutex::new(Vec::new()));
        let router = Router::new()
            .route(
                "/",
                get({
                    let bootstrap_count = Arc::clone(&bootstrap_count);
                    let bootstrap_cookie = Arc::clone(&bootstrap_cookie);
                    move |headers: HeaderMap| async move {
                        bootstrap_count.fetch_add(1, Ordering::Relaxed);
                        *bootstrap_cookie.lock().unwrap() = headers
                            .get(reqwest::header::COOKIE)
                            .and_then(|value| value.to_str().ok())
                            .map(ToOwned::to_owned);
                        ([("content-type", "text/html")], TEST_SHELL)
                    }
                }),
            )
            .route(
                "/ui/read",
                get({
                    let ui_headers = Arc::clone(&ui_headers);
                    move |headers: HeaderMap| async move {
                        ui_headers.lock().unwrap().push((
                            headers
                                .get("x-g5-version")
                                .and_then(|value| value.to_str().ok())
                                .map(ToOwned::to_owned),
                            headers
                                .get(reqwest::header::COOKIE)
                                .and_then(|value| value.to_str().ok())
                                .map(ToOwned::to_owned),
                        ));
                        ([("content-type", "application/json")], r#"{"ok":true}"#)
                    }
                }),
            );
        let (base_url, server) = spawn_test_server(router).await;
        let client = Arc::new(
            BugboardClient::with_base_url(
                SessionConfig {
                    cookie: "session=value".to_owned(),
                },
                &base_url,
            )
            .unwrap(),
        );

        let mut calls = JoinSet::new();
        for _ in 0..8 {
            let client = Arc::clone(&client);
            let url = format!("{base_url}/ui/read");
            calls.spawn(async move {
                client
                    .execute("test_read", HttpRequest::get(url).unwrap())
                    .await
            });
        }
        while let Some(result) = calls.join_next().await {
            assert_eq!(result.unwrap().unwrap()["ok"], true);
        }

        assert_eq!(bootstrap_count.load(Ordering::Relaxed), 1);
        assert_eq!(
            bootstrap_cookie.lock().unwrap().as_deref(),
            Some("session=value")
        );
        let headers = ui_headers.lock().unwrap();
        assert_eq!(headers.len(), 8);
        assert!(headers.iter().all(|(g5, cookie)| {
            g5.as_deref() == Some("app_hash%2C9.2.9-12")
                && cookie.as_deref() == Some("session=value")
        }));
        assert_eq!(
            client.cached_g5_version().await.as_deref(),
            Some("app_hash%2C9.2.9-12")
        );
        server.abort();
    }

    #[tokio::test]
    async fn unversioned_auth_status_does_not_bootstrap_g5() {
        let bootstrap_count = Arc::new(AtomicUsize::new(0));
        let auth_g5 = Arc::new(StdMutex::new(None));
        let router = Router::new()
            .route(
                "/",
                get({
                    let bootstrap_count = Arc::clone(&bootstrap_count);
                    move || async move {
                        bootstrap_count.fetch_add(1, Ordering::Relaxed);
                        ([("content-type", "text/html")], TEST_SHELL)
                    }
                }),
            )
            .route(
                "/sys/auth/status",
                get({
                    let auth_g5 = Arc::clone(&auth_g5);
                    move |headers: HeaderMap| async move {
                        *auth_g5.lock().unwrap() = headers
                            .get("x-g5-version")
                            .and_then(|value| value.to_str().ok())
                            .map(ToOwned::to_owned);
                        (
                            [("content-type", "application/json")],
                            r#"{"isAuthenticated":true}"#,
                        )
                    }
                }),
            );
        let (base_url, server) = spawn_test_server(router).await;
        let client = BugboardClient::with_base_url(
            SessionConfig {
                cookie: "session=value".to_owned(),
            },
            &base_url,
        )
        .unwrap();

        let response = client
            .execute_unversioned(
                "bugboard_auth_status",
                HttpRequest::get(format!("{base_url}/sys/auth/status")).unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response["isAuthenticated"], true);
        assert_eq!(bootstrap_count.load(Ordering::Relaxed), 0);
        assert!(auth_g5.lock().unwrap().is_none());
        assert!(client.cached_g5_version().await.is_none());
        server.abort();
    }

    #[tokio::test]
    async fn same_origin_auth_shell_is_reported_as_not_authenticated() {
        let auth_status_count = Arc::new(AtomicUsize::new(0));
        let ui_count = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route(
                "/",
                get(|| async {
                    (
                        [("content-type", "text/html")],
                        "<html>authentication required</html>",
                    )
                }),
            )
            .route(
                "/sys/auth/status",
                get({
                    let auth_status_count = Arc::clone(&auth_status_count);
                    move || async move {
                        auth_status_count.fetch_add(1, Ordering::Relaxed);
                        (
                            [("content-type", "application/json")],
                            r#"{"isAuthenticated":false}"#,
                        )
                    }
                }),
            )
            .route(
                "/ui/read",
                get({
                    let ui_count = Arc::clone(&ui_count);
                    move || async move {
                        ui_count.fetch_add(1, Ordering::Relaxed);
                        ([("content-type", "application/json")], r#"{"ok":true}"#)
                    }
                }),
            );
        let (base_url, server) = spawn_test_server(router).await;
        let client = BugboardClient::with_base_url(
            SessionConfig {
                cookie: "session=value".to_owned(),
            },
            &base_url,
        )
        .unwrap();

        let error = client
            .execute(
                "test_read",
                HttpRequest::get(format!("{base_url}/ui/read")).unwrap(),
            )
            .await
            .unwrap_err()
            .into_result();

        assert_eq!(
            error
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/error/code")),
            Some(&json!("not_authenticated"))
        );
        assert_eq!(auth_status_count.load(Ordering::Relaxed), 1);
        assert_eq!(ui_count.load(Ordering::Relaxed), 0);
        server.abort();
    }

    #[tokio::test]
    async fn stale_read_refreshes_g5_and_retries_once() {
        let bootstrap_count = Arc::new(AtomicUsize::new(0));
        let seen_versions = Arc::new(StdMutex::new(Vec::new()));
        let router = Router::new()
            .route(
                "/",
                get({
                    let bootstrap_count = Arc::clone(&bootstrap_count);
                    move || async move {
                        let call = bootstrap_count.fetch_add(1, Ordering::Relaxed);
                        let shell = if call == 0 { STALE_SHELL } else { FRESH_SHELL };
                        ([("content-type", "text/html")], shell)
                    }
                }),
            )
            .route(
                "/ui/read",
                get({
                    let seen_versions = Arc::clone(&seen_versions);
                    move |headers: HeaderMap| async move {
                        let version = headers
                            .get("x-g5-version")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_owned();
                        seen_versions.lock().unwrap().push(version.clone());
                        if version == "stale_hash%2C1" {
                            (
                                StatusCode::OK,
                                [("content-type", INCONSISTENT_VERSION_CONTENT_TYPE)],
                                "",
                            )
                        } else {
                            (
                                StatusCode::OK,
                                [("content-type", "application/json")],
                                r#"{"ok":true}"#,
                            )
                        }
                    }
                }),
            );
        let (base_url, server) = spawn_test_server(router).await;
        let client = BugboardClient::with_base_url(
            SessionConfig {
                cookie: "session=value".to_owned(),
            },
            &base_url,
        )
        .unwrap();

        let response = client
            .execute(
                "test_read",
                HttpRequest::get(format!("{base_url}/ui/read")).unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response["ok"], true);
        assert_eq!(bootstrap_count.load(Ordering::Relaxed), 2);
        assert_eq!(
            *seen_versions.lock().unwrap(),
            ["stale_hash%2C1", "fresh_hash%2C2"]
        );
        assert_eq!(
            client.cached_g5_version().await.as_deref(),
            Some("fresh_hash%2C2")
        );
        server.abort();
    }

    #[tokio::test]
    async fn stale_mutation_refreshes_cache_without_replay() {
        let bootstrap_count = Arc::new(AtomicUsize::new(0));
        let mutation_count = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route(
                "/",
                get({
                    let bootstrap_count = Arc::clone(&bootstrap_count);
                    move || async move {
                        let call = bootstrap_count.fetch_add(1, Ordering::Relaxed);
                        let shell = if call == 0 { STALE_SHELL } else { FRESH_SHELL };
                        ([("content-type", "text/html")], shell)
                    }
                }),
            )
            .route(
                "/ui/mutate",
                post({
                    let mutation_count = Arc::clone(&mutation_count);
                    move |headers: HeaderMap| async move {
                        mutation_count.fetch_add(1, Ordering::Relaxed);
                        let fresh = headers
                            .get("x-g5-version")
                            .and_then(|value| value.to_str().ok())
                            == Some("fresh_hash%2C2");
                        if fresh {
                            (
                                StatusCode::OK,
                                [("content-type", "application/json")],
                                r#"{"debugExitReason":null,"result":true}"#,
                            )
                        } else {
                            (
                                StatusCode::OK,
                                [("content-type", INCONSISTENT_VERSION_CONTENT_TYPE)],
                                "",
                            )
                        }
                    }
                }),
            );
        let (base_url, server) = spawn_test_server(router).await;
        let client = BugboardClient::with_base_url(
            SessionConfig {
                cookie: "session=value".to_owned(),
            },
            &base_url,
        )
        .unwrap();
        let mutation_request =
            || HttpRequest::json(format!("{base_url}/ui/mutate"), json!({"value": true})).unwrap();

        let failure = client
            .execute_module_call_bool("test_mutation", mutation_request())
            .await
            .unwrap_err();
        assert!(!failure.mutation_delivery_is_uncertain());
        let error = failure.into_result();

        assert_eq!(
            error
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/error/code")),
            Some(&json!("bugboard_updated"))
        );
        assert_eq!(mutation_count.load(Ordering::Relaxed), 1);
        assert_eq!(bootstrap_count.load(Ordering::Relaxed), 2);
        assert!(
            client
                .execute_module_call_bool("test_mutation", mutation_request())
                .await
                .unwrap()
        );
        assert_eq!(mutation_count.load(Ordering::Relaxed), 2);
        assert_eq!(bootstrap_count.load(Ordering::Relaxed), 2);
        server.abort();
    }

    #[test]
    fn inconsistent_version_content_type_allows_parameters_and_case() {
        assert!(is_inconsistent_version(
            "Application/Vnd.E1c.G5rt.Inconsistent_Version; charset=utf-8"
        ));
        assert!(!is_inconsistent_version("application/json"));
    }

    #[test]
    fn mutation_response_distinguishes_rejection_from_shape_drift() {
        let rejected = decode_mutation_response(
            "project_subscribe",
            br#"{"debugExitReason":"ERROR","result":false}"#,
        )
        .unwrap_err();
        let malformed = decode_mutation_response("project_subscribe", br#"{}"#).unwrap_err();

        assert!(!rejected.mutation_delivery_is_uncertain());
        assert!(malformed.mutation_delivery_is_uncertain());
    }
}
