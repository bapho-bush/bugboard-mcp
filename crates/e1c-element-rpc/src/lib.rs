//! Wire-level request descriptors and response envelopes for 1C Element RPC.
//!
//! The crate deliberately does not execute HTTP requests. Callers may attach a
//! [`Session`] to a descriptor and pass its parts to any HTTP client.
//!
//! ```
//! # fn main() -> Result<(), e1c_element_rpc::Error> {
//! use e1c_element_rpc::{ElementRpc, Session};
//!
//! let rpc = ElementRpc::with_base_url("https://example.invalid/app")?
//!     .with_locale("en-US", "en_US")?;
//! let request = rpc
//!     .call("e1c::example::Module", "Method")?
//!     .param("Std::String", "value")?
//!     .request()?
//!     .with_session(&Session::from_cookie_header("session=value")?)?;
//!
//! assert_eq!(
//!     request.url(),
//!     "https://example.invalid/app/ui/module/call?locale=en-US&pubLocale=en_US"
//! );
//! # Ok(())
//! # }
//! ```

use serde::{Deserialize, Serialize, de::DeserializeOwned};

const DEFAULT_LOCALE: &str = "ru-RU";
const DEFAULT_PUB_LOCALE: &str = "ru_RU";
const MODULE_CALL_PATH: &str = "/ui/module/call";
const DYNAMIC_LIST_PATH: &str = "/ui/dynamic-list";
const DYNAMIC_LIST_INFO_PATH: &str = "/ui/dynamic-list-info";
const ENTITY_READ_PATH: &str = "/ui/entity/read";
const PRESENTATION_PATH: &str = "/ui/presentation";

pub mod bugboard;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Header {
    name: String,
    value: String,
}

impl Header {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Result<Self, Error> {
        let name = name.into();
        let value = value.into();

        if name.trim().is_empty() {
            return Err(Error::EmptyHeaderName);
        }
        if name.chars().any(|ch| matches!(ch, '\r' | '\n')) {
            return Err(Error::InvalidHeaderName);
        }
        if value.chars().any(|ch| matches!(ch, '\r' | '\n')) {
            return Err(Error::InvalidHeaderValue);
        }

        Ok(Self { name, value })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn into_parts(self) -> (String, String) {
        (self.name, self.value)
    }
}

impl std::fmt::Debug for Header {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sensitive = [
            "cookie",
            "set-cookie",
            "authorization",
            "proxy-authorization",
        ]
        .iter()
        .any(|name| self.name.eq_ignore_ascii_case(name));
        let value = if sensitive { "<redacted>" } else { &self.value };

        f.debug_struct("Header")
            .field("name", &self.name)
            .field("value", &value)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    method: HttpMethod,
    url: String,
    headers: Vec<Header>,
    body: Option<String>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Result<Self, Error> {
        Ok(Self {
            method: HttpMethod::Get,
            url: normalize_url(url.into())?,
            headers: request_headers(false)?,
            body: None,
        })
    }

    pub fn json(url: impl Into<String>, body: impl Serialize) -> Result<Self, Error> {
        Ok(Self {
            method: HttpMethod::Post,
            url: normalize_url(url.into())?,
            headers: request_headers(true)?,
            body: Some(serde_json::to_string(&body)?),
        })
    }

    pub fn method(&self) -> HttpMethod {
        self.method
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn headers(&self) -> &[Header] {
        &self.headers
    }

    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }

    pub fn into_parts(self) -> (HttpMethod, String, Vec<Header>, Option<String>) {
        (self.method, self.url, self.headers, self.body)
    }

    pub fn with_cookie_header(mut self, value: impl Into<String>) -> Result<Self, Error> {
        let value = normalize_cookie_header(value)?;

        self.headers
            .retain(|header| !header.name.eq_ignore_ascii_case("cookie"));
        self.headers.push(Header::new("Cookie", value)?);
        Ok(self)
    }

    pub fn with_session(self, session: &Session) -> Result<Self, Error> {
        self.with_cookie_header(session.cookie_header())
    }

