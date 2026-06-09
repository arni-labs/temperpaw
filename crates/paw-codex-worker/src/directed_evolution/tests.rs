#[cfg(test)]
mod directed_evolution_tests {
    use super::*;

    #[test]
    fn directed_evolution_work_item_parses_odata_fields() {
        let value = json!({
            "entity_id": "wi-1",
            "status": "Queued",
            "fields": {
                "Role": "observer",
                "TargetEntityType": "Episode",
                "TargetEntityId": "episode-1",
                "PromptRef": "literal:observe this",
                "ContextRef": "ctx-1",
                "OutputSchemaRef": "schema-1",
                "CorrelationJson": "{\"episode_id\":\"episode-1\"}"
            }
        });

        let work_item =
            directed_evolution_work_item_from_odata_value(value).expect("WorkItem should parse");

        assert_eq!(work_item.id, "wi-1");
        assert_eq!(work_item.status, "Queued");
        assert_eq!(work_item.role, "observer");
        assert_eq!(work_item.target_entity_type, "Episode");
        assert_eq!(work_item.target_entity_id, "episode-1");
        assert_eq!(work_item.prompt_ref, "literal:observe this");
        assert_eq!(work_item.context_ref, "ctx-1");
        assert_eq!(work_item.output_schema_ref, "schema-1");
        assert_eq!(work_item.correlation_json, "{\"episode_id\":\"episode-1\"}");
    }

