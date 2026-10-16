// ADR-0041 observability metadata: the temper kernel parses the
// X-Temper-Observe-Metadata header (a flat JSON object) into
// temper.observation.* span attributes. The worker resolves the Directed
// Evolution join fields from a WorkItem's CorrelationJson exactly once here
// and reuses them for the kernel header, the Datadog context, the prompt
// header block, and the Codex child environment.

const TEMPER_OBSERVE_METADATA_HEADER: &str = "x-temper-observe-metadata";
const TEMPER_OBSERVE_METADATA_MAX_KEYS: usize = 32;
const TEMPER_OBSERVE_METADATA_MAX_KEY_BYTES: usize = 96;
const TEMPER_OBSERVE_METADATA_MAX_VALUE_BYTES: usize = 1_024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DirectedEvolutionJoinFields {
    episode_id: String,
    direction_id: String,
    generation_id: String,
    variant_id: String,
    evaluation_stage_id: String,
    stage_result_id: String,
    trial_id: String,
    simulated_user_plan_id: String,
    persona_index: String,
    run_index: String,
    runtime_ref: String,
    app_ref: String,
    runtime_tenant: String,
    /// Runtime auth preflight inputs (B9). Resolved from the same single
    /// CorrelationJson parse but not part of the de.* join-field entries.
    runtime_base_url: String,
    runtime_auth_env_vars: Vec<String>,
}

impl DirectedEvolutionJoinFields {
    fn entries(&self) -> Vec<(&'static str, &str)> {
        [
            ("episode_id", self.episode_id.as_str()),
            ("direction_id", self.direction_id.as_str()),
            ("generation_id", self.generation_id.as_str()),
            ("variant_id", self.variant_id.as_str()),
            ("evaluation_stage_id", self.evaluation_stage_id.as_str()),
            ("stage_result_id", self.stage_result_id.as_str()),
            ("trial_id", self.trial_id.as_str()),
            (
                "simulated_user_plan_id",
                self.simulated_user_plan_id.as_str(),
            ),
            ("persona_index", self.persona_index.as_str()),
            ("run_index", self.run_index.as_str()),
            ("runtime_ref", self.runtime_ref.as_str()),
            ("app_ref", self.app_ref.as_str()),
            ("runtime_tenant", self.runtime_tenant.as_str()),
        ]
        .into_iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .collect()
    }
}

fn directed_evolution_join_fields(correlation_json: &str) -> DirectedEvolutionJoinFields {
    let correlation =
        serde_json::from_str::<Value>(correlation_json).unwrap_or_else(|_| json!({}));
    let field = |key: &str| {
        directed_evolution_correlation_string(&correlation, key).unwrap_or_default()
    };
    DirectedEvolutionJoinFields {
        episode_id: field("episode_id"),
        direction_id: field("direction_id"),
        generation_id: field("generation_id"),
        variant_id: field("variant_id"),
        evaluation_stage_id: field("evaluation_stage_id"),
        stage_result_id: field("stage_result_id"),
        trial_id: field("trial_id"),
        simulated_user_plan_id: field("simulated_user_plan_id"),
        persona_index: field("persona_index"),
        run_index: field("run_index"),
        runtime_ref: field("runtime_ref"),
        app_ref: field("app_ref"),
        runtime_tenant: field("runtime_tenant"),
        runtime_base_url: field("runtime_base_url"),
        runtime_auth_env_vars: observer_string_array(&correlation, "runtime_auth_env_vars"),
    }
}

fn directed_evolution_observe_metadata_value(
    work_item_id: &str,
    worker_run_id: &str,
    role: &str,
    join: &DirectedEvolutionJoinFields,
) -> String {
    let mut pairs: Vec<(String, &str)> = vec![
        ("producer.work_item_id".to_string(), work_item_id),
        ("producer.worker_run_id".to_string(), worker_run_id),
        ("de.role".to_string(), role),
    ];
    for (key, value) in join.entries() {
        pairs.push((format!("de.{key}"), value));
    }

    let mut object = serde_json::Map::new();
    for (key, value) in pairs {
        if object.len() >= TEMPER_OBSERVE_METADATA_MAX_KEYS {
            break;
        }
        let value = value.trim();
        if value.is_empty() || key.len() > TEMPER_OBSERVE_METADATA_MAX_KEY_BYTES {
            continue;
        }
        object.insert(
            key,
            json!(truncate_utf8_bytes(
                value,
                TEMPER_OBSERVE_METADATA_MAX_VALUE_BYTES
            )),
        );
    }
    Value::Object(object).to_string()
}

fn directed_evolution_work_item_observe_metadata(
    work_item: &DirectedEvolutionWorkItemState,
    worker_run_id: &str,
    join: &DirectedEvolutionJoinFields,
) -> String {
    directed_evolution_observe_metadata_value(&work_item.id, worker_run_id, &work_item.role, join)
}

fn directed_evolution_dd_tags(
    existing: Option<&str>,
    role: &str,
    join: &DirectedEvolutionJoinFields,
) -> String {
    let mut tags: Vec<String> = existing
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !role.trim().is_empty() {
        tags.push(format!("de.role:{}", sanitize_datadog_tag_value(role)));
    }
    for (key, value) in join.entries() {
        tags.push(format!("de.{key}:{}", sanitize_datadog_tag_value(value)));
    }
    tags.join(",")
}

fn sanitize_datadog_tag_value(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|ch| if ch.is_whitespace() || ch == ',' { '_' } else { ch })
        .collect()
}

fn directed_evolution_codex_child_env(
    work_item: &DirectedEvolutionWorkItemState,
    worker_run_id: &str,
    join: &DirectedEvolutionJoinFields,
    existing_dd_tags: Option<&str>,
) -> Vec<(String, String)> {
    vec![
        (
            "TEMPER_OBSERVE_METADATA".to_string(),
            directed_evolution_work_item_observe_metadata(work_item, worker_run_id, join),
        ),
        (
            "DD_TAGS".to_string(),
            directed_evolution_dd_tags(existing_dd_tags, &work_item.role, join),
        ),
    ]
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}
