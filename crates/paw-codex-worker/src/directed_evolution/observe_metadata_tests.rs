#[cfg(test)]
mod directed_evolution_observe_metadata_tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::io::{AsyncReadExt as ObserveAsyncReadExt, AsyncWriteExt as ObserveAsyncWriteExt};

    const FULL_CORRELATION: &str = r#"{
        "episode_id": "ep-1",
        "direction_id": "dir-1",
        "generation_id": "gen-1",
        "variant_id": "var-1",
        "evaluation_stage_id": "stage-1",
        "stage_result_id": "sr-1",
        "trial_id": "trial-1",
        "simulated_user_plan_id": "plan-1",
        "persona_index": 2,
        "run_index": 1,
        "runtime_ref": "temper://tenant/de-variant-1/app/nerdsane/agent-answers@abc123",
        "app_ref": "nerdsane/agent-answers@abc123",
        "runtime_tenant": "de-variant-1",
        "runtime_base_url": "https://temper.example",
        "unrelated_key": "ignored"
    }"#;

    fn observe_test_work_item(role: &str, correlation_json: &str) -> DirectedEvolutionWorkItemState {
        DirectedEvolutionWorkItemState {
            id: "wi-1".to_string(),
            status: "Running".to_string(),
            role: role.to_string(),
            target_entity_type: "Trial".to_string(),
            target_entity_id: "trial-1".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: correlation_json.to_string(),
            worker_run_id: String::new(),
        }
    }

    #[test]
    fn join_fields_resolve_all_known_correlation_keys_once() {
        let join = directed_evolution_join_fields(FULL_CORRELATION);

        assert_eq!(join.episode_id, "ep-1");
        assert_eq!(join.direction_id, "dir-1");
        assert_eq!(join.generation_id, "gen-1");
        assert_eq!(join.variant_id, "var-1");
        assert_eq!(join.evaluation_stage_id, "stage-1");
        assert_eq!(join.stage_result_id, "sr-1");
        assert_eq!(join.trial_id, "trial-1");
        assert_eq!(join.simulated_user_plan_id, "plan-1");
        assert_eq!(join.persona_index, "2");
        assert_eq!(join.run_index, "1");
        assert_eq!(
            join.runtime_ref,
            "temper://tenant/de-variant-1/app/nerdsane/agent-answers@abc123"
        );
        assert_eq!(join.app_ref, "nerdsane/agent-answers@abc123");
        assert_eq!(join.runtime_tenant, "de-variant-1");
    }

    #[test]
    fn join_fields_tolerate_invalid_correlation_json() {
        let join = directed_evolution_join_fields("not-json");

        assert_eq!(join, DirectedEvolutionJoinFields::default());
        assert!(join.entries().is_empty());
    }

    #[test]
    fn observe_metadata_value_carries_producer_and_de_join_keys() {
        let join = directed_evolution_join_fields(FULL_CORRELATION);

        let value =
            directed_evolution_observe_metadata_value("wi-1", "wr-1", "simulated_user", &join);
        let parsed: Value = serde_json::from_str(&value).expect("header value should be JSON");

        assert_eq!(parsed["producer.work_item_id"], "wi-1");
        assert_eq!(parsed["producer.worker_run_id"], "wr-1");
        assert_eq!(parsed["de.role"], "simulated_user");
        assert_eq!(parsed["de.episode_id"], "ep-1");
        assert_eq!(parsed["de.direction_id"], "dir-1");
        assert_eq!(parsed["de.generation_id"], "gen-1");
        assert_eq!(parsed["de.variant_id"], "var-1");
        assert_eq!(parsed["de.evaluation_stage_id"], "stage-1");
        assert_eq!(parsed["de.stage_result_id"], "sr-1");
        assert_eq!(parsed["de.trial_id"], "trial-1");
        assert_eq!(parsed["de.simulated_user_plan_id"], "plan-1");
        assert_eq!(parsed["de.persona_index"], "2");
        assert_eq!(parsed["de.run_index"], "1");
        assert_eq!(
            parsed["de.runtime_ref"],
            "temper://tenant/de-variant-1/app/nerdsane/agent-answers@abc123"
        );
        assert_eq!(parsed["de.app_ref"], "nerdsane/agent-answers@abc123");
        assert_eq!(parsed["de.runtime_tenant"], "de-variant-1");
        assert!(parsed.get("de.unrelated_key").is_none());
        assert!(parsed.get("de.runtime_base_url").is_none());
        assert!(!value.contains('\n'), "header values must be single-line");
    }

    #[test]
    fn observe_metadata_value_skips_empty_fields() {
        let join = directed_evolution_join_fields(r#"{"episode_id":"ep-1"}"#);

        let value = directed_evolution_observe_metadata_value("wi-1", "", "observer", &join);
        let parsed: Value = serde_json::from_str(&value).expect("header value should be JSON");

        assert_eq!(parsed["producer.work_item_id"], "wi-1");
        assert!(parsed.get("producer.worker_run_id").is_none());
        assert_eq!(parsed["de.role"], "observer");
        assert_eq!(parsed["de.episode_id"], "ep-1");
        assert!(parsed.get("de.variant_id").is_none());
        assert!(parsed.get("de.runtime_tenant").is_none());
    }

    #[test]
    fn observe_metadata_value_truncates_values_to_kernel_limit() {
        let long = "x".repeat(5_000);
        let join = directed_evolution_join_fields(&format!(r#"{{"episode_id":"{long}"}}"#));

        let value = directed_evolution_observe_metadata_value("wi-1", "wr-1", "observer", &join);
        let parsed: Value = serde_json::from_str(&value).expect("header value should be JSON");

        let episode = parsed["de.episode_id"].as_str().expect("episode id string");
        assert_eq!(episode.len(), 1_024);
        assert!(episode.chars().all(|ch| ch == 'x'));
    }

    #[test]
    fn observe_metadata_value_caps_total_keys_at_kernel_limit() {
        let join = directed_evolution_join_fields(FULL_CORRELATION);

        let value =
            directed_evolution_observe_metadata_value("wi-1", "wr-1", "simulated_user", &join);
        let parsed: Value = serde_json::from_str(&value).expect("header value should be JSON");

        assert!(parsed.as_object().expect("object").len() <= 32);
    }

    #[test]
    fn dd_tags_join_de_pairs_and_preserve_existing_tags() {
        let join = directed_evolution_join_fields(FULL_CORRELATION);

        let tags = directed_evolution_dd_tags(
            Some("env:prod,service:temperpaw"),
            "simulated_user",
            &join,
        );

        assert!(tags.starts_with("env:prod,service:temperpaw,de.role:simulated_user"));
        assert!(tags.contains("de.episode_id:ep-1"));
        assert!(tags.contains("de.trial_id:trial-1"));
        assert!(tags.contains("de.persona_index:2"));
        assert!(tags.contains("de.runtime_tenant:de-variant-1"));
    }

    #[test]
    fn dd_tags_sanitize_values_for_datadog() {
        let join = directed_evolution_join_fields(r#"{"episode_id":"ep 1,with comma"}"#);

        let tags = directed_evolution_dd_tags(None, "observer", &join);

        assert_eq!(tags, "de.role:observer,de.episode_id:ep_1_with_comma");
    }

    #[test]
    fn codex_child_env_injects_observe_metadata_and_dd_tags() {
        let work_item = observe_test_work_item("simulated_user", FULL_CORRELATION);
        let join = directed_evolution_join_fields(&work_item.correlation_json);

        let env = directed_evolution_codex_child_env(&work_item, "wr-1", &join, None);

        let observe = env
            .iter()
            .find(|(key, _)| key == "TEMPER_OBSERVE_METADATA")
            .map(|(_, value)| value.clone())
            .expect("child env should carry TEMPER_OBSERVE_METADATA");
        assert_eq!(
            observe,
            directed_evolution_observe_metadata_value("wi-1", "wr-1", "simulated_user", &join)
        );
        let dd_tags = env
            .iter()
            .find(|(key, _)| key == "DD_TAGS")
            .map(|(_, value)| value.clone())
            .expect("child env should carry DD_TAGS");
        assert!(dd_tags.contains("de.role:simulated_user"));
        assert!(dd_tags.contains("de.episode_id:ep-1"));
    }

    #[test]
    fn codex_child_env_appends_to_existing_dd_tags() {
        let work_item = observe_test_work_item("simulated_user", FULL_CORRELATION);
        let join = directed_evolution_join_fields(&work_item.correlation_json);

        let env =
            directed_evolution_codex_child_env(&work_item, "wr-1", &join, Some("env:prod"));

        let dd_tags = env
            .iter()
            .find(|(key, _)| key == "DD_TAGS")
            .map(|(_, value)| value.clone())
            .expect("child env should carry DD_TAGS");
        assert!(dd_tags.starts_with("env:prod,de.role:simulated_user"));
    }

    fn observe_test_config(temper_url: String) -> Config {
        Config {
            temper_url,
            tenant: "default".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            worker_token: Some("secret".to_string()),
            workspace_root: PathBuf::from("/tmp/worktrees"),
            repo_root: PathBuf::from("/tmp/temperpaw"),
            codex_bin: "codex".to_string(),
            max_concurrent_runs: 1,
            enable_execution: false,
            poll_on_start: true,
            codex_exec_smoke: false,
            codex_exec_timeout: Duration::from_secs(30),
        }
    }

    /// One-shot HTTP server that records the raw request text and replies
    /// with a fixed JSON body.
    async fn capture_one_request(
        response_body: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind capture server");
        let addr = listener.local_addr().expect("capture server addr");
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 1024];
            let header_end = loop {
                let read = ObserveAsyncReadExt::read(&mut stream, &mut chunk)
                    .await
                    .expect("read request");
                if read == 0 {
                    break None;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                    break Some(pos);
                }
            };
            if let Some(header_end) = header_end {
                let header_text = String::from_utf8_lossy(&buffer[..header_end]).to_string();
                let content_length = header_text
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                let body_start = header_end + 4;
                while buffer.len() < body_start + content_length {
                    let read = ObserveAsyncReadExt::read(&mut stream, &mut chunk)
                        .await
                        .expect("read request body");
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            ObserveAsyncWriteExt::write_all(&mut stream, response.as_bytes())
                .await
                .expect("write response");
            let _ = ObserveAsyncWriteExt::shutdown(&mut stream).await;
            String::from_utf8_lossy(&buffer).to_string()
        });
        (format!("http://{addr}"), handle)
    }

    fn observe_metadata_header_line(raw_request: &str) -> Option<String> {
        raw_request
            .lines()
            .find(|line| {
                line.to_ascii_lowercase()
                    .starts_with("x-temper-observe-metadata:")
            })
            .map(str::to_string)
    }

    #[tokio::test]
    async fn directed_evolution_action_posts_carry_observe_metadata_header() {
        let (url, capture) = capture_one_request("{}").await;
        let config = observe_test_config(url);
        let client = reqwest::Client::new();
        let join = directed_evolution_join_fields(r#"{"episode_id":"ep-1"}"#);
        let metadata =
            directed_evolution_observe_metadata_value("wi-1", "wr-1", "observer", &join);

        post_directed_evolution_action(
            &client,
            &config,
            "StageResults",
            "sr-1",
            "EliminateStageResult",
            json!({}),
            Some(&metadata),
        )
        .await
        .expect("post action");

        let raw = capture.await.expect("captured request");
        let header = observe_metadata_header_line(&raw)
            .expect("DE action posts should carry x-temper-observe-metadata");
        assert!(header.contains("producer.work_item_id"));
        assert!(header.contains("de.episode_id"));
    }

    #[tokio::test]
    async fn paw_orchestration_posts_carry_observe_metadata_header() {
        let (url, capture) = capture_one_request("{}").await;
        let config = observe_test_config(url);
        let client = reqwest::Client::new();
        let join = directed_evolution_join_fields(r#"{"trial_id":"trial-1"}"#);
        let metadata =
            directed_evolution_observe_metadata_value("wi-1", "wr-1", "simulated_user", &join);

        post_paw_orchestration_action(
            &client,
            &config,
            "WorkItems",
            "wi-1",
            "StartWorkItem",
            json!({ "WorkerRunId": "wr-1" }),
            Some(&metadata),
        )
        .await
        .expect("post action");

        let raw = capture.await.expect("captured request");
        let header = observe_metadata_header_line(&raw)
            .expect("orchestration posts should carry x-temper-observe-metadata");
        assert!(header.contains("de.trial_id"));
        assert!(header.contains("producer.worker_run_id"));
    }

    #[tokio::test]
    async fn directed_evolution_entity_creates_carry_observe_metadata_header() {
        let (url, capture) = capture_one_request(r#"{"entity_id":"e-1"}"#).await;
        let config = observe_test_config(url);
        let client = reqwest::Client::new();
        let join = directed_evolution_join_fields(r#"{"episode_id":"ep-1"}"#);
        let metadata =
            directed_evolution_observe_metadata_value("wi-1", "", "observer", &join);

        let id = create_entity_with_observe_metadata(
            &client,
            &config,
            "WorkerRuns",
            json!({}),
            Some(&metadata),
        )
        .await
        .expect("create entity");

        assert_eq!(id, "e-1");
        let raw = capture.await.expect("captured request");
        let header = observe_metadata_header_line(&raw)
            .expect("DE entity creates should carry x-temper-observe-metadata");
        assert!(header.contains("producer.work_item_id"));
    }

    #[tokio::test]
    async fn observer_runtime_probe_sends_observe_metadata_header() {
        let (url, capture) = capture_one_request("{}").await;
        let client = reqwest::Client::new();
        let join = directed_evolution_join_fields(r#"{"runtime_tenant":"de-variant-1"}"#);
        let metadata = directed_evolution_observe_metadata_value("wi-1", "", "observer", &join);

        let result = observer_runtime_get_text(
            &client,
            &format!("{url}/tdata/$metadata"),
            "de-variant-1",
            None,
            Some(&metadata),
        )
        .await;

        assert_eq!(result["status"], "available");
        let raw = capture.await.expect("captured request");
        let header = observe_metadata_header_line(&raw)
            .expect("observer runtime probes should carry x-temper-observe-metadata");
        assert!(header.contains("producer.work_item_id"));
        assert!(header.contains("de.role"));
        assert!(header.contains("de.runtime_tenant"));
    }

    #[tokio::test]
    async fn codex_child_process_receives_observe_env_mechanically() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fake-codex.sh");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "paw-codex-worker-observe-env-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("temp dir");
        let mut config = observe_test_config("http://127.0.0.1:3497".to_string());
        config.codex_bin = fixture.display().to_string();
        let work_item = observe_test_work_item("simulated_user", FULL_CORRELATION);
        let join = directed_evolution_join_fields(&work_item.correlation_json);
        let child_env = directed_evolution_codex_child_env(&work_item, "wr-1", &join, None);

        let output = run_codex_exec_command_with_env(
            &config,
            &root,
            "PAW_FAKE_CODEX_PRINT_OBSERVE_ENV: report".to_string(),
            "observe env fixture",
            &child_env,
        )
        .await
        .expect("run fixture codex");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(r#""producer.work_item_id":"wi-1""#),
            "child should see TEMPER_OBSERVE_METADATA: {stdout}"
        );
        assert!(
            stdout.contains("de.episode_id:ep-1"),
            "child should see DD_TAGS: {stdout}"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn datadog_context_resolves_full_join_field_set() {
        let work_item = observe_test_work_item("simulated_user", FULL_CORRELATION);

        let context = directed_evolution_datadog_context(&work_item);

        assert_eq!(context["work_item_id"], "wi-1");
        assert_eq!(context["role"], "simulated_user");
        assert_eq!(context["episode_id"], "ep-1");
        assert_eq!(context["direction_id"], "dir-1");
        assert_eq!(context["generation_id"], "gen-1");
        assert_eq!(context["variant_id"], "var-1");
        assert_eq!(context["evaluation_stage_id"], "stage-1");
        assert_eq!(context["stage_result_id"], "sr-1");
        assert_eq!(context["trial_id"], "trial-1");
        assert_eq!(context["simulated_user_plan_id"], "plan-1");
        assert_eq!(context["persona_index"], "2");
        assert_eq!(context["run_index"], "1");
        assert_eq!(
            context["runtime_ref"],
            "temper://tenant/de-variant-1/app/nerdsane/agent-answers@abc123"
        );
        assert_eq!(context["app_ref"], "nerdsane/agent-answers@abc123");
        assert_eq!(context["runtime_tenant"], "de-variant-1");
    }

    #[test]
    fn datadog_context_omits_unresolved_join_fields() {
        let work_item = observe_test_work_item("observer", r#"{"episode_id":"ep-1"}"#);

        let context = directed_evolution_datadog_context(&work_item);

        assert_eq!(context["episode_id"], "ep-1");
        assert!(context.get("variant_id").is_none());
        assert!(context.get("trial_id").is_none());
    }

    #[test]
    fn directed_evolution_prompt_header_includes_resolved_join_fields() {
        let work_item = observe_test_work_item("simulated_user", FULL_CORRELATION);

        let prompt = directed_evolution_prompt(&work_item);

        assert!(prompt.contains("ResolvedCorrelation:"));
        assert!(prompt.contains("de.episode_id: ep-1"));
        assert!(prompt.contains("de.direction_id: dir-1"));
        assert!(prompt.contains("de.generation_id: gen-1"));
        assert!(prompt.contains("de.variant_id: var-1"));
        assert!(prompt.contains("de.evaluation_stage_id: stage-1"));
        assert!(prompt.contains("de.stage_result_id: sr-1"));
        assert!(prompt.contains("de.trial_id: trial-1"));
        assert!(prompt.contains("de.simulated_user_plan_id: plan-1"));
        assert!(prompt.contains("de.persona_index: 2"));
        assert!(prompt.contains("de.run_index: 1"));
        assert!(prompt.contains(
            "de.runtime_ref: temper://tenant/de-variant-1/app/nerdsane/agent-answers@abc123"
        ));
        assert!(prompt.contains("de.app_ref: nerdsane/agent-answers@abc123"));
        assert!(prompt.contains("de.runtime_tenant: de-variant-1"));
    }

    #[test]
    fn directed_evolution_prompt_marks_empty_resolved_correlation() {
        let work_item = observe_test_work_item("observer", "{}");

        let prompt = directed_evolution_prompt(&work_item);

        assert!(prompt.contains("ResolvedCorrelation:\n(none)"));
    }

    #[test]
    fn headers_with_observe_metadata_attach_kernel_header() {
        let config = observe_test_config("http://127.0.0.1:3497".to_string());
        let join = directed_evolution_join_fields(r#"{"episode_id":"ep-1"}"#);
        let value = directed_evolution_observe_metadata_value("wi-1", "wr-1", "observer", &join);

        let with_header =
            headers_with_observe_metadata(&config, Some(&value)).expect("headers should build");
        let without_header =
            headers_with_observe_metadata(&config, None).expect("headers should build");

        assert_eq!(
            with_header
                .get("x-temper-observe-metadata")
                .and_then(|header| header.to_str().ok()),
            Some(value.as_str())
        );
        assert_eq!(
            with_header
                .get("x-temper-principal-id")
                .and_then(|header| header.to_str().ok()),
            Some("mac-mini-codex-1")
        );
        assert!(!without_header.contains_key("x-temper-observe-metadata"));
    }
}
