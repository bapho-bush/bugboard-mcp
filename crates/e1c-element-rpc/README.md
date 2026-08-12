# e1c-element-rpc

Wire-level request descriptors and response envelopes for 1C Element RPC.

The crate does not execute HTTP. Callers attach a `Session` to an `HttpRequest`
and pass `method`, `url`, `headers`, and `body` to their HTTP client.

## Generic module call

```rust
use e1c_element_rpc::{ElementRpc, Session};

let rpc = ElementRpc::with_base_url("https://example.invalid/app")?
    .with_locale("en-US", "en_US")?;
let session = Session::from_cookie_header("session=value")?;
let request = rpc
    .call("e1c::example::Module", "Method")?
    .param("Std::String", "value")?
    .request()?
    .with_session(&session)?;

assert_eq!(request.method(), e1c_element_rpc::HttpMethod::Post);
# Ok::<(), e1c_element_rpc::Error>(())
```

`ElementRpc` builds sessionless descriptors. Generic descriptors add only
protocol headers such as `Accept` and `Content-Type`; deployment-specific
`Origin`, `Referer`, authentication, transport, timeout, and retry remain with
the caller or a concrete adapter.

Stable response envelopes are strict and fail when required fields are absent:

```rust
use e1c_element_rpc::{DynamicListResponse, ModuleCallResponse};

let call = ModuleCallResponse::<bool>::from_slice(
    br#"{"debugExitReason":null,"result":true}"#,
)?;
assert!(*call.result()?);

let list = DynamicListResponse::from_slice(
    br#"{"rows":[{"fieldValues":[]}]}"#,
)?;
assert_eq!(list.rows().len(), 1);
# Ok::<(), e1c_element_rpc::Error>(())
```

Module-call results are accessible only when `debugExitReason` is `null` or
`"NONE"`; other values fail closed as an unexpected response.

## Bugboard adapter

`e1c_element_rpc::bugboard` owns the captured bugboard wire contract:

- Russian module and method names;
- exact parameter signatures for subscribe, unsubscribe, vote, and unvote;
- project, version, and bug dynamic-list specifications;
- matching `ProjectRow`, `VersionRow`, `BugRow`, and `BugDetails` decoders;
- known bugboard `Origin` and `Referer` headers.

Examples:

```rust
use e1c_element_rpc::{DynamicListResponse, bugboard};

let request = bugboard::bug_list_request(10)?;
let response = DynamicListResponse::from_slice(
    br#"{"rows":[]}"#,
)?;
let bugs = bugboard::decode_bug_rows(&response)?;

let subscribe = bugboard::bug_subscribe_request("bug-reference")?;
let unsubscribe = bugboard::bug_unsubscribe_request("bug-reference")?;
# let _ = (request, bugs, subscribe, unsubscribe);
# Ok::<(), e1c_element_rpc::Error>(())
```

Raw references remain in adapter rows. A consumer such as the MCP server is
responsible for replacing them with its own handles and applying output
redaction.

## Live verification

The dev-only live example supplies `reqwest`; it is not a library dependency.
Run it through the workspace task with the cookie outside the repository. The
example derives `X-G5-Version` from the authenticated bugboard shell.

```powershell
$env:BUGBOARD_COOKIE="-"
Get-Clipboard | mise run live:bugboard-cookie-check
Remove-Item Env:\BUGBOARD_COOKIE
```

The probe rejects malformed successful responses. Its only opt-in mutation is
`BUGBOARD_MUTATION=project_subscription_roundtrip`; it attempts restoration even
after an intermediate failure and fails unless the final state matches the
initial state.

Export a project reference, inspect the dry-run, then execute the roundtrip:

```powershell
$env:BUGBOARD_COOKIE="-"
$env:BUGBOARD_EXPORT_REFS="1"
Get-Clipboard | mise run live:bugboard-cookie-check
$env:BUGBOARD_PROJECT_REF="<projectRef from reference_export>"
Remove-Item Env:\BUGBOARD_EXPORT_REFS
$env:BUGBOARD_MUTATION="project_subscription_roundtrip"
$env:BUGBOARD_MUTATION_DRY_RUN="1"
mise run live:bugboard-cookie-check
Remove-Item Env:\BUGBOARD_MUTATION_DRY_RUN
Get-Clipboard | mise run live:bugboard-cookie-check
Remove-Item Env:\BUGBOARD_COOKIE, Env:\BUGBOARD_EXPORT_REFS, Env:\BUGBOARD_PROJECT_REF, Env:\BUGBOARD_MUTATION -ErrorAction SilentlyContinue
```

Reference export is opt-in because it prints raw references; do not persist its
output.