    pub fn with_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, Error> {
        self.headers.push(Header::new(name, value)?);
        Ok(self)
    }
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("body_bytes", &self.body.as_ref().map(String::len))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Session {
    cookie_header: String,
}

impl Session {
    pub fn from_cookie_header(value: impl Into<String>) -> Result<Self, Error> {
        Ok(Self {
            cookie_header: normalize_cookie_header(value)?,
        })
    }

    pub fn cookie_header(&self) -> &str {
        &self.cookie_header
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("cookie_header", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementRpc {
    base_url: String,
    locale: String,
    pub_locale: String,
}

impl ElementRpc {
    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self, Error> {
        Ok(Self {
            base_url: normalize_base_url(base_url.into())?,
            locale: DEFAULT_LOCALE.to_owned(),
            pub_locale: DEFAULT_PUB_LOCALE.to_owned(),
        })
    }

    pub fn with_locale(
        mut self,
        locale: impl Into<String>,
        pub_locale: impl Into<String>,
    ) -> Result<Self, Error> {
        let locale = locale.into();
        let pub_locale = pub_locale.into();

        require_argument("locale", &locale)?;
        require_argument("pubLocale", &pub_locale)?;
        self.locale = locale;
        self.pub_locale = pub_locale;
        Ok(self)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn locale(&self) -> &str {
        &self.locale
    }

    pub fn pub_locale(&self) -> &str {
        &self.pub_locale
    }

    pub fn call(
        &self,
        module_name: impl Into<String>,
        method_name: impl Into<String>,
    ) -> Result<ModuleCallBuilder<'_>, Error> {
        Ok(ModuleCallBuilder {
            rpc: self,
            call: ModuleCall::new(module_name, method_name)?,
        })
    }

    fn module_call_request(&self, call: ModuleCall) -> Result<HttpRequest, Error> {
        call.into_http_request_without_session(&self.base_url, &self.locale, &self.pub_locale)
    }
}

pub struct ModuleCallBuilder<'a> {
    rpc: &'a ElementRpc,
    call: ModuleCall,
}

impl ModuleCallBuilder<'_> {
    pub fn param(
        mut self,
        type_name: impl Into<String>,
        value: impl Serialize,
    ) -> Result<Self, Error> {
        self.call = self
            .call
            .with_parameter(Parameter::typed(type_name, value)?);
        Ok(self)
    }

    pub fn remote_caller(mut self, id: impl Into<String>) -> Result<Self, Error> {
        self.call = self
            .call
            .with_remote_caller_info(RemoteCallerInfo::disabled(id)?);
        Ok(self)
    }

    pub fn request(self) -> Result<HttpRequest, Error> {
        self.rpc.module_call_request(self.call)
    }

    pub fn build(self) -> ModuleCall {
        self.call
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModuleCall {
    #[serde(rename = "moduleName")]
    module_name: String,
    #[serde(rename = "methodName")]
    method_name: String,
    parameters: Vec<Parameter>,
    #[serde(rename = "remoteCallerInfo", skip_serializing_if = "Option::is_none")]
    remote_caller_info: Option<RemoteCallerInfo>,
}

impl ModuleCall {
    pub fn new(
        module_name: impl Into<String>,
        method_name: impl Into<String>,
    ) -> Result<Self, Error> {
        let module_name = module_name.into();
        let method_name = method_name.into();

        require_argument("moduleName", &module_name)?;
        require_argument("methodName", &method_name)?;

        Ok(Self {
            module_name,
            method_name,
            parameters: Vec::new(),
            remote_caller_info: None,
        })
    }

    pub fn module_name(&self) -> &str {
        &self.module_name
    }

    pub fn method_name(&self) -> &str {
        &self.method_name
    }

    pub fn parameters(&self) -> &[Parameter] {
        &self.parameters
    }

    pub fn with_parameter(mut self, parameter: Parameter) -> Self {
        self.parameters.push(parameter);
        self
    }

    pub fn with_remote_caller_info(mut self, remote_caller_info: RemoteCallerInfo) -> Self {
        self.remote_caller_info = Some(remote_caller_info);
        self
    }

    pub fn into_http_request_without_session(
        self,
        base_url: impl Into<String>,
        locale: &str,
        pub_locale: &str,
    ) -> Result<HttpRequest, Error> {
        json_request(
            &normalize_base_url(base_url.into())?,
            ui_path(MODULE_CALL_PATH, locale, pub_locale)?,
            self,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleCallResponse<T = serde_json::Value> {
    debug_exit_reason: Option<serde_json::Value>,
    result: T,
}

impl<'de, T> Deserialize<'de> for ModuleCallResponse<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Envelope<T> {
            #[serde(rename = "debugExitReason")]
            debug_exit_reason: serde_json::Value,
            result: T,
        }

        let envelope = Envelope::deserialize(deserializer)?;
        let completed = envelope.debug_exit_reason.is_null()
            || envelope.debug_exit_reason.as_str() == Some("NONE");
        Ok(Self {
            debug_exit_reason: (!completed).then_some(envelope.debug_exit_reason),
            result: envelope.result,
        })
    }
}

impl<T> ModuleCallResponse<T>
where
    T: DeserializeOwned,
{
    pub fn from_slice(body: impl AsRef<[u8]>) -> Result<Self, Error> {
        Ok(serde_json::from_slice(body.as_ref())?)
    }
}

impl<T> ModuleCallResponse<T> {
    pub fn debug_exit_reason(&self) -> Option<&serde_json::Value> {
        self.debug_exit_reason.as_ref()
    }

    pub fn result(&self) -> Result<&T, Error> {
        self.ensure_completed()?;
        Ok(&self.result)
    }

    pub fn into_result(self) -> Result<T, Error> {
        self.ensure_completed()?;
        Ok(self.result)
    }

    fn ensure_completed(&self) -> Result<(), Error> {
        if self.debug_exit_reason.is_none() {
            Ok(())
        } else {
            Err(Error::UnexpectedResponse(
                "module call reported a non-success debugExitReason",
            ))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DynamicListResponse {
    rows: Vec<DynamicListRow>,
}

impl DynamicListResponse {
    pub fn from_slice(body: impl AsRef<[u8]>) -> Result<Self, Error> {
        Ok(serde_json::from_slice(body.as_ref())?)
    }

    pub fn rows(&self) -> &[DynamicListRow] {
        &self.rows
    }

    pub fn into_rows(self) -> Vec<DynamicListRow> {
        self.rows
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DynamicListRow {
    #[serde(rename = "fieldValues")]
    field_values: Vec<serde_json::Value>,
}

impl DynamicListRow {
    pub fn field_values(&self) -> &[serde_json::Value] {
        &self.field_values
    }

    pub fn into_field_values(self) -> Vec<serde_json::Value> {
        self.field_values
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RemoteCallerInfo {
    id: String,
    #[serde(rename = "debugState")]
    debug_state: String,
    #[serde(rename = "bslStack")]
    bsl_stack: Vec<String>,
}

impl RemoteCallerInfo {
    pub fn disabled(id: impl Into<String>) -> Result<Self, Error> {
        let id = id.into();
        require_argument("remote caller id", &id)?;

        Ok(Self {
            id,
            debug_state: "DISABLED".to_owned(),
            bsl_stack: Vec::new(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Parameter {
    #[serde(rename = "type")]
    type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<serde_json::Value>,
}

impl Parameter {
    pub fn typed(type_name: impl Into<String>, value: impl Serialize) -> Result<Self, Error> {
        let type_name = type_name.into();
        require_argument("parameter type", &type_name)?;

        Ok(Self {
            type_name,
            value: Some(serde_json::to_value(value)?),
        })
    }

    pub fn undefined() -> Self {
        Self {
            type_name: "Std::Undefined".to_owned(),
            value: None,
        }
    }

    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    pub fn value(&self) -> Option<&serde_json::Value> {
        self.value.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reference {
    #[serde(rename = "type")]
    type_name: String,
    value: String,
}

impl Reference {
    pub fn new(type_name: impl Into<String>, value: impl Into<String>) -> Result<Self, Error> {
        let type_name = type_name.into();
        let value = value.into();
        require_argument("reference type", &type_name)?;
        if value.trim().is_empty() {
            return Err(Error::EmptyReferenceValue);
        }

        Ok(Self { type_name, value })
    }

    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EntityReadRequest {
    reference: Reference,
}

impl EntityReadRequest {
    pub fn new(reference: Reference) -> Self {
        Self { reference }
    }

    pub fn reference(&self) -> &Reference {
        &self.reference
    }

    pub fn into_http_request_without_session(
        self,
        base_url: impl Into<String>,
        locale: &str,
        pub_locale: &str,
    ) -> Result<HttpRequest, Error> {
        json_request(
            &normalize_base_url(base_url.into())?,
            ui_path(ENTITY_READ_PATH, locale, pub_locale)?,
            self,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PresentationLookup {
    #[serde(rename = "typeName")]
    type_name: String,
    ids: Vec<String>,
}

impl PresentationLookup {
    pub fn new(type_name: impl Into<String>, ids: Vec<String>) -> Result<Self, Error> {
        let type_name = type_name.into();
        require_argument("presentation type", &type_name)?;

        if ids.iter().any(|id| id.trim().is_empty()) {
            return Err(Error::EmptyReferenceValue);
        }

        Ok(Self { type_name, ids })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PresentationRequest(Vec<PresentationLookup>);

impl PresentationRequest {
    pub fn new(lookups: Vec<PresentationLookup>) -> Self {
        Self(lookups)
    }

    pub fn into_http_request_without_session(
        self,
        base_url: impl Into<String>,
        locale: &str,
        pub_locale: &str,
    ) -> Result<HttpRequest, Error> {
        json_request(
            &normalize_base_url(base_url.into())?,
            ui_path(PRESENTATION_PATH, locale, pub_locale)?,
            self,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DynamicListRequest {
    #[serde(rename = "dynamicList")]
    dynamic_list: serde_json::Value,
    #[serde(rename = "cursorDirection")]
    cursor_direction: i32,
    #[serde(rename = "keyRowPosition")]
    key_row_position: i32,
    #[serde(rename = "showDeleted")]
    show_deleted: bool,
    limit: u32,
    #[serde(rename = "useHierarchy")]
    use_hierarchy: bool,
}

impl DynamicListRequest {
    pub fn new(dynamic_list: serde_json::Value, limit: u32) -> Self {
        Self {
            dynamic_list,
            cursor_direction: 0,
            key_row_position: 0,
            show_deleted: true,
            limit,
            use_hierarchy: false,
        }
    }

    pub fn into_http_request_without_session(
        self,
        base_url: impl Into<String>,
        locale: &str,
        pub_locale: &str,
    ) -> Result<HttpRequest, Error> {
        json_request(
            &normalize_base_url(base_url.into())?,
            ui_path(DYNAMIC_LIST_PATH, locale, pub_locale)?,
            self,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DynamicListInfoRequest {
    #[serde(rename = "dynamicList")]
    dynamic_list: serde_json::Value,
}

impl DynamicListInfoRequest {
    pub fn new(dynamic_list: serde_json::Value) -> Self {
        Self { dynamic_list }
    }

    pub fn into_http_request_without_session(
        self,
        base_url: impl Into<String>,
        locale: &str,
        pub_locale: &str,
    ) -> Result<HttpRequest, Error> {
        json_request(
            &normalize_base_url(base_url.into())?,
            ui_path(DYNAMIC_LIST_INFO_PATH, locale, pub_locale)?,
            self,
        )
    }
}

fn json_request(
    base_url: &str,
    path_and_query: impl AsRef<str>,
    body: impl Serialize,
) -> Result<HttpRequest, Error> {
    HttpRequest::json(format!("{}{}", base_url, path_and_query.as_ref()), body)
}

fn request_headers(json: bool) -> Result<Vec<Header>, Error> {
    let mut headers = vec![Header::new("Accept", "application/json, text/plain, */*")?];
    if json {
        headers.push(Header::new("Content-Type", "application/json")?);
    }
    Ok(headers)
}

fn normalize_url(value: String) -> Result<String, Error> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(Error::EmptyUrl)
    } else {
        Ok(value)
    }
}

fn normalize_base_url(value: String) -> Result<String, Error> {
    let value = value.trim().trim_end_matches('/').to_owned();
    if value.is_empty() {
        Err(Error::EmptyBaseUrl)
    } else {
        Ok(value)
    }
}

fn normalize_cookie_header(value: impl Into<String>) -> Result<String, Error> {
    let value = value.into();
    let value = value.trim();
    let value = if value
        .get(.."Cookie:".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Cookie:"))
    {
        &value["Cookie:".len()..]
    } else {
        value
    }
    .trim();

    if value.is_empty() {
        return Err(Error::EmptyCookieHeader);
    }
    if value.chars().any(|ch| matches!(ch, '\r' | '\n')) {
        return Err(Error::InvalidCookieHeader);
    }
    Ok(value.to_owned())
}

fn require_argument<'a>(name: &'static str, value: &'a str) -> Result<&'a str, Error> {
    if value.trim().is_empty() {
        Err(Error::EmptyArgument(name))
    } else {
        Ok(value)
    }
}

fn ui_path(path: &str, locale: &str, pub_locale: &str) -> Result<String, Error> {
    Ok(format!(
        "{path}?locale={}&pubLocale={}",
        encode_component(require_argument("locale", locale)?),
        encode_component(require_argument("pubLocale", pub_locale)?),
    ))
}

pub(crate) fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[derive(Debug)]
pub enum Error {
    EmptyArgument(&'static str),
    EmptyBaseUrl,
    EmptyCookieHeader,
    EmptyHeaderName,
    EmptyReferenceValue,
    EmptyUrl,
    InvalidCookieHeader,
    InvalidHeaderName,
    InvalidHeaderValue,
    UnexpectedResponse(&'static str),
    ZeroLimit,
    Json(serde_json::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyArgument(name) => write!(f, "{name} must not be empty"),
            Self::EmptyBaseUrl => f.write_str("base url must not be empty"),
            Self::EmptyCookieHeader => f.write_str("cookie header must not be empty"),
            Self::EmptyHeaderName => f.write_str("header name must not be empty"),
            Self::EmptyReferenceValue => f.write_str("reference value must not be empty"),
            Self::EmptyUrl => f.write_str("url must not be empty"),
            Self::InvalidCookieHeader => f.write_str("cookie header must be a single header value"),
            Self::InvalidHeaderName => f.write_str("header name must not contain CR or LF"),
            Self::InvalidHeaderValue => f.write_str("header value must not contain CR or LF"),
            Self::UnexpectedResponse(message) => {
                write!(f, "unexpected Element response: {message}")
            }
            Self::ZeroLimit => f.write_str("dynamic list limit must be greater than zero"),
            Self::Json(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn facade_builds_sessionless_descriptor_without_browser_origin_headers() {
        let request = ElementRpc::with_base_url("https://example.invalid/app/")
            .unwrap()
            .with_locale("en-US", "en_US")
            .unwrap()
            .call("module", "method")
            .unwrap()
            .param("Std::String", "value")
            .unwrap()
            .request()
            .unwrap();

        assert_eq!(
            request.url(),
            "https://example.invalid/app/ui/module/call?locale=en-US&pubLocale=en_US"
        );
        assert!(
            request
                .headers()
                .iter()
                .all(|header| !matches!(header.name(), "Origin" | "Referer" | "Cookie"))
        );
    }

    #[test]
    fn session_is_attached_explicitly_and_redacted() {
        let request = HttpRequest::get("https://example.invalid/status")
            .unwrap()
            .with_session(&Session::from_cookie_header("Cookie: session=value").unwrap())
            .unwrap();

        assert_eq!(
            request
                .headers()
                .iter()
                .find(|header| header.name().eq_ignore_ascii_case("cookie"))
                .map(Header::value),
            Some("session=value")
        );
        assert!(!format!("{request:?}").contains("session=value"));
    }

    #[test]
    fn headers_reject_line_breaks() {
        assert!(matches!(
            Header::new("X-Test\r\nInjected", "value"),
            Err(Error::InvalidHeaderName)
        ));
        assert!(matches!(
            Header::new("X-Test", "value\r\nInjected: true"),
            Err(Error::InvalidHeaderValue)
        ));
    }

    #[test]
    fn sensitive_headers_are_redacted() {
        let header = Header::new("Authorization", "Bearer secret").unwrap();

        assert!(!format!("{header:?}").contains("secret"));
    }

    #[test]
    fn module_call_response_requires_stable_envelope() {
        let response =
            ModuleCallResponse::<bool>::from_slice(br#"{"debugExitReason":null,"result":true}"#)
                .unwrap();
        assert_eq!(response.debug_exit_reason(), None);
        assert_eq!(response.result().unwrap(), &true);

        let response =
            ModuleCallResponse::<bool>::from_slice(br#"{"debugExitReason":"NONE","result":true}"#)
                .unwrap();
        assert_eq!(response.debug_exit_reason(), None);
        assert!(response.into_result().unwrap());

        assert!(ModuleCallResponse::<bool>::from_slice(br#"{"result":true}"#).is_err());
        assert!(ModuleCallResponse::<bool>::from_slice(br#"{"debugExitReason":null}"#).is_err());
    }

    #[test]
    fn module_call_response_rejects_non_success_exit_reasons() {
        for body in [
            br#"{"debugExitReason":"ERROR","result":true}"#.as_slice(),
            br#"{"debugExitReason":{"kind":"failure"},"result":true}"#.as_slice(),
        ] {
            let response = ModuleCallResponse::<bool>::from_slice(body).unwrap();

            assert!(response.result().is_err());
            assert!(response.into_result().is_err());
        }
    }

    #[test]
    fn dynamic_list_response_requires_rows_and_field_values() {
        let response = DynamicListResponse::from_slice(
            br#"{"rows":[{"fieldValues":[{"type":"Std::String","value":"x"}]}]}"#,
        )
        .unwrap();
        assert_eq!(response.rows().len(), 1);
        assert_eq!(response.rows()[0].field_values()[0]["value"], "x");

        assert!(DynamicListResponse::from_slice(br#"{}"#).is_err());
        assert!(DynamicListResponse::from_slice(br#"{"rows":{}}"#).is_err());
        assert!(DynamicListResponse::from_slice(br#"{"rows":[{}]}"#).is_err());
        assert!(DynamicListResponse::from_slice(br#"{"rows":[{"fieldValues":{}}]}"#).is_err());
    }

    #[test]
    fn serializes_wire_requests() {
        let call = ModuleCall::new("module", "method")
            .unwrap()
            .with_parameter(Parameter::undefined());
        assert_eq!(
            serde_json::to_value(call).unwrap(),
            json!({
                "moduleName": "module",
                "methodName": "method",
                "parameters": [{"type": "Std::Undefined"}]
            })
        );

        let dynamic_list = DynamicListRequest::new(json!({"MainTable": "Example"}), 4);
        assert_eq!(
            serde_json::to_value(dynamic_list).unwrap()["limit"],
            json!(4)
        );
    }

    #[test]
    fn request_parts_are_available_to_external_transports() {
        let request =
            HttpRequest::json("https://example.invalid/call", json!({"ok": true})).unwrap();
        let (method, url, headers, body) = request.into_parts();

        assert_eq!(method, HttpMethod::Post);
        assert_eq!(url, "https://example.invalid/call");
        assert_eq!(headers.len(), 2);
        assert_eq!(body.as_deref(), Some(r#"{"ok":true}"#));
    }
}
