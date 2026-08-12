use std::{
    future::Future,
    sync::{Arc, Mutex},
};

use e1c_element_rpc::bugboard;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::task::JoinSet;

use crate::{
    client::BugboardClient,
    config::SessionConfig,
    errors::ToolFailure,
    handles::{HandleKind, HandleStore},
    normalize::*,
    write_safety::{RequestedState, apply_write},
};
const DEFAULT_LIST_LIMIT: u32 = 10;
const MAX_LIST_LIMIT: u32 = 50;
// ponytail: fixed window; make it configurable only after live throttling evidence.
const MAX_CONCURRENT_BUG_READS: usize = 8;

#[derive(Clone)]
pub(crate) struct BugboardServer {
    pub(crate) tool_router: ToolRouter<Self>,
    client: Result<Arc<BugboardClient>, ToolFailure>,
    handles: Arc<Mutex<HandleStore>>,
}

impl BugboardServer {
    pub(crate) fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            client: SessionConfig::from_env()
                .map_err(ToolFailure::from)
                .and_then(BugboardClient::new)
                .map(Arc::new),
            handles: Arc::new(Mutex::new(HandleStore::default())),
        }
    }

    fn client(&self) -> Result<Arc<BugboardClient>, ToolFailure> {
        self.client.clone()
    }

    pub(crate) fn remember_ref(
        &self,
        kind: HandleKind,
        reference: &str,
    ) -> Result<String, ToolFailure> {
        let mut handles = self
            .handles
            .lock()
            .map_err(|_| ToolFailure::internal("handle store is unavailable"))?;
        Ok(handles.remember(kind, reference))
    }

    fn resolve_ref(&self, kind: HandleKind, handle: &str) -> Result<String, ToolFailure> {
        let handles = self
            .handles
            .lock()
            .map_err(|_| ToolFailure::internal("handle store is unavailable"))?;
        handles.resolve(kind, handle).ok_or_else(|| {
            ToolFailure::invalid_reference(
                "Unknown or mismatched handle. Fetch it again in this server session.",
            )
        })
    }
}

