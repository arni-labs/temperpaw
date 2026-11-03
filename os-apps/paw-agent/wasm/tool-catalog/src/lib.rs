use std::collections::BTreeSet;

pub const DEFAULT_TOOLS_ENABLED: &str = "temper_create,temper_get,temper_list,temper_action,temper_patch,temper_submit_specs,temper_show_spec,temper_specs,temper_upload_wasm,temper_get_trajectories,temper_get_insights,temper_get_decisions,temper_poll_decision,temper_approve_decision,temper_deny_decision,temper_submit_policy,temper_list_policies,temper_get_policy,temper_update_policy,temper_delete_policy,temper_search_apps,temper_install_app,temper_publish_app,temper_update_app,temper_list_apps,temper_spawn_session,temper_list_sessions,temper_abort_session,temper_steer_session,temper_save_memory,temper_recall_memory,temper_write,temper_write_many,temper_read,temper_ls,temper_grep,temper_glob,temper_edit,temper_rename,temper_search_history,temper_run_coding_agent,temper_get_secret,temper_datadog_query,temper_railway,temper_vercel,temper_web_search,temper_web_fetch,temper_image_generate,read,write,edit,bash";

#[derive(Clone, Copy, Debug)]
pub struct ReplMethodSpec {
    pub object: &'static str,
    pub method: &'static str,
    pub signature: &'static str,
    pub description: &'static str,
    pub token: Option<&'static str>,
}