    #[test]
    fn directed_evolution_prompt_uses_literal_prompt_ref() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-1".to_string(),
            status: "Queued".to_string(),
            role: "variant_generator".to_string(),
            target_entity_type: "Generation".to_string(),
            target_entity_id: "gen-1".to_string(),
            prompt_ref: "literal:make three variants".to_string(),
            context_ref: "ctx-1".to_string(),
            output_schema_ref: "schema-1".to_string(),
            correlation_json: "{}".to_string(),
        };

        let prompt = directed_evolution_prompt(&work_item);

        assert!(prompt.contains("Role: variant_generator"));
        assert!(prompt.contains("Generate one bounded candidate variant"));
        assert!(prompt.contains("make three variants"));
        assert!(!prompt.contains("literal:make three variants"));
    }

    #[test]
    fn observer_prompt_requires_datadog_evidence_scope() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-observer".to_string(),
            status: "Queued".to_string(),
            role: "observer".to_string(),
            target_entity_type: "Signal".to_string(),
            target_entity_id: "sig-1".to_string(),
            prompt_ref: "literal:Source: datadog\nSummary: p95 latency climbed".to_string(),
            context_ref: "signal:sig-1".to_string(),
            output_schema_ref: "schema-1".to_string(),
            correlation_json: "{\"source\":\"datadog\"}".to_string(),
        };

        let prompt = directed_evolution_prompt(&work_item);

        assert!(prompt.contains("Datadog MCP"));
        assert!(prompt.contains("evidence_scope"));
        assert!(prompt.contains("datadog_url"));
        assert!(prompt.contains("Do not treat the signal summary as proof"));
    }

    #[test]
    fn evaluation_prompt_treats_required_datadog_as_mandatory() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-review".to_string(),
            status: "Queued".to_string(),
            role: "telemetry_evaluator".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-1".to_string(),
            prompt_ref: "literal:RequiredEvidence: [\"datadog_evidence_scope\"]".to_string(),
            context_ref: "stage-result:sr-1".to_string(),
            output_schema_ref: "schema-1".to_string(),
            correlation_json: "{}".to_string(),
        };

        let prompt = directed_evolution_prompt(&work_item);

        assert!(prompt.contains("observation_metadata"));
        assert!(prompt.contains("temper.observation.de.*"));
        assert!(prompt.contains("de.variant_id"));
        assert!(prompt.contains("provenance_kind=datadog-measured"));
        assert!(prompt.contains("result_count"));
        assert!(prompt.contains("zero_result_meaning"));
    }

    #[test]
    fn directed_evolution_codex_stdout_is_normalized_to_json() {
        let parsed = parse_codex_jsonish("codex\n{\"summary\":\"variant\",\"changed_files\":[\"app.ts\"]}\n")
            .expect("parse JSON from Codex output");

        assert_eq!(parsed["summary"], "variant");
        assert_eq!(parsed["changed_files"], json!(["app.ts"]));
    }

    #[test]
    fn stale_stage_work_cancels_when_stage_result_already_eliminated() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-review".to_string(),
            status: "Queued".to_string(),
            role: "reviewer".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-1".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: "{}".to_string(),
        };

        let reason = stale_directed_evolution_stage_work_reason(
            &work_item,
            &json!({ "Status": "Eliminated", "VariantId": "var-1" }),
            &json!({ "Status": "Active" }),
        )
        .expect("eliminated stage result should cancel");

        assert!(reason.contains("StageResult sr-1 is already Eliminated"));
    }

    #[test]
    fn stale_stage_work_cancels_when_variant_already_eliminated() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-user".to_string(),
            status: "Queued".to_string(),
            role: "viability_evaluator".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-2".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: "{}".to_string(),
        };

        let reason = stale_directed_evolution_stage_work_reason(
            &work_item,
            &json!({ "Status": "Running", "VariantId": "var-2" }),
            &json!({ "Status": "Eliminated" }),
        )
        .expect("eliminated variant should cancel");

        assert!(reason.contains("Target variant is already Eliminated"));
    }

    #[test]
    fn stage_work_continues_for_running_active_variant() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-review".to_string(),
            status: "Queued".to_string(),
            role: "reviewer".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-3".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: "{}".to_string(),
        };

        let reason = stale_directed_evolution_stage_work_reason(
            &work_item,
            &json!({ "Status": "Running", "VariantId": "var-3" }),
            &json!({ "Status": "Active" }),
        );

        assert!(reason.is_none());
    }

    #[test]
    fn stale_stage_work_terminalizes_running_stage_result() {
        assert!(stale_stage_result_should_eliminate(&json!({
            "Status": "Running"
        })));
        assert!(stale_stage_result_should_eliminate(&json!({
            "Status": "Failed"
        })));
        assert!(!stale_stage_result_should_eliminate(&json!({
            "Status": "Eliminated"
        })));
        assert!(!stale_stage_result_should_eliminate(&json!({
            "Status": "Passed"
        })));
    }

    #[test]
    fn stale_stage_work_targets_only_evaluation_stage_results() {
        let mut work_item = DirectedEvolutionWorkItemState {
            id: "wi-review".to_string(),
            status: "Queued".to_string(),
            role: "reviewer".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-3".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: "{}".to_string(),
        };

        assert!(stale_stage_work_targets_stage_result(&work_item));
        work_item.role = "variant_generator".to_string();
        assert!(!stale_stage_work_targets_stage_result(&work_item));
        work_item.role = "simulated_user".to_string();
        work_item.target_entity_type = "Variant".to_string();
        assert!(!stale_stage_work_targets_stage_result(&work_item));
    }

    #[test]
    fn mechanical_state_verifier_passes_bounded_agent_answers_mutation() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-state".to_string(),
            status: "Running".to_string(),
            role: "state_verifier".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-1".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: "{}".to_string(),
        };

        let output = directed_evolution_state_verifier_output(
            &work_item,
            &json!({ "VariantId": "var-1", "EvaluationStageId": "stage-1" }),
            &json!({ "MutationId": "mut-1" }),
            &json!({
                "ChangedFilesJson": "[\"apps/agent-answers/specs/question.ioa.toml\",\"apps/agent-answers/APP.md\"]"
            }),
            &json!({
                "EvaluatorRef": "genesis://nerdsane/agent-answers-evaluation@abc",
                "EvaluatorModule": "state-verifier"
            }),
        );

        assert_eq!(output["passed"], true);
        assert_eq!(output["provenance_kind"], "state-verified");
        assert_eq!(output["metrics"]["viability_regression_count"]["value"], 0);
        assert_eq!(output["metrics"]["mutation_file_count"]["value"], 2);
    }

    #[test]
    fn mechanical_state_verifier_passes_canonical_app_root_mutation() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-state".to_string(),
            status: "Running".to_string(),
            role: "state_verifier".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-root".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: "{}".to_string(),
        };

        let output = directed_evolution_state_verifier_output(
            &work_item,
            &json!({ "VariantId": "var-root", "EvaluationStageId": "stage-root" }),
            &json!({ "MutationId": "mut-root" }),
            &json!({
                "ChangedFilesJson": "[\"APP.md\",\"adrs/0005-question-intent-context.md\",\"policies/agent_answers.cedar\",\"specs/model.csdl.xml\",\"specs/question.ioa.toml\"]"
            }),
            &json!({
                "EvaluatorRef": "genesis://nerdsane/agent-answers-evaluation@abc",
                "EvaluatorModule": "state-verifier"
            }),
        );

        assert_eq!(output["passed"], true);
        assert_eq!(output["metrics"]["viability_regression_count"]["value"], 0);
        assert_eq!(output["metrics"]["mutation_file_count"]["value"], 5);
    }

    #[test]
    fn mechanical_state_verifier_rejects_evaluator_mutation() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-state".to_string(),
            status: "Running".to_string(),
            role: "state_verifier".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-2".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: "{}".to_string(),
        };

        let output = directed_evolution_state_verifier_output(
            &work_item,
            &json!({ "VariantId": "var-2", "EvaluationStageId": "stage-2" }),
            &json!({ "MutationId": "mut-2" }),
            &json!({
                "ChangedFilesJson": "[\"apps/agent-answers-evaluation/wasm/state_verifier/src/lib.rs\"]"
            }),
            &json!({}),
        );

        assert_eq!(output["passed"], false);
        assert_eq!(
            output["metrics"]["evaluator_boundary_violation_count"]["value"],
            1
        );
        assert!(
            output["failure_reason"]
                .as_str()
                .unwrap()
                .contains("pinned evaluator files")
        );
    }

    #[test]
    fn mechanical_evaluator_roles_are_not_codex_brains() {
        assert!(directed_evolution_mechanical_evaluator_role("state_verifier"));
        assert!(directed_evolution_mechanical_evaluator_role("wasm_evaluator"));
        assert_eq!(
            directed_evolution_agent_kind_for_role("state_verifier"),
            "temperpaw-worker"
        );
        assert_eq!(
            directed_evolution_model_for_role("wasm_evaluator"),
            "deterministic-worker"
        );
        assert!(!directed_evolution_mechanical_evaluator_role("simulated_user"));
    }

    #[test]
    fn directed_evolution_repo_mapping_accepts_app_ref_prefix() {
        let previous = env::var_os("DIRECTED_EVOLUTION_ORGANISM_REPOS_JSON");
        unsafe {
            env::set_var(
                "DIRECTED_EVOLUTION_ORGANISM_REPOS_JSON",
                r#"{"arni-labs/agent-answers":"/tmp/agent-answers"}"#,
            );
        }

        let path = directed_evolution_repo_from_mapping(
            "org-agent-answers",
            "arni-labs/agent-answers@abc123",
        )
        .expect("repo should resolve");

        assert_eq!(path, PathBuf::from("/tmp/agent-answers"));
        unsafe {
            if let Some(value) = previous {
                env::set_var("DIRECTED_EVOLUTION_ORGANISM_REPOS_JSON", value);
            } else {
                env::remove_var("DIRECTED_EVOLUTION_ORGANISM_REPOS_JSON");
            }
        }
    }

    #[test]
    fn directed_evolution_app_ref_maps_to_genesis_app_id() {
        let app = directed_evolution_genesis_app_from_ref("nerdsane/agent-answers@abc123")
            .expect("app ref should parse");

        assert_eq!(app.owner, "nerdsane");
        assert_eq!(app.name, "agent-answers");
        assert_eq!(app.hash, "abc123");
        assert_eq!(app.app_id(), "app-nerdsane-agent-answers");
        assert_eq!(app.pinned_ref(), "nerdsane/agent-answers@abc123");
    }

    #[test]
    fn directed_evolution_variant_tenant_is_stable_and_safe() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "en-019e6713-ae4a/unsafe work item".to_string(),
            status: "Queued".to_string(),
            role: "variant_generator".to_string(),
            target_entity_type: "Generation".to_string(),
            target_entity_id: "gen-1".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: String::new(),
        };

        let tenant = directed_evolution_variant_tenant("de-variant", &work_item);

        assert_eq!(tenant, "de-variant-en-019e6713-ae4a-unsafe-work-item");
    }

    #[test]
    fn directed_evolution_variant_push_uses_registry_tenant_header() {
        let app = directed_evolution_genesis_app_from_ref("nerdsane/agent-answers@abc123")
            .expect("app ref should parse");

        let args = directed_evolution_variant_push_args(
            "https://genesis-production-164d.up.railway.app/",
            &app,
            "directed-evolution/wi-1",
            "default",
        );

        assert_eq!(
            args,
            vec![
                "-c",
                "http.extraHeader=x-tenant-id: default",
                "push",
                "https://genesis-production-164d.up.railway.app/nerdsane/agent-answers.git",
                "HEAD:refs/heads/directed-evolution/wi-1",
            ]
        );
    }

    #[test]
    fn directed_evolution_hotloaded_output_overrides_local_runtime_ref() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-variant-1".to_string(),
            status: "Running".to_string(),
            role: "variant_generator".to_string(),
            target_entity_type: "Generation".to_string(),
            target_entity_id: "gen-1".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: String::new(),
        };
        let mut payload = json!({
            "summary": "Adds answer evidence confidence.",
            "runtime_ref": "local-worktree:/tmp/variant"
        });
        let materialization = DirectedEvolutionVariantRuntime {
            app_ref: "nerdsane/agent-answers@abc123".to_string(),
            tenant: "de-variant-wi-variant-1".to_string(),
            runtime_ref: "temper://tenant/de-variant-wi-variant-1/app/nerdsane/agent-answers@abc123"
                .to_string(),
        };

        apply_directed_evolution_variant_runtime(&work_item, &mut payload, &materialization);

        assert_eq!(payload["app_ref"], "nerdsane/agent-answers@abc123");
        assert_eq!(payload["variant_tenant"], "de-variant-wi-variant-1");
        assert_eq!(
            payload["runtime_ref"],
            "temper://tenant/de-variant-wi-variant-1/app/nerdsane/agent-answers@abc123"
        );
        assert_eq!(payload["hot_loaded"], true);
    }

    #[test]
    fn directed_evolution_production_tenant_defaults_to_default() {
        let previous = env::var_os("DIRECTED_EVOLUTION_PRODUCTION_TENANT");
        unsafe {
            env::remove_var("DIRECTED_EVOLUTION_PRODUCTION_TENANT");
        }

        assert_eq!(directed_evolution_production_tenant(), "default");

        unsafe {
            if let Some(value) = previous {
                env::set_var("DIRECTED_EVOLUTION_PRODUCTION_TENANT", value);
            }
        }
    }

    #[test]
    fn directed_evolution_canonical_push_targets_main_with_registry_tenant() {
        let app = directed_evolution_genesis_app_from_ref("nerdsane/agent-answers@abc123")
            .expect("app ref should parse");

        let args = directed_evolution_canonical_push_args(
            "https://genesis-production-164d.up.railway.app/",
            &app,
            "default",
        );

        assert_eq!(
            args,
            vec![
                "-c",
                "http.extraHeader=x-tenant-id: default",
                "push",
                "https://genesis-production-164d.up.railway.app/nerdsane/agent-answers.git",
                "HEAD:refs/heads/main",
            ]
        );
    }

    #[test]
    fn promotion_materialization_output_reports_runtime_ref() {
        let materialization = DirectedEvolutionPromotionMaterialization {
            canonical_app_ref: "nerdsane/agent-answers@abc123".to_string(),
            production_tenant: "default".to_string(),
            runtime_ref: "temper://tenant/default/app/nerdsane/agent-answers@abc123".to_string(),
            summary: "Published and installed winner".to_string(),
            digest: "abc123".to_string(),
        };

        let payload = directed_evolution_promotion_output(&materialization);

        assert_eq!(payload["canonical_app_ref"], "nerdsane/agent-answers@abc123");
        assert_eq!(payload["production_tenant"], "default");
        assert_eq!(
            payload["runtime_ref"],
            "temper://tenant/default/app/nerdsane/agent-answers@abc123"
        );
        assert_eq!(payload["evidence_refs"][0], payload["runtime_ref"]);
    }

    #[test]
    fn directed_evolution_evidence_uri_prefers_agent_reference() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-1".to_string(),
            status: "Queued".to_string(),
            role: "reviewer".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-1".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: String::new(),
        };

        let uri = directed_evolution_evidence_uri(
            &work_item,
            &json!({
                "evidence_refs": ["temper://evidence/stage-result"]
            }),
        );

        assert_eq!(uri, "temper://evidence/stage-result");
    }

    #[test]
    fn directed_evolution_evidence_uri_prefers_datadog_scope_url() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-logs".to_string(),
            status: "Queued".to_string(),
            role: "observer".to_string(),
            target_entity_type: "Signal".to_string(),
            target_entity_id: "sig-1".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: String::new(),
        };

        let uri = directed_evolution_evidence_uri(
            &work_item,
            &json!({
                "evidence_scope": [
                    {
                        "surface": "logs",
                        "query": "service:temperpaw",
                        "result_summary": "no errors",
                        "datadog_url": "https://app.datadoghq.com/logs?query=service%3Atemperpaw"
                    }
                ]
            }),
        );

        assert_eq!(
            uri,
            "https://app.datadoghq.com/logs?query=service%3Atemperpaw"
        );
    }

    #[test]
    fn directed_evolution_evidence_uri_accepts_datadog_site_hosts() {
        assert!(is_datadog_app_url("https://app.datadoghq.com/logs"));
        assert!(is_datadog_app_url("https://app.us3.datadoghq.com/apm/traces"));
        assert!(is_datadog_app_url("https://app.datadoghq.eu/metric/explorer"));
        assert!(is_datadog_app_url("https://app.ap2.datadoghq.com/dashboard"));
        assert!(!is_datadog_app_url("https://example.com/logs"));
    }

    #[test]
    fn directed_evolution_summary_uses_agent_summary_when_available() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-review".to_string(),
            status: "Queued".to_string(),
            role: "reviewer".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-1".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: String::new(),
        };

        let summary = directed_evolution_summary(
            &work_item,
            r#"{"summary":"Variant preserved baseline actions and showed no live OData errors."}"#,
        );

        assert!(summary.contains("reviewer"));
        assert!(summary.contains("Variant preserved baseline actions"));
    }

    #[test]
    fn directed_evolution_datadog_url_encodes_work_item_query() {
        let encoded = encode_url_component("service:temperpaw env:local @work_item_id:wi-1");

        assert_eq!(
            encoded,
            "service%3Atemperpaw%20env%3Alocal%20%40work_item_id%3Awi-1"
        );
    }

    #[test]
    fn directed_evolution_datadog_context_carries_join_fields() {
        let work_item = DirectedEvolutionWorkItemState {
            id: "wi-dd".to_string(),
            status: "Queued".to_string(),
            role: "simulated_user".to_string(),
            target_entity_type: "StageResult".to_string(),
            target_entity_id: "sr-1".to_string(),
            prompt_ref: String::new(),
            context_ref: String::new(),
            output_schema_ref: String::new(),
            correlation_json: String::new(),
        };

        let context = directed_evolution_datadog_context(&work_item);

        assert_eq!(context["work_item_id"], "wi-dd");
        assert_eq!(context["role"], "simulated_user");
        assert_eq!(context["target_entity_type"], "StageResult");
        assert_eq!(context["target_entity_id"], "sr-1");
        assert!(context["query"].as_str().unwrap().contains("@work_item_id:wi-dd"));
    }

    #[test]
    fn human_episode_contract_uses_direction_and_organism_defaults() {
        let input: DirectedEvolutionHumanEpisodeInput = serde_json::from_value(json!({
            "direction_id": "direction-growth",
            "selection_statement": "Prefer variants with clearer answer comparison.",
            "viability_constraints": [
                {"statement": "Answer creation still works.", "kind": "baseline"}
            ]
        }))
        .expect("contract input should parse");

        let plan = directed_evolution_episode_plan_from_input(
            input,
            &json!({
                "OrganismId": "org-agent-answers",
                "AutonomyLane": "growth-human-gated",
                "ProposedAdaptationGoal": "Humans compare candidate answers before acceptance.",
                "ProposedViabilityConstraintsJson": "[\"Do not regress answer acceptance.\"]"
            }),
            &json!({
                "OrganismVersionId": "ov-parent"
            }),
        )
        .expect("episode plan should resolve");

        assert_eq!(plan.direction_id, "direction-growth");
        assert_eq!(plan.organism_id, "org-agent-answers");
        assert_eq!(plan.parent_version_id, "ov-parent");
        assert_eq!(plan.autonomy_lane, "growth-human-gated");
        assert_eq!(
            plan.adaptation_goal,
            "Humans compare candidate answers before acceptance."
        );
        assert_eq!(plan.viability_constraints[0].statement, "Answer creation still works.");
        assert_eq!(plan.metrics.len(), 4);
        assert_eq!(plan.simulated_user_plan.users_per_variant, 3);
        assert_eq!(plan.simulated_user_plan.runs_per_persona, 2);
        assert!(
            plan.evaluation_stages
                .iter()
                .any(|stage| stage.kind == "simulated_user")
        );
    }

    #[test]
    fn human_episode_contract_falls_back_to_direction_constraints() {
        let input: DirectedEvolutionHumanEpisodeInput = serde_json::from_value(json!({
            "DirectionId": "direction-growth",
            "AdaptationGoal": "Improve citation memory."
        }))
        .expect("contract input should parse");

        let plan = directed_evolution_episode_plan_from_input(
            input,
            &json!({
                "OrganismId": "org-agent-answers",
                "ProposedViabilityConstraintsJson": "[\"Keep source fidelity.\",\"Do not increase answer latency.\"]"
            }),
            &json!({
                "ParentVersionId": "ov-parent"
            }),
        )
        .expect("episode plan should resolve");

        assert_eq!(plan.viability_constraints.len(), 2);
        assert_eq!(plan.viability_constraints[0].statement, "Keep source fidelity.");
        assert_eq!(plan.viability_constraints[1].statement, "Do not increase answer latency.");
    }

    #[test]
    fn human_episode_plan_authors_semantic_entities_directly() {
        let input: DirectedEvolutionHumanEpisodeInput = serde_json::from_value(json!({
            "DirectionId": "direction-growth",
            "AdaptationGoal": "Improve answer comparison.",
            "Metrics": [
                {"MetricName": "goal_score", "MetricKind": "goal", "Unit": "score"}
            ],
            "EliminationRules": [
                {"RuleStatement": "Eliminate failures.", "MetricNames": ["goal_score"]}
            ],
            "ScoringRules": [
                {"RuleStatement": "Prefer highest goal score.", "MetricNames": ["goal_score"]}
            ]
        }))
        .expect("contract input should parse");
        let plan = directed_evolution_episode_plan_from_input(
            input,
            &json!({
                "OrganismId": "org-agent-answers",
                "ProposedAdaptationGoal": "Direction goal",
            }),
            &json!({ "ParentVersionId": "ov-parent" }),
        )
        .expect("episode plan should resolve");

        assert_eq!(plan.direction_id, "direction-growth");
        assert_eq!(plan.organism_id, "org-agent-answers");
        assert_eq!(plan.parent_version_id, "ov-parent");
        assert_eq!(plan.metrics[0].name, "goal_score");
        assert_eq!(plan.metrics[0].provenance_kind, "brain-judged");
        assert_eq!(plan.elimination_rules[0].statement, "Eliminate failures.");
        assert_eq!(plan.scoring_rules[0].statement, "Prefer highest goal score.");
        assert_eq!(plan.simulated_user_plan.personas.len(), 3);
        assert!(!plan.evaluator_ref.trim().is_empty());
    }

    #[test]
    fn directed_evolution_start_episode_command_accepts_contract_path() {
        let command = parse_worker_command([
            "directed-evolution-start-episode".to_string(),
            "episode.json".to_string(),
        ]);

        assert_eq!(
            command,
            WorkerCommand::DirectedEvolutionStartEpisode {
                contract_path: Some("episode.json".to_string())
            }
        );
    }
}