#[tool_router(router = tool_router)]
impl BugboardServer {
    #[tool(
        name = "bugboard_auth_status",
        description = "Check whether the configured 1C bugboard session is authenticated."
    )]
    async fn bugboard_auth_status(&self) -> CallToolResult {
        tool_result(self.auth_status_value()).await
    }

    #[tool(
        name = "project_list",
        description = "List visible 1C bugboard projects as human-readable summaries."
    )]
    async fn project_list(&self, Parameters(params): Parameters<ListParams>) -> CallToolResult {
        tool_result(self.project_list_value(params.limit)).await
    }

    #[tool(
        name = "project_get_versions",
        description = "List visible versions for a project returned by project_list."
    )]
    async fn project_get_versions(
        &self,
        Parameters(params): Parameters<ProjectVersionsParams>,
    ) -> CallToolResult {
        tool_result(self.project_get_versions_value(params)).await
    }

    #[tool(
        name = "version_get_bugs",
        description = "List bugs for a unique exact version title returned by project_get_versions."
    )]
    async fn version_get_bugs(
        &self,
        Parameters(params): Parameters<VersionGetBugsParams>,
    ) -> CallToolResult {
        tool_result(self.version_get_bugs_value(params)).await
    }

    #[tool(
        name = "project_list_subscribed",
        description = "Return the configured user's subscribed projects as safe bugboard data."
    )]
    async fn project_list_subscribed(&self) -> CallToolResult {
        tool_result(self.project_list_subscribed_value()).await
    }

    #[tool(
        name = "project_subscribe",
        description = "Idempotently subscribe to a project and verify the resulting state.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn project_subscribe(
        &self,
        Parameters(params): Parameters<ProjectWriteParams>,
    ) -> CallToolResult {
        tool_result(self.set_project_subscription_value(params, RequestedState::Enabled)).await
    }

    #[tool(
        name = "project_unsubscribe",
        description = "Idempotently unsubscribe from a project and verify the resulting state.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn project_unsubscribe(
        &self,
        Parameters(params): Parameters<ProjectWriteParams>,
    ) -> CallToolResult {
        tool_result(self.set_project_subscription_value(params, RequestedState::Disabled)).await
    }

    #[tool(
        name = "bug_list_recent",
        description = "List recently visible bugboard bugs using first-page dynamic-list summaries."
    )]
    async fn bug_list_recent(&self, Parameters(params): Parameters<ListParams>) -> CallToolResult {
        tool_result(self.bug_list_recent_value(params.limit)).await
    }

    #[tool(
        name = "bug_search",
        description = "Search bugboard bugs by exact bug number or full-text query."
    )]
    async fn bug_search(&self, Parameters(params): Parameters<BugSearchParams>) -> CallToolResult {
        tool_result(self.bug_search_value(params)).await
    }

    #[tool(
        name = "bug_list_subscribed",
        description = "List bugs subscribed by the configured bugboard user."
    )]
    async fn bug_list_subscribed(
        &self,
        Parameters(params): Parameters<ListParams>,
    ) -> CallToolResult {
        tool_result(self.bug_list_subscribed_value(params)).await
    }

    #[tool(
        name = "bug_subscribe",
        description = "Idempotently subscribe to a bug and verify the resulting state.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn bug_subscribe(
        &self,
        Parameters(params): Parameters<BugWriteParams>,
    ) -> CallToolResult {
        tool_result(self.set_bug_subscription_value(params, RequestedState::Enabled)).await
    }

    #[tool(
        name = "bug_unsubscribe",
        description = "Idempotently unsubscribe from a bug and verify the resulting state.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn bug_unsubscribe(
        &self,
        Parameters(params): Parameters<BugWriteParams>,
    ) -> CallToolResult {
        tool_result(self.set_bug_subscription_value(params, RequestedState::Disabled)).await
    }

    #[tool(
        name = "bug_list_voted",
        description = "List bugs voted by the configured bugboard user."
    )]
    async fn bug_list_voted(
        &self,
        Parameters(params): Parameters<VotedBugListParams>,
    ) -> CallToolResult {
        tool_result(self.bug_list_voted_value(params)).await
    }

    #[tool(
        name = "bug_vote",
        description = "Idempotently add one kind of bug vote and verify the resulting state.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn bug_vote(&self, Parameters(params): Parameters<BugVoteWriteParams>) -> CallToolResult {
        tool_result(self.set_bug_vote_value(params, RequestedState::Enabled)).await
    }

    #[tool(
        name = "bug_unvote",
        description = "Idempotently remove one kind of bug vote and verify the resulting state.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn bug_unvote(
        &self,
        Parameters(params): Parameters<BugVoteWriteParams>,
    ) -> CallToolResult {
        tool_result(self.set_bug_vote_value(params, RequestedState::Disabled)).await
    }

    #[tool(
        name = "bug_get",
        description = "Get human-readable details for a bug returned by a list or search tool."
    )]
    async fn bug_get(&self, Parameters(params): Parameters<BugGetParams>) -> CallToolResult {
        tool_result(self.bug_get_value(params)).await
    }

    #[tool(
        name = "bug_get_history",
        description = "Get reference-redacted upstream history entries embedded in a bug card."
    )]
    async fn bug_get_history(
        &self,
        Parameters(params): Parameters<BugGetParams>,
    ) -> CallToolResult {
        tool_result(self.bug_get_history_value(params)).await
    }

    #[tool(
        name = "bug_get_subscription_vote_state",
        description = "Get the current subscription and vote state for a bug."
    )]
    async fn bug_get_subscription_vote_state(
        &self,
        Parameters(params): Parameters<BugGetParams>,
    ) -> CallToolResult {
        tool_result(self.bug_get_subscription_vote_state_value(params)).await
    }

    #[tool(
        name = "bug_open_in_browser",
        description = "Return the browser URL for a bug."
    )]
    async fn bug_open_in_browser(
        &self,
        Parameters(params): Parameters<BugGetParams>,
    ) -> CallToolResult {
        tool_result(self.bug_open_in_browser_value(params)).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for BugboardServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::LATEST)
            .with_server_info(
                Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                    .with_title("1C Bugboard MCP Server"),
            )
            .with_instructions(
                "1C bugboard tools with verified idempotent writes. Session material is read from BUGBOARD_SESSION_ENV.",
            )
    }
}