pub const REPL_METHOD_SPECS: &[ReplMethodSpec] = &[
    ReplMethodSpec {
        object: "sandbox",
        method: "bash",
        signature: "(command)",
        description: "run shell command, returns stdout",
        token: Some("bash"),
    },
    ReplMethodSpec {
        object: "sandbox",
        method: "read",
        signature: "(path, opts=None)",
        description: "read file content; image paths return a sandbox image handle unless opts.inline is true",
        token: Some("read"),
    },
    ReplMethodSpec {
        object: "sandbox",
        method: "write",
        signature: "(path, content)",
        description: "write file",
        token: Some("write"),
    },
    ReplMethodSpec {
        object: "sandbox",
        method: "edit",
        signature: "(path, old, new)",
        description: "search-replace in file",
        token: Some("edit"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "create",
        signature: "(entity_set, fields_dict)",
        description: "create entity, returns dict with entity_id",
        token: Some("temper_create"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "get",
        signature: "(entity_set, entity_id)",
        description: "get entity by id",
        token: Some("temper_get"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "list",
        signature: "(entity_set, filter_str)",
        description: "list entities with OData $filter",
        token: Some("temper_list"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "action",
        signature: "(entity_set, entity_id, action_name, params_dict)",
        description: "dispatch action",
        token: Some("temper_action"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "patch",
        signature: "(entity_set, entity_id, fields_dict)",
        description: "partial update",
        token: Some("temper_patch"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "spawn_session",
        signature: "(task, soul_id=None, model=None, tools=None, workdir=None, sandbox_url=None, max_turns=None, background=False)",
        description: "spawn sub-session",
        token: Some("temper_spawn_session"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "list_sessions",
        signature: "(filter=None, top=50)",
        description: "list recent sessions; add a filter to narrow the results",
        token: Some("temper_list_sessions"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "abort_session",
        signature: "(session_id)",
        description: "cancel session",
        token: Some("temper_abort_session"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "steer_session",
        signature: "(session_id, message)",
        description: "inject message",
        token: Some("temper_steer_session"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "save_memory",
        signature: "(key, content, memory_type='project')",
        description: "persist long-lived memory",
        token: Some("temper_save_memory"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "recall_memory",
        signature: "(query)",
        description: "search persisted memories, returns list",
        token: Some("temper_recall_memory"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "write",
        signature: "(path, content, opts=None) or ({path, content, opts})",
        description: "write file by path; accepts text or sandbox image handles and auto-creates workspace/dirs",
        token: Some("temper_write"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "write_many",
        signature: "(files, opts=None) or ({files, opts})",
        description: "write multiple files through an ArtifactBatch and one WorkspaceUsageBucket delta",
        token: Some("temper_write_many"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "read",
        signature: "(path, opts=None)",
        description: "read file content by path. opts: {offset: int, limit: int} for partial reads (0-indexed line numbers)",
        token: Some("temper_read"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "ls",
        signature: "(path, opts=None)",
        description: "list directory contents (files and subdirectories), returns JSON array",
        token: Some("temper_ls"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "grep",
        signature: "(pattern, path, opts=None)",
        description: "search file contents for pattern, returns matching lines with file paths and line numbers. opts: {case_insensitive: bool, max_results: int}",
        token: Some("temper_grep"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "glob",
        signature: "(pattern, path='/')",
        description: "find files matching name pattern (supports *, **, ?), returns list of matching paths",
        token: Some("temper_glob"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "edit",
        signature: "(path, old_string, new_string, opts=None)",
        description: "replace first occurrence of old_string with new_string in file",
        token: Some("temper_edit"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "rename",
        signature: "(old_path, new_path, opts=None)",
        description: "rename or move a file to a new path",
        token: Some("temper_rename"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "search_history",
        signature: "(pattern)",
        description: "search recent conversation history in this session, including compacted entries",
        token: Some("temper_search_history"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "run_coding_agent",
        signature: "(agent_type, task)",
        description: "spawn coding session",
        token: Some("temper_run_coding_agent"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "submit_specs",
        signature: "(files_dict)",
        description: "load specs into Temper; files_dict must include model.csdl.xml plus one or more *.ioa.toml files (nested paths allowed)",
        token: Some("temper_submit_specs"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "show_spec",
        signature: "(entity_name)",
        description: "inspect entity spec",
        token: Some("temper_show_spec"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "specs",
        signature: "()",
        description: "list available entity specs",
        token: Some("temper_specs"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "install_app",
        signature: "({app_ref, tenant?, registry_url?, registry_tenant?, follow_policy?, reason?})",
        description: "install a pinned Genesis app ref into this Temper instance; default follow_policy is pinned",
        token: Some("temper_install_app"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "search_apps",
        signature: "({query?, owner?, status?, registry_url?, registry_tenant?})",
        description: "search Genesis registry apps",
        token: Some("temper_search_apps"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "publish_app",
        signature: "({path, owner, name, registry_url?, registry_tenant?, message?})",
        description: "publish app bytes to Genesis through Temper.Git.RegisterNewApp/PublishNewVersion, verify latest, and return owner/name@hash",
        token: Some("temper_publish_app"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "update_app",
        signature: "({path, app_ref_or_name, registry_url?, registry_tenant?, message?})",
        description: "push a new Genesis app version through Temper.Git.PublishNewVersion, verify latest, and return owner/name@hash",
        token: Some("temper_update_app"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "list_apps",
        signature: "()",
        description: "list available apps",
        token: Some("temper_list_apps"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "upload_wasm",
        signature: "(module_name, wasm_base64)",
        description: "upload WASM module",
        token: Some("temper_upload_wasm"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "get_secret",
        signature: "(key)",
        description: "read secret from vault (Cedar-gated)",
        token: Some("temper_get_secret"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "switch_provider",
        signature: "(model=None, provider=None)",
        description: "change LLM provider/model mid-session (takes effect on next turn)",
        token: None,
    },
    ReplMethodSpec {
        object: "temper",
        method: "switch_mode",
        signature: "({\"mode\": \"plan\"})",
        description: "switch to plan mode (read-only + Plan entities)",
        token: None,
    },
    ReplMethodSpec {
        object: "temper",
        method: "switch_mode",
        signature: "({\"mode\": \"execute\"})",
        description: "switch to execute mode (full tools)",
        token: None,
    },
    ReplMethodSpec {
        object: "temper",
        method: "done",
        signature: "(result)",
        description: "signal session completion with result",
        token: None,
    },
    ReplMethodSpec {
        object: "temper",
        method: "submit_policy",
        signature: "(policy_id, cedar_text)",
        description: "create Cedar policy (Cedar-gated)",
        token: Some("temper_submit_policy"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "list_policies",
        signature: "()",
        description: "list all Cedar policies",
        token: Some("temper_list_policies"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "get_policy",
        signature: "(policy_id)",
        description: "read a specific Cedar policy",
        token: Some("temper_get_policy"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "update_policy",
        signature: "(policy_id, cedar_text)",
        description: "update Cedar policy (Cedar-gated)",
        token: Some("temper_update_policy"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "delete_policy",
        signature: "(policy_id)",
        description: "delete Cedar policy (Cedar-gated)",
        token: Some("temper_delete_policy"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "get_trajectories",
        signature: "(entity_type, include_actions, limit=10)",
        description: "evolution data",
        token: Some("temper_get_trajectories"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "get_insights",
        signature: "()",
        description: "evolution insights",
        token: Some("temper_get_insights"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "get_decisions",
        signature: "()",
        description: "pending governance decisions",
        token: Some("temper_get_decisions"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "poll_decision",
        signature: "(decision_id)",
        description: "wait for decision",
        token: Some("temper_poll_decision"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "approve_decision",
        signature: "(decision_id, scope_dict)",
        description: "approve governance decision (Cedar-gated)",
        token: Some("temper_approve_decision"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "deny_decision",
        signature: "(decision_id)",
        description: "deny governance decision (Cedar-gated)",
        token: Some("temper_deny_decision"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "get_agent_id",
        signature: "()",
        description: "return the current agent/session identifier",
        token: None,
    },
    ReplMethodSpec {
        object: "temper",
        method: "get_session_id",
        signature: "()",
        description: "return the current session identifier",
        token: None,
    },
    ReplMethodSpec {
        object: "temper",
        method: "datadog_query",
        signature: "(query_kind, monitor_id=None, query=None, from=None, to=None, limit=25, ...)",
        description: "Datadog monitors, metrics, logs, traces, LLMObs, DBM, and profiling",
        token: Some("temper_datadog_query"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "railway",
        signature: "(action, project_id=None, ...)",
        description: "Railway API",
        token: Some("temper_railway"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "vercel",
        signature: "(action, deployment_id=None, ...)",
        description: "Vercel API",
        token: Some("temper_vercel"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "web_search",
        signature: "(query)",
        description: "search the web via Exa, returns list of {title, url, text}",
        token: Some("temper_web_search"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "web_fetch",
        signature: "(url)",
        description: "fetch a URL, returns text content (HTML tags stripped)",
        token: Some("temper_web_fetch"),
    },
    ReplMethodSpec {
        object: "temper",
        method: "image_generate",
        signature: "(prompt, opts=None) or ({prompt, opts})",
        description: "generate an image through the paw-media app. Use this tool for user image requests; gpt-image-2, gpt-image-1, and dall-e-* requests are normalized onto the Codex media backend. Returns PawFS file metadata and an image handle",
        token: Some("temper_image_generate"),
    },
];

pub fn normalize_tool_token(token: &str) -> &str {
    match token {
        "read_entity" => "temper_get",
        "save_memory" => "temper_save_memory",
        "recall_memory" => "temper_recall_memory",
        "spawn_agent" | "spawn_session" => "temper_spawn_session",
        "temper_file_upload" => "temper_write",
        "sandbox_bash" | "sandbox_exec" => "bash",
        "sandbox_read" => "read",
        "sandbox_write" => "write",
        "sandbox_edit" => "edit",
        other => other,
    }
}

pub fn enabled_tool_set(tools_enabled: &str) -> BTreeSet<String> {
    tools_enabled
        .split(',')
        .map(str::trim)
        .filter(|tool| !tool.is_empty())
        .map(normalize_tool_token)
        .map(ToOwned::to_owned)
        .collect()
}

fn method_is_enabled(spec: &ReplMethodSpec, enabled: &BTreeSet<String>) -> bool {
    match spec.token {
        Some(token) => enabled.contains(token),
        None => true,
    }
}

pub fn has_sandbox_surface(enabled: &BTreeSet<String>) -> bool {
    enabled.contains("bash")
        || enabled.contains("read")
        || enabled.contains("write")
        || enabled.contains("edit")
        || enabled.contains("temper_run_coding_agent")
}

pub fn build_method_listing(enabled: &BTreeSet<String>) -> String {
    REPL_METHOD_SPECS
        .iter()
        .filter(|spec| method_is_enabled(spec, enabled))
        .map(|spec| {
            format!(
                "- {}.{}{} -> {}",
                spec.object, spec.method, spec.signature, spec.description
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
