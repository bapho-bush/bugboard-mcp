#[cfg(test)]
mod tests {
    use crate::{
        allowed_hosts,
        client::BugboardClient,
        config::{SessionConfig, parse_env_file},
        ensure_loopback,
        errors::ToolFailure,
        handles::{HandleKind, HandleStore},
        http_server_config,
        normalize::{
            normalize_bug_details, normalize_bug_history, normalize_bug_row, normalize_project_row,
            normalize_version_row, scrub_references,
        },
        server::{BugboardServer, join_in_input_order, normalize_limit},
        write_safety::{RequestedState, WriteOutcome, WriteSafety, apply_write},
    };
    use e1c_element_rpc::bugboard::{
        BUG_REFERENCE_TYPE, BugRow, PROJECT_REFERENCE_TYPE, ProjectRow, VERSION_REFERENCE_TYPE,
        VersionRow,
    };
    use serde_json::{Value, json};

    #[test]
    fn session_config_reads_only_the_cookie_from_env_values() {
        let values = parse_env_file(
            r#"
            # outside repo
            BUGBOARD_COOKIE="session=value"
            BUGBOARD_HEADER_X_G5_VERSION=stale-and-ignored
            BUGBOARD_HEADER_X_OTHER=ignored
            "#,
        );
        let config = SessionConfig::from_values(&values).unwrap();

        assert_eq!(config.cookie, "session=value");
        assert!(!format!("{config:?}").contains("stale-and-ignored"));
    }

    #[test]
    fn session_config_accepts_a_direct_cookie_value() {
        let config = SessionConfig::from_cookie(Some("session=from-docker-env")).unwrap();

        assert_eq!(config.cookie, "session=from-docker-env");
        assert!(SessionConfig::from_cookie(Some("  ")).is_err());
    }

    #[test]
    fn server_registers_bugboard_tools() {
        let server = BugboardServer::new();
        let names = server
            .tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();

        for name in [
            "bugboard_auth_status",
            "project_list",
            "project_get_versions",
            "version_get_bugs",
            "project_list_subscribed",
            "project_subscribe",
            "project_unsubscribe",
            "bug_list_recent",
            "bug_search",
            "bug_list_subscribed",
            "bug_subscribe",
            "bug_unsubscribe",
            "bug_list_voted",
            "bug_vote",
            "bug_unvote",
            "bug_get",
            "bug_get_history",
            "bug_get_subscription_vote_state",
            "bug_open_in_browser",
        ] {
            assert!(names.contains(&name.to_owned()), "missing {name}");
        }
        assert_eq!(names.len(), 19);
        assert!(!names.contains(&"bug_get_comments".to_owned()));

        for (name, destructive) in [
            ("project_subscribe", false),
            ("project_unsubscribe", true),
            ("bug_subscribe", false),
            ("bug_unsubscribe", true),
            ("bug_vote", false),
            ("bug_unvote", true),
        ] {
            let tool = server
                .tool_router
                .list_all()
                .into_iter()
                .find(|tool| tool.name == name)
                .unwrap();
            let annotations = tool.annotations.unwrap();

            assert_eq!(annotations.read_only_hint, Some(false));
            assert_eq!(annotations.destructive_hint, Some(destructive));
            assert_eq!(annotations.idempotent_hint, Some(true));
            assert_eq!(annotations.open_world_hint, Some(true));
        }

        for (name, expected_properties) in [
            ("project_subscribe", &["project_handle"][..]),
            ("project_unsubscribe", &["project_handle"][..]),
            ("bug_subscribe", &["bug_handle"][..]),
            ("bug_unsubscribe", &["bug_handle"][..]),
            ("bug_vote", &["bug_handle", "vote_kind"][..]),
            ("bug_unvote", &["bug_handle", "vote_kind"][..]),
        ] {
            let tool = server
                .tool_router
                .list_all()
                .into_iter()
                .find(|tool| tool.name == name)
                .unwrap();
            let required = tool
                .input_schema
                .get("required")
                .and_then(Value::as_array)
                .unwrap();
            let properties = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .unwrap();

            assert_eq!(required.len(), expected_properties.len());
            assert_eq!(properties.len(), expected_properties.len());
            for property in expected_properties {
                assert!(required.contains(&json!(property)));
                assert!(properties.contains_key(*property));
            }
        }
    }