impl BugboardServer {
    async fn auth_status_value(&self) -> Result<Value, ToolFailure> {
        let client = self.client()?;
        let parsed = client
            .execute_unversioned("bugboard_auth_status", bugboard::auth_status_request()?)
            .await?;
        let authenticated = parsed
            .get("isAuthenticated")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                ToolFailure::bugboard_changed(
                    "bugboard_auth_status",
                    "missing boolean isAuthenticated",
                )
            })?;

        Ok(json!({
            "authenticated": authenticated,
            "authentication_method": parsed
                .get("authenticationMethod")
                .and_then(Value::as_str),
        }))
    }

    async fn project_list_value(&self, limit: Option<u32>) -> Result<Value, ToolFailure> {
        let limit = normalize_limit(limit)?;
        let client = self.client()?;
        let parsed = client
            .execute_dynamic_list("project_list", bugboard::project_list_request(limit)?)
            .await?;
        let projects = bugboard::decode_project_rows(&parsed)
            .map_err(|error| ToolFailure::bugboard_changed("project_list", error))?
            .iter()
            .map(|row| normalize_project_row(self, row))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(json!({
            "projects": projects,
            "count": projects.len(),
            "limit": limit,
        }))
    }

    async fn project_get_versions_value(
        &self,
        params: ProjectVersionsParams,
    ) -> Result<Value, ToolFailure> {
        let limit = normalize_limit(params.limit)?;
        let project_ref = self.resolve_project_ref(&params.project_handle)?;
        let client = self.client()?;
        let parsed = client
            .execute_dynamic_list(
                "project_get_versions",
                bugboard::project_versions_request(&project_ref, limit)?,
            )
            .await?;
        let versions = bugboard::decode_version_rows(&parsed)
            .map_err(|error| ToolFailure::bugboard_changed("project_get_versions", error))?
            .iter()
            .map(normalize_version_row)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(json!({
            "project_handle": params.project_handle,
            "versions": versions,
            "count": versions.len(),
            "limit": limit,
        }))
    }

    async fn project_list_subscribed_value(&self) -> Result<Value, ToolFailure> {
        let client = self.client()?;
        let projects = Self::project_subscription_references(&client)
            .await?
            .into_iter()
            .map(|reference| {
                Ok(json!({
                    "project_handle": self.remember_ref(HandleKind::Project, &reference)?,
                }))
            })
            .collect::<Result<Vec<_>, ToolFailure>>()?;

        Ok(json!({
            "projects": projects,
            "count": projects.len(),
        }))
    }

    async fn bug_list_recent_value(&self, limit: Option<u32>) -> Result<Value, ToolFailure> {
        let limit = normalize_limit(limit)?;
        let client = self.client()?;
        let parsed = client
            .execute_dynamic_list("bug_list_recent", bugboard::bug_list_request(limit)?)
            .await?;
        let bugs = bugboard::decode_bug_rows(&parsed)
            .map_err(|error| ToolFailure::bugboard_changed("bug_list_recent", error))?
            .iter()
            .map(|row| normalize_bug_row(self, row))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(json!({
            "bugs": bugs,
            "count": bugs.len(),
            "limit": limit,
        }))
    }

    pub(crate) async fn version_get_bugs_value(
        &self,
        params: VersionGetBugsParams,
    ) -> Result<Value, ToolFailure> {
        let limit = normalize_limit(params.limit)?;
        let version_title = params.version_title.trim();
        if version_title.is_empty() {
            return Err(ToolFailure::invalid_arguments(
                "Set version_title from project_get_versions.",
            ));
        }
        let project_ref = self.resolve_project_ref(&params.project_handle)?;
        let client = self.client()?;
        let parsed = client
            .execute_dynamic_list(
                "version_get_bugs",
                bugboard::version_lookup_request(&project_ref, version_title)?,
            )
            .await?;
        let versions = bugboard::decode_version_rows(&parsed)
            .map_err(|error| ToolFailure::bugboard_changed("version_get_bugs", error))?;
        let version_ref = Self::resolve_version_ref(&versions, &project_ref, version_title)?;
        let result = client
            .execute_module_call(
                "version_get_bugs",
                bugboard::version_bugs_request(&project_ref, &version_ref)?,
            )
            .await?;
        let refs = bugboard::decode_version_bug_references(&result)
            .map_err(|error| ToolFailure::bugboard_changed("version_get_bugs", error))?;

        self.bug_references_response(
            client,
            refs,
            limit,
            json!({
                "project_handle": params.project_handle,
                "version_title": version_title,
            }),
        )
        .await
    }

    async fn set_project_subscription_value(
        &self,
        params: ProjectWriteParams,
        requested: RequestedState,
    ) -> Result<Value, ToolFailure> {
        let project_ref = self.resolve_ref(HandleKind::Project, &params.project_handle)?;
        let client = self.client()?;
        let outcome = apply_write(
            requested,
            || {
                let client = Arc::clone(&client);
                let project_ref = project_ref.clone();
                async move { Self::project_subscription_state(&client, &project_ref).await }
            },
            |requested| {
                let client = Arc::clone(&client);
                let project_ref = project_ref.clone();
                async move {
                    let (operation, request) = match requested {
                        RequestedState::Enabled => (
                            "project_subscribe",
                            bugboard::project_subscribe_request(&project_ref),
                        ),
                        RequestedState::Disabled => (
                            "project_unsubscribe",
                            bugboard::project_unsubscribe_request(&project_ref),
                        ),
                    };
                    client.execute_module_call_bool(operation, request?).await?;
                    Ok(())
                }
            },
        )
        .await?;

        Ok(json!(outcome))
    }

    async fn project_subscription_state(
        client: &BugboardClient,
        project_ref: &str,
    ) -> Result<bool, ToolFailure> {
        Ok(Self::project_subscription_references(client)
            .await?
            .iter()
            .any(|reference| reference == project_ref))
    }

    async fn project_subscription_references(
        client: &BugboardClient,
    ) -> Result<Vec<String>, ToolFailure> {
        let result = client
            .execute_module_call(
                "project_list_subscribed",
                bugboard::project_subscriptions_request()?,
            )
            .await?;
        bugboard::decode_project_references(&result)
            .map_err(|error| ToolFailure::bugboard_changed("project_list_subscribed", error))
    }

    async fn bug_search_value(&self, params: BugSearchParams) -> Result<Value, ToolFailure> {
        let query = params.query.trim();
        if query.is_empty() {
            return Err(ToolFailure::new(
                "invalid_arguments",
                "Set a non-empty query.",
                json!({"field": "query"}),
            ));
        }
        let limit = normalize_limit(params.limit)?;
        let client = self.client()?;
        if is_bug_number(query) {
            let refs = bugboard::decode_bug_rows(
                &client
                    .execute_dynamic_list("bug_search", bugboard::bug_lookup_request(query)?)
                    .await?,
            )
            .map_err(|error| ToolFailure::bugboard_changed("bug_search", error))?
            .into_iter()
            .filter(|row| row.number.as_deref() == Some(query))
            .map(|row| row.reference)
            .collect();

            return self
                .bug_references_response(
                    client,
                    refs,
                    limit,
                    json!({"query": query, "mode": "bug_lookup", "full_text": false}),
                )
                .await;
        }

        let result = client
            .execute_module_call("bug_search", bugboard::bug_full_text_search_request(query)?)
            .await?;
        let refs = bugboard::decode_full_text_bug_references(&result)
            .map_err(|error| ToolFailure::bugboard_changed("bug_search", error))?;
        self.bug_references_response(
            client,
            refs,
            limit,
            json!({"query": query, "mode": "full_text", "full_text": true}),
        )
        .await
    }

    async fn bug_list_subscribed_value(&self, params: ListParams) -> Result<Value, ToolFailure> {
        let limit = normalize_limit(params.limit)?;
        let client = self.client()?;
        let refs = Self::bug_subscription_references(&client).await?;
        self.bug_references_response(client, refs, limit, json!({"subscribed": true}))
            .await
    }

    async fn bug_list_voted_value(&self, params: VotedBugListParams) -> Result<Value, ToolFailure> {
        let limit = normalize_limit(params.limit)?;
        let client = self.client()?;
        let vote_kind = params.vote_kind;
        let refs = Self::bug_vote_references(&client, vote_kind).await?;
        self.bug_references_response(
            client,
            refs,
            limit,
            json!({"vote_kind": vote_kind.name(), "vote_code": vote_kind.code()}),
        )
        .await
    }

    async fn set_bug_subscription_value(
        &self,
        params: BugWriteParams,
        requested: RequestedState,
    ) -> Result<Value, ToolFailure> {
        let bug_ref = self.resolve_ref(HandleKind::Bug, &params.bug_handle)?;
        let client = self.client()?;
        let outcome = apply_write(
            requested,
            || {
                let client = Arc::clone(&client);
                let bug_ref = bug_ref.clone();
                async move { Self::bug_subscription_state(&client, &bug_ref).await }
            },
            |requested| {
                let client = Arc::clone(&client);
                let bug_ref = bug_ref.clone();
                async move {
                    let (operation, request) = match requested {
                        RequestedState::Enabled => {
                            ("bug_subscribe", bugboard::bug_subscribe_request(&bug_ref))
                        }
                        RequestedState::Disabled => (
                            "bug_unsubscribe",
                            bugboard::bug_unsubscribe_request(&bug_ref),
                        ),
                    };
                    client.execute_module_call_bool(operation, request?).await?;
                    Ok(())
                }
            },
        )
        .await?;

        Ok(json!(outcome))
    }

    async fn set_bug_vote_value(
        &self,
        params: BugVoteWriteParams,
        requested: RequestedState,
    ) -> Result<Value, ToolFailure> {
        let bug_ref = self.resolve_ref(HandleKind::Bug, &params.bug_handle)?;
        let vote_kind = params.vote_kind;
        let client = self.client()?;
        let outcome = apply_write(
            requested,
            || {
                let client = Arc::clone(&client);
                let bug_ref = bug_ref.clone();
                async move { Self::bug_vote_state(&client, &bug_ref, vote_kind).await }
            },
            |requested| {
                let client = Arc::clone(&client);
                let bug_ref = bug_ref.clone();
                async move {
                    let (operation, request) = match requested {
                        RequestedState::Enabled => (
                            "bug_vote",
                            bugboard::bug_vote_request(&bug_ref, vote_kind.into()),
                        ),
                        RequestedState::Disabled => (
                            "bug_unvote",
                            bugboard::bug_unvote_request(&bug_ref, vote_kind.into()),
                        ),
                    };
                    client.execute_module_call_bool(operation, request?).await?;
                    Ok(())
                }
            },
        )
        .await?;

        Ok(json!(outcome))
    }

    async fn bug_subscription_state(
        client: &BugboardClient,
        bug_ref: &str,
    ) -> Result<bool, ToolFailure> {
        Ok(Self::bug_subscription_references(client)
            .await?
            .iter()
            .any(|reference| reference == bug_ref))
    }

    async fn bug_subscription_references(
        client: &BugboardClient,
    ) -> Result<Vec<String>, ToolFailure> {
        let result = client
            .execute_module_call(
                "bug_list_subscribed",
                bugboard::subscribed_bug_list_request()?,
            )
            .await?;
        bugboard::decode_bug_references(&result)
            .map_err(|error| ToolFailure::bugboard_changed("bug_list_subscribed", error))
    }

    async fn bug_vote_state(
        client: &BugboardClient,
        bug_ref: &str,
        vote_kind: BugVoteKind,
    ) -> Result<bool, ToolFailure> {
        Ok(Self::bug_vote_references(client, vote_kind)
            .await?
            .iter()
            .any(|reference| reference == bug_ref))
    }

    async fn bug_vote_references(
        client: &BugboardClient,
        vote_kind: BugVoteKind,
    ) -> Result<Vec<String>, ToolFailure> {
        let result = client
            .execute_module_call(
                "bug_list_voted",
                bugboard::voted_bug_list_request(vote_kind.into())?,
            )
            .await?;
        bugboard::decode_bug_references(&result)
            .map_err(|error| ToolFailure::bugboard_changed("bug_list_voted", error))
    }

    async fn bug_get_value(&self, params: BugGetParams) -> Result<Value, ToolFailure> {
        let bug_ref = self.resolve_bug_params(&params).await?;
        let client = self.client()?;
        let parsed = client
            .execute("bug_get", bugboard::bug_get_request(&bug_ref)?)
            .await?;
        normalize_bug_details(parsed)
    }

    async fn bug_get_history_value(&self, params: BugGetParams) -> Result<Value, ToolFailure> {
        let bug_ref = self.resolve_bug_params(&params).await?;
        let client = self.client()?;
        let parsed = client
            .execute("bug_get_history", bugboard::bug_get_request(&bug_ref)?)
            .await?;
        let details = bugboard::decode_bug_details(&parsed)
            .map_err(|error| ToolFailure::bugboard_changed("bug_get_history", error))?;
        Ok(normalize_bug_history(details.history))
    }

    async fn bug_get_subscription_vote_state_value(
        &self,
        params: BugGetParams,
    ) -> Result<Value, ToolFailure> {
        let bug_ref = self.resolve_bug_params(&params).await?;
        let client = self.client()?;
        let state = client
            .execute_module_call(
                "bug_get_subscription_vote_state",
                bugboard::bug_subscription_vote_state_request(&bug_ref)?,
            )
            .await?;

        Ok(json!({
            "state": scrub_references(&state),
        }))
    }

    async fn bug_open_in_browser_value(&self, params: BugGetParams) -> Result<Value, ToolFailure> {
        let bug_ref = self.resolve_bug_params(&params).await?;
        let client = self.client()?;
        let parsed = client
            .execute("bug_get", bugboard::bug_get_request(&bug_ref)?)
            .await?;
        let details = normalize_bug_details(parsed)?;
        let url = details.get("url").and_then(Value::as_str).ok_or_else(|| {
            ToolFailure::empty_result("Bug details did not contain a browser URL.")
        })?;

        Ok(json!({
            "url": url,
        }))
    }

    async fn bug_references_response(
        &self,
        client: Arc<BugboardClient>,
        refs: Vec<String>,
        limit: u32,
        extra: Value,
    ) -> Result<Value, ToolFailure> {
        self.bug_references_response_with(refs, limit, extra, move |reference| {
            let client = Arc::clone(&client);
            async move {
                normalize_bug_details(
                    client
                        .execute("bug_get", bugboard::bug_get_request(&reference)?)
                        .await?,
                )
            }
        })
        .await
    }

    pub(crate) async fn bug_references_response_with<F, Fut>(
        &self,
        refs: Vec<String>,
        limit: u32,
        extra: Value,
        fetch: F,
    ) -> Result<Value, ToolFailure>
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, ToolFailure>> + Send + 'static,
    {
        let refs = refs.into_iter().take(limit as usize).collect::<Vec<_>>();
        let mut fetched = Vec::with_capacity(refs.len());
        let fetch = Arc::new(fetch);
        for chunk in refs.chunks(MAX_CONCURRENT_BUG_READS) {
            let mut pending = JoinSet::new();
            for (index, reference) in chunk.iter().cloned().enumerate() {
                let fetch = Arc::clone(&fetch);
                pending.spawn(async move {
                    let bug = fetch(reference.clone()).await?;
                    Ok::<_, ToolFailure>((index, (reference, bug)))
                });
            }
            fetched.extend(join_in_input_order(pending).await?);
        }
        let mut bugs = Vec::new();
        for (reference, mut bug) in fetched {
            if let Value::Object(object) = &mut bug {
                object.insert(
                    "bug_handle".to_owned(),
                    Value::String(self.remember_ref(HandleKind::Bug, &reference)?),
                );
            }
            bugs.push(bug);
        }

        Ok(json!({
            "bugs": bugs,
            "count": bugs.len(),
            "limit": limit,
            "filter": extra,
        }))
    }

    async fn resolve_bug_params(&self, params: &BugGetParams) -> Result<String, ToolFailure> {
        self.resolve_bug_parts(params.bug_handle.as_deref(), params.bug_number.as_deref())
            .await
    }

    pub(crate) async fn resolve_bug_parts(
        &self,
        bug_handle: Option<&str>,
        bug_number: Option<&str>,
    ) -> Result<String, ToolFailure> {
        let handle = bug_handle.map(str::trim).filter(|value| !value.is_empty());
        let number = bug_number.map(str::trim).filter(|value| !value.is_empty());
        match (handle, number) {
            (Some(handle), None) => self.resolve_ref(HandleKind::Bug, handle),
            (None, Some(number)) => self.resolve_bug_number(number).await,
            (Some(_), Some(_)) => Err(ToolFailure::invalid_arguments(
                "Set either bug_handle or bug_number, not both.",
            )),
            (None, None) => Err(ToolFailure::invalid_reference(
                "Set bug_handle from a list/search tool or set bug_number.",
            )),
        }
    }

    fn resolve_project_ref(&self, project_handle: &str) -> Result<String, ToolFailure> {
        let handle = project_handle.trim();
        if handle.is_empty() {
            return Err(ToolFailure::invalid_reference(
                "Set project_handle from project_list.",
            ));
        }
        self.resolve_ref(HandleKind::Project, handle)
    }

    pub(crate) fn resolve_version_ref(
        versions: &[bugboard::VersionRow],
        project_ref: &str,
        version_title: &str,
    ) -> Result<String, ToolFailure> {
        let mut matching_versions = versions
            .iter()
            .filter(|row| {
                let same_project = row.project_reference == project_ref;
                let same_title = row.title.as_deref() == Some(version_title);
                same_project && same_title
            })
            .map(|row| row.reference.as_str());
        match (matching_versions.next(), matching_versions.next()) {
            (Some(reference), None) => Ok(reference.to_owned()),
            (None, _) => Err(ToolFailure::empty_result(format!(
                "Version {version_title} was not found in project."
            ))),
            (Some(_), Some(_)) => Err(ToolFailure::invalid_arguments(
                "Version title is ambiguous. Use an exact title unique within the project.",
            )),
        }
    }

    async fn resolve_bug_number(&self, number: &str) -> Result<String, ToolFailure> {
        if !is_bug_number(number) {
            return Err(ToolFailure::invalid_arguments(
                "bug_number must contain only ASCII digits.",
            ));
        }
        let client = self.client()?;
        let parsed = client
            .execute_dynamic_list("bug_search", bugboard::bug_lookup_request(number)?)
            .await?;
        bugboard::decode_bug_rows(&parsed)
            .map_err(|error| ToolFailure::bugboard_changed("bug_search", error))?
            .into_iter()
            .find(|row| row.number.as_deref() == Some(number))
            .map(|row| row.reference)
            .ok_or_else(|| ToolFailure::empty_result(format!("Bug {number} was not found.")))
    }
}

