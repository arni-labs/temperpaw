use serde_json::Value;
use std::{collections::HashMap, path::Path};

fn load_monitors() -> Vec<Value> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let monitor_path = repo_root.join("dd-monitors/temperpaw-monitors.json");
    let monitors: Value =
        serde_json::from_str(&std::fs::read_to_string(monitor_path).unwrap()).unwrap();
    monitors.as_array().unwrap().clone()
}

fn load_dashboard() -> Value {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dashboard_path = repo_root.join("dd-dashboards/temperpaw-overview.json");
    serde_json::from_str(&std::fs::read_to_string(dashboard_path).unwrap()).unwrap()
}

fn monitors_by_name(monitors: &[Value]) -> HashMap<&str, &Value> {
    monitors
        .iter()
        .map(|monitor| (monitor["name"].as_str().unwrap(), monitor))
        .collect()
}

fn collect_strings<'a>(value: &'a Value, strings: &mut Vec<&'a str>) {
    match value {
        Value::String(value) => strings.push(value),
        Value::Array(values) => {
            for value in values {
                collect_strings(value, strings);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_strings(value, strings);
            }
        }
        _ => {}
    }
}

fn dashboard_group<'a>(dashboard: &'a Value, title: &str) -> &'a Value {
    dashboard["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|widget| {
            widget["definition"]["type"].as_str() == Some("group")
                && widget["definition"]["title"].as_str() == Some(title)
        })
        .unwrap_or_else(|| panic!("{title} dashboard group should exist"))
}