    #[test]
    fn http_allowed_hosts_include_bound_authority() {
        let addr = "127.0.0.1:8123".parse().unwrap();

        assert!(allowed_hosts(&addr).contains(&"127.0.0.1:8123".to_owned()));
    }

    #[tokio::test]
    async fn http_server_rejects_cross_origin_browser_requests() {
        use axum::{
            body::Body,
            http::{Method, Request, header::CONTENT_TYPE},
        };
        use rmcp::transport::{
            StreamableHttpService, streamable_http_server::session::local::LocalSessionManager,
        };

        let addr = "127.0.0.1:8123".parse().unwrap();
        let service: StreamableHttpService<BugboardServer, LocalSessionManager> =
            StreamableHttpService::new(
                || Ok(BugboardServer::new()),
                Default::default(),
                http_server_config(&addr),
            );
        let request = Request::builder()
            .method(Method::POST)
            .header("Accept", "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .header("Host", "127.0.0.1:8123")
            .header("Origin", "http://attacker.example")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "clientInfo": {"name": "test-client", "version": "1.0.0"}
                    }
                })
                .to_string(),
            ))
            .unwrap();

        let response = service.handle(request).await;

        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[test]
    fn handles_do_not_expose_references() {
        let mut store = HandleStore::default();

        let first = store.remember(HandleKind::Bug, "raw-reference");
        let second = store.remember(HandleKind::Bug, "raw-reference");

        assert_eq!(first, "bug-1");
        assert_eq!(first, second);
        assert_eq!(
            store.resolve(HandleKind::Bug, &first).as_deref(),
            Some("raw-reference")
        );
        assert_eq!(store.resolve(HandleKind::Project, &first), None);
    }

    #[test]
    fn project_row_normalization_returns_human_fields() {
        let server = BugboardServer::new();
        let row = ProjectRow {
            reference: "project-ref".to_owned(),
            abbreviation: Some("ERP".to_owned()),
            title: Some("1C:ERP".to_owned()),
            deleted: Some(false),
            updated_at: Some("2026-07-01".to_owned()),
            group_order: None,
            order: None,
        };

        let project = normalize_project_row(&server, &row).unwrap();

        assert_eq!(project["project_handle"], "project-1");
        assert_eq!(project["title"], "1C:ERP");
        assert_eq!(project["abbreviation"], "ERP");
        assert!(!project.to_string().contains("project-ref"));
    }

    #[test]
    fn version_row_normalization_returns_human_fields() {
        let row = VersionRow {
            reference: "version-ref".to_owned(),
            title: Some("8.3.27".to_owned()),
            project_reference: "project-ref".to_owned(),
            source_order: Some(27),
        };

        let version = normalize_version_row(&row).unwrap();

        assert_eq!(version["title"], "8.3.27");
        assert_eq!(version["source_order"], 27);
        assert!(!version.to_string().contains("version-ref"));
    }

    #[test]
    fn bug_row_normalization_returns_human_fields() {
        let server = BugboardServer::new();
        let row = BugRow {
            reference: "bug-ref".to_owned(),
            number: Some("123456".to_owned()),
            title: Some("Crash on start".to_owned()),
            updated_at: Some("2026-07-01".to_owned()),
            published_at: Some("2026-06-30".to_owned()),
            status: Some("Published".to_owned()),
            status_code: None,
        };

        let bug = normalize_bug_row(&server, &row).unwrap();

        assert_eq!(bug["bug_handle"], "bug-1");
        assert_eq!(bug["number"], "123456");
        assert_eq!(bug["title"], "Crash on start");
        assert!(!bug.to_string().contains("bug-ref"));
    }

    #[test]
    fn bug_row_normalization_drops_default_dates() {
        let server = BugboardServer::new();
        let row = BugRow {
            reference: "bug-ref".to_owned(),
            number: Some("123456".to_owned()),
            title: None,
            updated_at: Some("2026-07-01".to_owned()),
            published_at: None,
            status: None,
            status_code: None,
        };

        let bug = normalize_bug_row(&server, &row).unwrap();

        assert_eq!(bug["title"], Value::Null);
        assert_eq!(bug["published_at"], Value::Null);
        assert_eq!(bug["updated_at"], "2026-07-01");
    }

    #[test]
    fn scrub_references_redacts_nested_reference_values() {
        let value = json!({
            "safe": "text",
            "nested": [{
                "type": BUG_REFERENCE_TYPE,
                "value": "raw-reference"
            }]
        });

        let scrubbed = scrub_references(&value);

        assert_eq!(scrubbed["safe"], "text");
        assert_eq!(scrubbed["nested"][0]["value"], "<redacted-reference>");
        assert!(!scrubbed.to_string().contains("raw-reference"));
    }

    #[test]
    fn bug_history_output_redacts_all_references_and_keeps_the_stable_envelope() {
        let history = vec![json!({
            "ВерсияПривнесения": {"type": VERSION_REFERENCE_TYPE, "value": "introduced-ref"},
            "ВерсияИсправления": {"type": VERSION_REFERENCE_TYPE, "value": "fixed-ref"},
            "Проект": {"type": PROJECT_REFERENCE_TYPE, "value": "project-ref"},
            "Исправлена": true,
        })];

        let output = normalize_bug_history(history);

        assert_eq!(output["count"], 1);
        assert_eq!(output["history"][0]["Исправлена"], true);
        assert_eq!(
            output["history"][0]["ВерсияИсправления"]["value"],
            "<redacted-reference>"
        );
        assert!(!output.to_string().contains("introduced-ref"));
        assert!(!output.to_string().contains("fixed-ref"));
        assert!(!output.to_string().contains("project-ref"));
    }

    #[test]
    fn http_bind_rejects_non_loopback_addresses() {
        assert!(ensure_loopback(&"127.0.0.1:8000".parse().unwrap()).is_ok());
        assert!(ensure_loopback(&"0.0.0.0:8000".parse().unwrap()).is_err());
    }

    #[tokio::test]
    async fn conflicting_bug_identifiers_are_rejected() {
        let server = BugboardServer::new();

        assert!(
            server
                .resolve_bug_parts(Some("bug-1"), Some("60024806"))
                .await
                .is_err()
        );
        assert!(
            server
                .resolve_bug_parts(None, Some("not-a-number"))
                .await
                .is_err()
        );
    }

    #[test]
    fn session_config_debug_redacts_session_values() {
        let config = SessionConfig {
            cookie: "session=secret".to_owned(),
        };
        let debug = format!("{config:?}");

        assert!(!debug.contains("secret"));
    }

    #[test]
    fn client_rejects_invalid_cookie_as_config_error() {
        let result = BugboardClient::new(SessionConfig {
            cookie: "session=invalid\nvalue".to_owned(),
        });
        let Err(error) = result else {
            panic!("invalid Cookie must be rejected");
        };
        let error = error.into_result();

        assert_eq!(
            error
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/error/code")),
            Some(&json!("config_error"))
        );
        assert!(
            !error
                .structured_content
                .expect("structured config error")
                .to_string()
                .contains("session=invalid")
        );
    }

    #[test]
    fn client_accepts_valid_session_values() {
        assert!(
            BugboardClient::new(SessionConfig {
                cookie: "session=value".to_owned(),
            })
            .is_ok()
        );
    }

    #[test]
    fn strict_limit_validation_rejects_out_of_range_values() {
        assert_eq!(normalize_limit(None).unwrap(), 10);
        assert_eq!(normalize_limit(Some(1)).unwrap(), 1);
        assert_eq!(normalize_limit(Some(50)).unwrap(), 50);

        let zero = format!("{:?}", normalize_limit(Some(0)).unwrap_err());
        assert!(zero.contains("invalid_arguments"));
        assert!(zero.contains("limit must be between 1 and 50."));

        let too_large = format!("{:?}", normalize_limit(Some(51)).unwrap_err());
        assert!(too_large.contains("invalid_arguments"));
        assert!(too_large.contains("value"));
        assert!(too_large.contains("51"));
    }

    #[test]
    fn bug_search_and_list_tool_schemas_match_the_contract() {
        let server = BugboardServer::new();
        let tools = server.tool_router.list_all();

        let bug_list_recent = tools
            .iter()
            .find(|tool| tool.name == "bug_list_recent")
            .unwrap();
        let recent_properties = bug_list_recent
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .unwrap();
        assert_eq!(recent_properties.len(), 1);
        assert!(recent_properties.contains_key("limit"));
        let recent_required = bug_list_recent
            .input_schema
            .get("required")
            .and_then(Value::as_array);
        assert!(match recent_required {
            None => true,
            Some(required) => required.is_empty(),
        });

        let bug_search = tools.iter().find(|tool| tool.name == "bug_search").unwrap();
        let search_required = bug_search
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .unwrap();
        assert!(search_required.contains(&json!("query")));

        let bug_list_subscribed = tools
            .iter()
            .find(|tool| tool.name == "bug_list_subscribed")
            .unwrap();
        let subscribed_properties = bug_list_subscribed
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .unwrap();
        assert_eq!(subscribed_properties.len(), 1);
        assert!(subscribed_properties.contains_key("limit"));

        let bug_list_voted = tools
            .iter()
            .find(|tool| tool.name == "bug_list_voted")
            .unwrap();
        let voted_required = bug_list_voted
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .unwrap();
        assert!(voted_required.contains(&json!("vote_kind")));

        let version_get_bugs = tools
            .iter()
            .find(|tool| tool.name == "version_get_bugs")
            .unwrap();
        let version_required = version_get_bugs
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .unwrap();
        assert!(version_required.contains(&json!("project_handle")));
        assert!(version_required.contains(&json!("version_title")));
        let version_properties = version_get_bugs
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .unwrap();
        assert_eq!(version_properties.len(), 3);
        assert!(version_properties.contains_key("limit"));
    }

    #[test]
    fn bug_details_normalization_unwraps_typed_object_value() {
        let response = json!({
            "object": {
                "type": "e1c::bugboard::Багборд::Ошибки",
                "value": {
                    "Наименование": "70116179",
                    "Заголовок": "Readable title",
                    "РассчитанныйСтатусБагборда": "Published",
                    "ДатаПубликации": "2026-01-01",
                    "ДатаПоследнегоОбновления": "2026-01-02",
                    "Комментарии": "",
                    "ИсторияЖизни": {
                        "type": "Std::Collections::Array<e1c::bugboard::Багборд::Ошибки.ИсторияЖизни>",
                        "value": {
                            "items": [{
                                "type": "e1c::bugboard::Багборд::Ошибки.ИсторияЖизни",
                                "value": {
                                    "Проект": {"type": PROJECT_REFERENCE_TYPE, "value": "project-ref"},
                                    "Исправлена": true
                                }
                            }]
                        }
                    }
                }
            }
        });

        let details = normalize_bug_details(response).unwrap();

        assert_eq!(details["number"], "70116179");
        assert_eq!(details["title"], "Readable title");
        assert!(details.get("comments_count").is_none());
        assert_eq!(details["history_count"], 1);
    }

    #[test]
    fn bug_details_normalization_maps_status_enum() {
        let response = json!({
            "object": {
                "value": {
                    "Наименование": "60024806",
                    "Заголовок": "Crash",
                    "РассчитанныйСтатусБагборда": {
                        "type": "e1c::bugboard::Багборд::СтатусыОшибок",
                        "value": 8
                    },
                    "ИсторияЖизни": {
                        "type": "Std::Collections::Array<e1c::bugboard::Багборд::Ошибки.ИсторияЖизни>",
                        "value": {"items": []}
                    }
                }
            }
        });

        let details = normalize_bug_details(response).unwrap();

        assert_eq!(details["status"], "Отклонена");
        assert_eq!(details["status_code"], 8);
    }

    #[tokio::test]
    async fn concurrent_results_are_restored_to_input_order() {
        let (release_slow, slow) = tokio::sync::oneshot::channel();
        let (fast_done, fast) = tokio::sync::oneshot::channel();
        let mut pending = tokio::task::JoinSet::new();
        pending.spawn(async move {
            slow.await.unwrap();
            Ok((0, "first"))
        });
        pending.spawn(async move {
            fast_done.send(()).unwrap();
            Ok((1, "second"))
        });

        fast.await.unwrap();
        release_slow.send(()).unwrap();

        assert_eq!(
            join_in_input_order(pending).await.unwrap(),
            ["first", "second"]
        );
    }

    #[tokio::test]
    async fn concurrent_error_returns_without_waiting_for_pending_sibling() {
        let mut pending = tokio::task::JoinSet::new();
        pending.spawn(std::future::pending::<
            Result<(usize, &'static str), ToolFailure>,
        >());
        pending.spawn(async { Err(ToolFailure::transport("boom")) });

        let error = tokio::select! {
            result = join_in_input_order(pending) => result.unwrap_err(),
            _ = async {
                for _ in 0..1_000 {
                    tokio::task::yield_now().await;
                }
            } => panic!("error path must not wait for pending siblings"),
        };
        let failure = error.into_result();

        assert_eq!(
            failure
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/error/code")),
            Some(&json!("transport_error"))
        );
    }

    #[tokio::test]
    async fn chunked_bug_enrichment_preserves_response_and_handle_order() {
        let server = BugboardServer::new();
        let refs = (0..10).map(|index| format!("raw-{index}")).collect();
        let response = server
            .bug_references_response_with(
                refs,
                10,
                json!({"source": "test"}),
                |reference| async move { Ok(json!({"number": reference})) },
            )
            .await
            .unwrap();
        let bugs = response["bugs"].as_array().unwrap();

        assert_eq!(response["count"], 10);
        assert_eq!(response["limit"], 10);
        assert_eq!(response["filter"]["source"], "test");
        for (index, bug) in bugs.iter().enumerate() {
            assert_eq!(bug["number"], format!("raw-{index}"));
            assert_eq!(bug["bug_handle"], format!("bug-{}", index + 1));
        }
    }

    #[tokio::test]
    async fn later_chunk_failure_does_not_allocate_partial_handles() {
        let server = BugboardServer::new();
        let refs = (0..10).map(|index| format!("raw-{index}")).collect();
        let result = server
            .bug_references_response_with(refs, 10, json!({}), |reference| async move {
                if reference == "raw-9" {
                    Err(ToolFailure::transport("boom"))
                } else {
                    Ok(json!({"number": reference}))
                }
            })
            .await;

        assert!(result.is_err());
        assert!(server.resolve_bug_parts(Some("bug-1"), None).await.is_err());
    }

    #[test]
    fn version_resolution_requires_an_exact_unique_title_in_the_project() {
        let project_ref = "project-ref";
        let rows = vec![
            VersionRow {
                reference: "version-a".to_owned(),
                title: Some("release-title".to_owned()),
                project_reference: project_ref.to_owned(),
                source_order: Some(1),
            },
            VersionRow {
                reference: "version-b".to_owned(),
                title: Some("release-title".to_owned()),
                project_reference: project_ref.to_owned(),
                source_order: Some(2),
            },
        ];

        assert_eq!(
            BugboardServer::resolve_version_ref(&rows[..1], project_ref, "release-title").unwrap(),
            "version-a"
        );

        let ambiguous = BugboardServer::resolve_version_ref(&rows, project_ref, "release-title")
            .unwrap_err()
            .into_result();

        assert_eq!(
            ambiguous
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/error/code")),
            Some(&json!("invalid_arguments"))
        );

        let missing = BugboardServer::resolve_version_ref(&[], project_ref, "release-title")
            .unwrap_err()
            .into_result();

        assert_eq!(
            missing
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/error/code")),
            Some(&json!("empty_result"))
        );
    }

    #[test]
    fn write_safety_skips_an_already_satisfied_request() {
        let safety = WriteSafety::new(RequestedState::Enabled, true);

        assert!(!safety.needs_mutation());
        assert_eq!(
            safety.finish(true).unwrap(),
            crate::write_safety::WriteOutcome {
                requested: true,
                changed: false,
                previous_state: true,
                final_state: true,
            }
        );
    }

    #[test]
    fn write_safety_reports_a_verified_change() {
        let safety = WriteSafety::new(RequestedState::Disabled, true);

        assert!(safety.needs_mutation());
        assert_eq!(
            safety.finish(false).unwrap(),
            crate::write_safety::WriteOutcome {
                requested: false,
                changed: true,
                previous_state: true,
                final_state: false,
            }
        );
    }

    #[test]
    fn write_safety_rejects_an_unmet_postcondition() {
        let failure = WriteSafety::new(RequestedState::Enabled, false)
            .finish(false)
            .unwrap_err()
            .into_result();
        let error = failure.structured_content.unwrap();

        assert_eq!(
            error.pointer("/error/code"),
            Some(&json!("write_postcondition_failed"))
        );
        assert_eq!(
            error.pointer("/error/details/requested"),
            Some(&json!(true))
        );
        assert_eq!(
            error.pointer("/error/details/previous_state"),
            Some(&json!(false))
        );
        assert_eq!(
            error.pointer("/error/details/final_state"),
            Some(&json!(false))
        );
    }

    #[tokio::test]
    async fn write_safety_does_not_mutate_an_already_satisfied_state() {
        let mutation_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = std::sync::Arc::clone(&mutation_calls);
        let outcome = apply_write(
            RequestedState::Enabled,
            || std::future::ready(Ok(true)),
            move |_| {
                calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                std::future::ready(Ok(()))
            },
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            WriteOutcome {
                requested: true,
                changed: false,
                previous_state: true,
                final_state: true,
            }
        );
        assert_eq!(mutation_calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn write_safety_uses_post_state_after_a_lost_mutation_response() {
        let states = std::sync::Mutex::new(vec![true, false].into_iter());
        let outcome = apply_write(
            RequestedState::Disabled,
            || std::future::ready(Ok(states.lock().unwrap().next().unwrap())),
            |_| std::future::ready(Err(ToolFailure::transport("lost response"))),
        )
        .await
        .unwrap();

        assert!(outcome.changed);
        assert!(!outcome.final_state);
    }

    #[tokio::test]
    async fn write_safety_preserves_an_explicit_mutation_rejection() {
        let states = std::sync::Mutex::new(vec![false, true].into_iter());
        let failure = apply_write(
            RequestedState::Enabled,
            || std::future::ready(Ok(states.lock().unwrap().next().unwrap())),
            |_| {
                std::future::ready(Err(ToolFailure::new(
                    "unknown_bugboard_error",
                    "Bugboard rejected the mutation.",
                    json!({}),
                )))
            },
        )
        .await
        .unwrap_err()
        .into_result();

        assert_eq!(
            failure
                .structured_content
                .and_then(|value| value.pointer("/error/code").cloned()),
            Some(json!("unknown_bugboard_error"))
        );
    }

    #[tokio::test]
    async fn write_safety_reports_an_unmet_state_after_uncertain_delivery() {
        let failure = apply_write(
            RequestedState::Enabled,
            || std::future::ready(Ok(false)),
            |_| std::future::ready(Err(ToolFailure::transport("lost response"))),
        )
        .await
        .unwrap_err()
        .into_result();

        assert_eq!(
            failure
                .structured_content
                .and_then(|value| value.pointer("/error/code").cloned()),
            Some(json!("write_postcondition_failed"))
        );
    }

    #[tokio::test]
    async fn write_safety_preserves_rejection_when_post_read_fails() {
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let failure = apply_write(
            RequestedState::Enabled,
            || {
                let read = reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                std::future::ready(if read == 0 {
                    Ok(false)
                } else {
                    Err(ToolFailure::transport("post-read failed"))
                })
            },
            |_| {
                std::future::ready(Err(ToolFailure::new(
                    "mutation_rejected",
                    "Bugboard rejected the mutation.",
                    json!({}),
                )))
            },
        )
        .await
        .unwrap_err()
        .into_result();

        assert_eq!(
            failure
                .structured_content
                .and_then(|value| value.pointer("/error/code").cloned()),
            Some(json!("mutation_rejected"))
        );
    }
}