#[derive(Default, Deserialize, JsonSchema)]
struct ListParams {
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct ProjectVersionsParams {
    project_handle: String,
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct VersionGetBugsParams {
    pub(crate) project_handle: String,
    pub(crate) version_title: String,
    pub(crate) limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct BugSearchParams {
    query: String,
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct ProjectWriteParams {
    project_handle: String,
}

#[derive(Deserialize, JsonSchema)]
struct BugWriteParams {
    bug_handle: String,
}

#[derive(Deserialize, JsonSchema)]
struct BugVoteWriteParams {
    bug_handle: String,
    vote_kind: BugVoteKind,
}

#[derive(Deserialize, JsonSchema)]
struct VotedBugListParams {
    vote_kind: BugVoteKind,
    limit: Option<u32>,
}

#[derive(Default, Deserialize, JsonSchema)]
struct BugGetParams {
    bug_handle: Option<String>,
    bug_number: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BugVoteKind {
    ManifestsForMe,
    FixImportant,
}

impl BugVoteKind {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::ManifestsForMe => 0,
            Self::FixImportant => 1,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ManifestsForMe => "manifests_for_me",
            Self::FixImportant => "fix_important",
        }
    }
}

pub(crate) async fn join_in_input_order<T: Send + 'static>(
    mut pending: JoinSet<Result<(usize, T), ToolFailure>>,
) -> Result<Vec<T>, ToolFailure> {
    let mut completed = Vec::with_capacity(pending.len());
    while let Some(result) = pending.join_next().await {
        let item = result.map_err(|_| ToolFailure::internal("bug detail worker failed"))??;
        completed.push(item);
    }
    completed.sort_unstable_by_key(|(index, _)| *index);
    Ok(completed.into_iter().map(|(_, value)| value).collect())
}

impl From<BugVoteKind> for e1c_element_rpc::bugboard::BugVoteKind {
    fn from(value: BugVoteKind) -> Self {
        match value {
            BugVoteKind::ManifestsForMe => Self::ManifestsForMe,
            BugVoteKind::FixImportant => Self::FixImportant,
        }
    }
}

fn ok(value: Value) -> CallToolResult {
    CallToolResult::structured(value)
}

async fn tool_result(work: impl Future<Output = Result<Value, ToolFailure>>) -> CallToolResult {
    match work.await {
        Ok(value) => ok(value),
        Err(error) => error.into_result(),
    }
}
pub(crate) fn normalize_limit(limit: Option<u32>) -> Result<u32, ToolFailure> {
    let limit = limit.unwrap_or(DEFAULT_LIST_LIMIT);
    if (1..=MAX_LIST_LIMIT).contains(&limit) {
        Ok(limit)
    } else {
        Err(ToolFailure::new(
            "invalid_arguments",
            format!("limit must be between 1 and {MAX_LIST_LIMIT}."),
            json!({
                "field": "limit",
                "minimum": 1,
                "maximum": MAX_LIST_LIMIT,
                "value": limit,
            }),
        ))
    }
}