#[test]
fn temperpaw_monitors_use_current_emitted_metrics_instead_of_stale_trace_custom_metrics() {
    let monitors = load_monitors();
    let by_name = monitors_by_name(&monitors);

    let expected = [
        (
            "[TemperPaw] Error Rate Spike",
            "sum(last_15m):default_zero(sum:temperpaw.logs.errors{service:temperpaw}.as_count()) / sum:temper_cedar_evaluations_total{service:temperpaw}.as_count() > 0.1",
        ),
        (
            "[TemperPaw] Request Latency Spike (P95)",
            "avg(last_15m):p95:temper_dispatch_ask_latency_ms{service:temperpaw} > 5000",
        ),
        (
            "[TemperPaw] No Traffic",
            "sum(last_15m):default_zero(sum:temper_cedar_evaluations_total{service:temperpaw}.as_count()) < 1",
        ),
    ];

    for (name, query) in expected {
        let monitor = by_name
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing"));
        assert_eq!(monitor["type"].as_str(), Some("metric alert"));
        assert_eq!(monitor["query"].as_str(), Some(query));
        assert_eq!(monitor["options"]["notify_no_data"].as_bool(), Some(false));
    }

    assert_eq!(
        by_name["[TemperPaw] Error Rate Spike"]["options"]["on_missing_data"].as_str(),
        Some("default"),
        "default_zero metric monitors must use Datadog's compatible missing-data default"
    );
    assert_eq!(
        by_name["[TemperPaw] Request Latency Spike (P95)"]["options"]["on_missing_data"].as_str(),
        Some("resolve"),
        "non-zero-filled latency samples should resolve when traffic is absent"
    );

    for monitor in &monitors {
        let query = monitor["query"].as_str().unwrap_or_default();
        if monitor["type"].as_str() == Some("metric alert") && query.contains("default_zero") {
            assert_ne!(
                monitor["options"]["on_missing_data"].as_str(),
                Some("resolve"),
                "{} uses default_zero and must not set Datadog-incompatible on_missing_data=resolve",
                monitor["name"].as_str().unwrap_or("<unnamed>")
            );
        }
    }

    let all_queries = monitors
        .iter()
        .filter_map(|monitor| monitor["query"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !all_queries.contains("trace.custom"),
        "trace.custom is not emitted for service:temperpaw and must not back monitors"
    );
}

#[test]
fn monitor_queries_use_current_runtime_metric_names_and_zero_fill_sparse_counters() {
    let monitors = load_monitors();
    let by_name = monitors_by_name(&monitors);

    assert_eq!(
        by_name["[Temper] Active Entities Drop"]["query"].as_str(),
        Some("min(last_10m):avg:temper_active_actors{service:temperpaw}.rollup(avg, 60) < 1")
    );

    let sparse_counter_monitors = [
        "[Temper] Required WASM Load Failures",
        "[Temper] Session Memory Budget Exceeded",
        "[Temper] Dispatch Retry Budget Exhausted",
        "[Temper] Permanent Actor Failures",
        "[Temper] Mailbox Saturation (mailbox_full spike)",
        "[Temper] Abnormal State Timeout Firing",
        "[Temper] Spec Liveness Violation",
        "[Temper] Admission Deferred Spike",
        "[Temper] Unexpected Mailbox Drops (post-P4)",
        "[Temper] Profiler Upload Failures",
        "[Temper] Session Memory Externalization Spike",
        "[Temper] Integration Silent Exit (ADR-0056)",
        "[Temper] Turso Write Retry Exhaustion",
        "[TemperPaw] Session Phase Budget Exceeded",
        "[Temper] Query Projection Update Errors",
    ];

    for name in sparse_counter_monitors {
        let monitor = by_name
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing"));
        let query = monitor["query"].as_str().unwrap();
        assert!(
            query.contains("default_zero(sum:"),
            "{name} should zero-fill absent healthy sparse counter data, got {query}"
        );
        assert_eq!(monitor["options"]["notify_no_data"].as_bool(), Some(false));
    }
}

/// A state timer that fires on time is how a spec polls, schedules a step or meets a deadline, so counting firings
/// alerted on healthy polling: a run computer checks every 60 s while Ready, and two or three of them kept
/// "[Temper] Abnormal State Timeout Firing" in Alert on 24 Oct 2026. Abnormal is a timer the kernel finds already
/// past its own deadline, on hydration or on replay, for any entity type: one monitor, grouped by entity_type.
#[test]
fn abnormal_state_timeout_monitor_counts_overdue_timers_not_firings() {
    let monitors = load_monitors();
    let by_name = monitors_by_name(&monitors);
    let query = by_name["[Temper] Abnormal State Timeout Firing"]["query"].as_str().unwrap();
    assert!(!query.contains("temper_state_timeout_fired_total"), "counts firings, so polls trip it: {query}");
    for signal in [
        "temper_state_timeout_armed_on_hydration_total{service:temperpaw,elapsed_bucket:overdue} by {entity_type}",
        "temper_scheduler_overdue_on_replay_total{service:temperpaw} by {entity_type}",
    ] {
        assert!(query.contains(signal), "misses {signal}: {query}");
    }
    assert!(query.ends_with("> 0"), "one overdue timer is abnormal: {query}");
    for name in ["[Temper] Excessive Overdue Timers on Replay", "[Temper] Hydration Re-arm Overdue Spike"] {
        assert!(!by_name.contains_key(name), "{name} watches a signal the abnormal monitor already alerts on");
    }
}

#[test]
fn process_rss_monitor_guards_against_oom_regressions() {
    let monitors = load_monitors();
    let by_name = monitors_by_name(&monitors);
    let monitor = by_name
        .get("[TemperPaw] Process RSS OOM Guard")
        .expect("process RSS OOM guard should be source-controlled");

    assert_eq!(monitor["type"].as_str(), Some("metric alert"));
    assert_eq!(
        monitor["query"].as_str(),
        Some(
            "max(last_10m):max:process_resident_memory_bytes{service:temperpaw,env:prod} by {host,version} > 6500000000"
        )
    );
    assert_eq!(
        monitor["options"]["thresholds"]["critical"].as_u64(),
        Some(6_500_000_000)
    );
    assert_eq!(
        monitor["options"]["thresholds"]["warning"].as_u64(),
        Some(4_500_000_000)
    );
    assert_eq!(monitor["options"]["notify_no_data"].as_bool(), Some(false));
    assert_eq!(
        monitor["options"]["on_missing_data"].as_str(),
        Some("resolve")
    );
}

#[test]
fn platform_dashboard_uses_live_runtime_metrics_instead_of_stale_trace_custom_queries() {
    let dashboard = load_dashboard();
    let dashboard_json = dashboard.to_string();

    assert!(
        !dashboard_json.contains("trace.custom"),
        "trace.custom is not emitted for service:temperpaw and must not back dashboard widgets"
    );
    assert!(
        !dashboard_json.contains("trace.wasm.invoke"),
        "trace.wasm.invoke migration metrics are not deployable/live for service:temperpaw"
    );
    assert!(
        !dashboard_json.contains("temper_active_entities"),
        "temper_active_entities is a retired dashboard query; use temper_active_actors instead"
    );
    assert!(
        dashboard_json.contains(
            "top(sum:temper_cedar_evaluations_total{service:temperpaw} by {decision}.as_count(), 10, 'sum', 'desc')"
        ),
        "Dashboard should replace the stale span-resource toplist with live Cedar runtime traffic"
    );
    assert!(
        dashboard_json.contains("avg:temper_active_actors{service:temperpaw}"),
        "Dashboard should use the live active actor gauge for runtime hydration"
    );
    assert!(
        !dashboard_json.contains("p99:temper_dispatch_ask_latency_ms"),
        "temper_dispatch_ask_latency_ms does not have percentile aggregations enabled in Datadog"
    );
    assert!(
        dashboard_json.contains(
            "avg:temper_dispatch_ask_latency_ms{service:temperpaw} by {entity_type,action}"
        ),
        "Dashboard should replace trace.custom dispatch phase latency with emitted dispatch latency"
    );
    assert!(
        dashboard_json.contains("avg:temper_wasm_invocation_duration_ms{service:temperpaw} by {trigger_action}.rollup(avg, 60)"),
        "Dashboard should use emitted WASM invocation duration instead of trace.wasm.invoke overlays"
    );
}

#[test]
fn platform_dashboard_widgets_do_not_blank_on_known_datadog_query_drift() {
    let dashboard = load_dashboard();
    let dashboard_json = dashboard.to_string();

    assert!(
        !dashboard_json.contains("trace.tool.llm_call"),
        "LLM dashboard widgets must use live LLM Observability metrics, not stale trace.tool.llm_call metrics"
    );
    assert!(
        dashboard_json.contains(
            "default_zero(sum:ml_obs.trace{service:temperpaw,ml_app:temperpaw,span_kind:llm}.as_count().rollup(sum, 60))"
        ),
        "LLM call volume should use the live ml_obs.trace metric and zero-fill quiet windows"
    );
    assert!(
        dashboard_json.contains(
            "default_zero(sum:ml_obs.span.error{service:temperpaw,ml_app:temperpaw}.as_count().rollup(sum, 60))"
        ),
        "LLM error volume should use the live ml_obs.span.error metric and zero-fill quiet windows"
    );

    for query in [
        "default_zero(sum:datadog.trace_agent.otlp.spans{*}.as_count().rollup(sum, 60))",
        "default_zero(sum:datadog.trace_agent.otlp.traces{*}.as_count().rollup(sum, 60))",
        "default_zero(sum:datadog.trace_agent.sampler.kept{*}.as_count().rollup(sum, 60))",
        "default_zero(sum:datadog.trace_agent.sampler.seen{*}.as_count().rollup(sum, 60))",
    ] {
        assert!(
            dashboard_json.contains(query),
            "trace-agent platform widgets should zero-fill inactive windows: {query}"
        );
    }

    for stale_filter in [
        "temper_monty_repl_observed_active_invocations{service:temperpaw}",
        "temper_monty_repl_wait_duration_ms{service:temperpaw}",
        "temper_monty_repl_acquisitions_total{service:temperpaw}",
        "temper_session_large_content_externalized_total{service:temperpaw}",
    ] {
        assert!(
            !dashboard_json.contains(stale_filter),
            "dashboard must not filter untagged historical metrics with service:temperpaw: {stale_filter}"
        );
    }

    for query in [
        "default_zero(max:temper_monty_repl_observed_active_invocations{*}.rollup(max, 60))",
        "default_zero(avg:temper_monty_repl_wait_duration_ms{*} by {max_concurrency}.rollup(avg, 60))",
        "default_zero(sum:temper_monty_repl_acquisitions_total{*} by {max_concurrency}.as_count().rollup(sum, 60))",
        "default_zero(sum:temper_session_large_content_externalized_total{*} by {entity_type}.as_count().rollup(sum, 60))",
    ] {
        assert!(
            dashboard_json.contains(query),
            "dashboard should use validated live metric query: {query}"
        );
    }

    for unavailable_metric in [
        "temper_handler_deadline_remaining_ms",
        "temper_handler_deadline_exceeded_total",
        "temper_wasm_epoch_tick_interval_ms",
        "temper_handler_kill_latency_ms",
    ] {
        assert!(
            !dashboard_json.contains(unavailable_metric),
            "reserved handler-liveness metrics should not appear as blank dashboard widgets until emitted: {unavailable_metric}"
        );
    }
    assert!(
        !dashboard_json.contains("Handler liveness metrics are reserved until temper#147 emits"),
        "reserved handler-liveness coverage should not render as a comment-only dashboard section"
    );
}

#[test]
fn platform_dashboard_avoids_unsupported_percentile_queries() {
    let dashboard = load_dashboard();
    let mut strings = Vec::new();
    collect_strings(&dashboard, &mut strings);

    let configured_percentile_metrics = [
        "temper_admission_permit_hold_time_ms",
        "temper_admission_wait_time_ms",
        "temper_actor_ask_reply_latency_ms",
        "temper_actor_cold_start_duration_ms",
        "temper_actor_registry_lock_wait_ms",
        "temper_blob_io_wait_duration_ms",
        "temper_blob_native_transport_duration_ms",
        "temper_blob_transport_wait_duration_ms",
        "temper_cedar_evaluation_duration",
        "temper_cedar_evaluation_duration_ms",
        "temper_cedar_evaluation_phase_duration_ms",
        "temper_dispatch_ask_attempts",
        "temper_dispatch_ask_latency_ms",
        "temper_event_store_append_wait_ms",
        "temper_monty_repl_wait_duration_ms",
        "temper_postgres_pool_acquire_duration_ms",
        "temper_postgres_transaction_duration_ms",
        "temper_query_projection_backfill_duration_ms",
        "temper_query_projection_backfill_replay_events",
        "temper_query_projection_replay_parity_duration_ms",
        "temper_query_projection_replay_parity_sequence_gap",
        "temper_query_projection_shadow_sequence_gap",
        "temper_query_projection_update_duration_ms",
        "temper_query_projection_update_end_to_end_duration_ms",
        "temper_query_projection_update_queue_wait_ms",
        "temper_session_context_prepare_duration_ms",
        "temper_session_phase_duration_ms",
        "temper_session_phase_step_duration_ms",
        "temper_trajectory_outbox_persist_latency_ms",
        "temper_wasm_host_http_duration_ms",
        "temper_wasm_invocation_duration_ms",
    ];
    let percentile_prefixes = ["p50:", "p75:", "p90:", "p95:", "p99:"];

    for value in strings {
        let Some((prefix, start)) = percentile_prefixes
            .iter()
            .find_map(|prefix| value.find(prefix).map(|idx| (*prefix, idx)))
        else {
            continue;
        };
        let metric_start = start + prefix.len();
        let metric = value[metric_start..]
            .split(|ch: char| ch == '{' || ch == ',' || ch.is_ascii_whitespace())
            .next()
            .unwrap_or_default();
        assert!(
            configured_percentile_metrics.contains(&metric),
            "dashboard percentile query must be backed by scripts/configure_metric_percentiles.py: {value}"
        );
    }
}

#[test]
fn platform_dashboard_groups_are_not_comment_only_sections() {
    let dashboard = load_dashboard();
    let widgets = dashboard["widgets"].as_array().unwrap();
    let note_only_groups = widgets
        .iter()
        .filter_map(|widget| {
            let definition = &widget["definition"];
            if definition["type"].as_str()? != "group" {
                return None;
            }
            let child_widgets = definition["widgets"].as_array()?;
            let non_note_widgets = child_widgets
                .iter()
                .filter(|child| child["definition"]["type"].as_str() != Some("note"))
                .count();
            (non_note_widgets == 0).then(|| definition["title"].as_str().unwrap_or("<untitled>"))
        })
        .collect::<Vec<_>>();

    assert!(
        note_only_groups.is_empty(),
        "dashboard groups should contain real data widgets, not only notes: {note_only_groups:?}"
    );
    assert!(
        widgets.iter().all(|widget| {
            widget["definition"]["title"]
                .as_str()
                .is_none_or(|title| !title.starts_with("Handler Liveness"))
        }),
        "reserved handler-liveness telemetry should be omitted until live metrics exist"
    );
}

#[test]
fn log_oriented_dashboard_sections_have_list_widgets() {
    let dashboard = load_dashboard();

    for (title, expected_query) in [
        (
            "Channel Transports",
            "service:temperpaw @observability_event:temperpaw.transport",
        ),
        (
            "Webhook Triggers",
            "service:temperpaw @observability_event:temperpaw.webhook",
        ),
        (
            "Governance Approvals",
            "service:temperpaw @observability_event:temperpaw.approval",
        ),
    ] {
        let group = dashboard_group(&dashboard, title);
        let list_queries = group["definition"]["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|widget| widget["definition"]["type"].as_str() == Some("list_stream"))
            .flat_map(|widget| {
                widget["definition"]["requests"]
                    .as_array()
                    .into_iter()
                    .flatten()
            })
            .filter_map(|request| {
                let query = &request["query"];
                (query["data_source"].as_str() == Some("logs_stream"))
                    .then(|| query["query_string"].as_str())
                    .flatten()
            })
            .collect::<Vec<_>>();

        assert!(
            list_queries
                .iter()
                .any(|query| query.contains(expected_query)),
            "{title} should include a Datadog logs list widget scoped to {expected_query}, got {list_queries:?}"
        );
    }
}
