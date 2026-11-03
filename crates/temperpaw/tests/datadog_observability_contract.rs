use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_json(relative_path: &str) -> Value {
    let path = repo_root().join(relative_path);
    serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display())),
    )
    .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()))
}

fn load_text(relative_path: &str) -> String {
    let path = repo_root().join(relative_path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

fn collect_cargo_manifests(root: &Path, relative_dir: &Path, files: &mut Vec<PathBuf>) {
    let dir = root.join(relative_dir);
    let entries =
        fs::read_dir(&dir).unwrap_or_else(|err| panic!("failed to read {}: {err}", dir.display()));

    for entry in entries {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read dir entry: {err}"));
        let relative_path = relative_dir.join(entry.file_name());
        let file_type = entry
            .file_type()
            .unwrap_or_else(|err| panic!("failed to stat {}: {err}", relative_path.display()));

        if file_type.is_dir() {
            let name = entry.file_name();
            if name != "target" && name != "node_modules" {
                collect_cargo_manifests(root, &relative_path, files);
            }
        } else if file_type.is_file()
            && relative_path
                .file_name()
                .is_some_and(|name| name == "Cargo.toml")
        {
            files.push(relative_path);
        }
    }
}

/// The Temper rev the server builds against: temper-platform's pin in crates/temperpaw/Cargo.toml.
fn server_temper_rev(manifest: &str) -> &str {
    manifest
        .lines()
        .find(|line| {
            line.starts_with("temper-platform = ") && line.contains("arni-labs/temper.git")
        })
        .and_then(|line| line.split("rev = \"").nth(1))
        .and_then(|rest| rest.get(..40))
        .expect("crates/temperpaw/Cargo.toml must pin temper-platform to an arni-labs/temper rev")
}

#[test]
fn temper_dependency_pin_uses_budgeted_wasm_host_call_revision() {
    let manifest = load_text("crates/temperpaw/Cargo.toml");
    let lockfile = load_text("Cargo.lock");
    // The kernel pin moves with every bump (temper-pin-bump.yml), so the contract
    // is one arni-labs rev across the server crates, resolved in Cargo.lock, and
    // none of the known-bad revs below; it does not name the current rev.
    let expected_rev = server_temper_rev(&manifest);
    let pre_llmobs_opt_out_rev = "510a0d9bc9517f7819d66849446cdf6aff2d5295";
    let observe_wait_only_rev = "6ccc483af87abbf6d9b060d0e6a6def3adfe6718";
    let host_boundary_rev = "7b170cf71246e01c337e81062b54ea8c597b9293";
    let parent_only_rev = "4fbfcb971c7c9513ad6605cb8376a8c492c21482";
    let parentless_rev = "ffa0a15212966dbada3db8da6e652f081e5f261b";
    let legacy_rev = "5a19c5f4406e95533896a860b5da15a7a68a70ee";
    // Superseded by the JCS OTS schema merge: rolling back to it would leave the
    // emitter writing contract fields the structs no longer model, and the OTS
    // round-trip test would fail to compile rather than at runtime.
    let pre_jcs_schema_rev = "804633e2c5cab3b0bd334f78bfb5ea23aca1858d";

    for temper_crate in [
        "temper-platform",
        "temper-observe",
        "temper-server",
        "temper-runtime",
        "temper-jit",
        "temper-authz",
        "temper-store-postgres",
        "temper-store-turso",
    ] {
        let manifest_clause = format!(
            "{temper_crate} = {{ git = \"https://github.com/arni-labs/temper.git\", rev = \"{expected_rev}\""
        );
        assert!(
            manifest.contains(&manifest_clause),
            "{temper_crate} must pin the Temper rev with budgeted WASM host-call deadlines, event-driven observe wait, native data-only create storage, lazy WASM secret authorization, WASM dispatch envelope attribution, data-only create fast path support, projection read parity, local TData tenant propagation, runtime-derived LLMObs service identity, parent stitching, agent/workflow hierarchy, DBM attribution, profiling envelope, Datadog-visible WASM span hints, host-boundary spans, guest progress/log correlation, and wasm.invoke phase tracing"
        );
    }

    assert!(
        !manifest.contains(observe_wait_only_rev)
            && !lockfile.contains(observe_wait_only_rev)
            && !manifest.contains(legacy_rev)
            && !lockfile.contains(legacy_rev)
            && !manifest.contains(parent_only_rev)
            && !lockfile.contains(parent_only_rev)
            && !manifest.contains(parentless_rev)
            && !lockfile.contains(parentless_rev)
            && !manifest.contains(host_boundary_rev)
            && !lockfile.contains(host_boundary_rev)
            && !manifest.contains(pre_jcs_schema_rev)
            && !lockfile.contains(pre_jcs_schema_rev),
        "TemperPaw must not pin Temper revs without budgeted WASM host-call deadlines, complete WASM host-boundary observability, hard-coded LLMObs identity, parentless direct LLMObs spans, one-span LLMObs traces, or the pre-JCS OTS schema"
    );
    assert!(
        !manifest.contains(pre_llmobs_opt_out_rev) && !lockfile.contains(pre_llmobs_opt_out_rev),
        "TemperPaw must not pin the pre-LLMObs-auto-conversion opt-out Temper rev"
    );
    assert!(
        lockfile.contains(expected_rev),
        "Cargo.lock must resolve Temper dependencies to the budgeted WASM host-call deadline revision"
    );
}

#[test]
fn wasm_sdk_dependencies_pin_same_temper_runtime_revision_as_server() {
    let root = repo_root();
    // The server's own pin, read from its manifest, so the two cannot drift apart while this passes.
    let server_manifest = load_text("crates/temperpaw/Cargo.toml");
    let expected_rev = server_temper_rev(&server_manifest);
    let expected_lock_source = format!(
        "source = \"git+https://github.com/arni-labs/temper.git?rev={expected_rev}#{expected_rev}\""
    );
    let expected_dependency = format!(
        "temper-wasm-sdk = {{ git = \"https://github.com/arni-labs/temper.git\", rev = \"{expected_rev}\""
    );
    let forbidden_dependency =
        "temper-wasm-sdk = { git = \"https://github.com/arni-labs/temper.git\", branch = \"main\"";
    let mut manifests = Vec::new();
    collect_cargo_manifests(&root, Path::new("os-apps"), &mut manifests);

    let mut sdk_manifests = 0usize;
    let mut sdk_lockfiles = 0usize;
    for manifest_path in manifests {
        let manifest = fs::read_to_string(root.join(&manifest_path))
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", manifest_path.display()));
        if !manifest.contains("temper-wasm-sdk") {
            continue;
        }
        sdk_manifests += 1;
        assert!(
            manifest.contains(&expected_dependency),
            "{} must pin temper-wasm-sdk to the server's Temper rev {expected_rev} (crates/temperpaw/Cargo.toml) so guest modules do not drift away from WASM host-call and observability contracts",
            manifest_path.display()
        );
        assert!(
            !manifest.contains(forbidden_dependency),
            "{} must not build temper-wasm-sdk from moving main; production images need one coherent SDK/runtime observability contract",
            manifest_path.display()
        );

        let lock_path = manifest_path.with_file_name("Cargo.lock");
        let absolute_lock_path = root.join(&lock_path);
        if absolute_lock_path.exists() {
            let lockfile = fs::read_to_string(&absolute_lock_path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", lock_path.display()));
            if let Some(sdk_source) = lockfile
                .split("[[package]]")
                .find(|package| package.contains("name = \"temper-wasm-sdk\""))
                .and_then(|package| package.lines().find(|line| line.starts_with("source = ")))
            {
                sdk_lockfiles += 1;
                assert_eq!(
                    sdk_source,
                    expected_lock_source,
                    "{} must resolve temper-wasm-sdk to the server's Temper rev",
                    lock_path.display()
                );
                assert!(
                    !lockfile.contains("?branch=main#"),
                    "{} must not keep a moving-main temper-wasm-sdk lock source",
                    lock_path.display()
                );
            }
        }
    }

    assert!(
        sdk_manifests > 20,
        "contract must inspect the packaged WASM module dependency surface"
    );
    assert!(
        sdk_lockfiles > 20,
        "contract must inspect checked-in WASM Cargo.lock resolution"
    );
}

#[test]
fn dockerfile_does_not_clone_or_rewrite_katagami_apps() {
    let dockerfile = load_text("Dockerfile");

    for forbidden in [
        "github.com/arni-labs/katagami.git",
        "KATAGAMI_REF",
        "TEMPER_OBSERVABILITY_REV",
        "/tmp/katagami",
        "os-apps/katagami-curation/wasm",
        "os-apps/katagami-commons",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "Dockerfile must install Katagami from Genesis refs, not clone or rewrite Katagami with `{forbidden}`"
        );
    }
}

#[test]
fn agent_operating_guidance_teaches_complete_datadog_diagnostics() {
    let sre_manual = load_text("os-apps/paw-agent/agents/sre/AGENT.md");
    let paw_skill = load_text("os-apps/paw-agent/agents/paw/skills/temperpaw-agent/SKILL.md");
    let combined = format!("{sre_manual}\n{paw_skill}");

    for required in [
        "temperpaw.agent.session",
        "managed_session_id",
        "inner_session_id",
        "dd.trace_id",
        "dd.span_id",
        "LLM Observability",
        "gen_ai.operation.name",
        "Postgres DBM",
        "profiling",
        "Database Monitoring",
        "get_llmobs_agent_loop",
        "wasm_module",
        "workflow_step",
        "wasm_guest.progress",
        "wasm.host.get_secret",
        "non-redundant",
        "chronological",
    ] {
        assert!(
            combined.contains(required),
            "agent operating guidance must teach Datadog diagnostic concept `{required}`"
        );
    }
}

#[test]
fn otel_collector_keeps_otlp_in_apm_and_avoids_noisy_llmobs_forwarding() {
    let collector =
        std::fs::read_to_string(repo_root().join("scripts/otel-collector-datadog.yaml"))
            .expect("collector config should be readable");

    for required in [
        "traces/apm",
        "transform/dbm",
        "set(attributes[\"span.type\"], \"sql\") where attributes[\"db.system\"] != nil and attributes[\"span.type\"] == nil",
        "exporters: [clickhouse, datadog]",
        "processors: [resourcedetection, resource, transform/dbm, batch]",
    ] {
        assert!(
            collector.contains(required),
            "collector must preserve Datadog APM routing clause `{required}`"
        );
    }
    for forbidden in [
        "traces/llmobs",
        "otlphttp/llmobs",
        "dd-otlp-source: llmobs",
        "filter/traces_llmobs",
        "filter/traces_apm",
    ] {
        assert!(
            !collector.contains(forbidden),
            "collector must not use generic OTLP trace forwarding for LLMObs clause `{forbidden}`"
        );
    }
}

#[test]
fn sensitive_data_scanner_covers_observability_and_agent_secret_shapes() {
    let scanner = load_json("dd-pipelines/sensitive-data-scanner.json").to_string();
    let deploy_script = std::fs::read_to_string(repo_root().join("scripts/deploy_pipelines.py"))
        .expect("deploy_pipelines.py should be readable");

    for required in [
        "Datadog API/application keys",
        "DD_(?:API|APP|APPLICATION)_KEY",
        "sk-(?:ant-)?",
        "gh[posu]_",
        "xox[pbaors]-",
        "[REDACTED:DATADOG]",
        "[REDACTED:OPENAI]",
        "[REDACTED:GITHUB]",
        "[REDACTED:SLACK]",
    ] {
        assert!(
            scanner.contains(required),
            "Sensitive-data scanner must cover `{required}`"
        );
    }

    assert!(
        deploy_script.contains("deploy_sds"),
        "Datadog pipeline deploy must read the sensitive-data scanner source of truth"
    );
}

/// Reconcile deletes only monitors TemperPaw owns: the team tag or a name from the file. On 24 Oct 2026 a dry run
/// found that matching words in a monitor's text would have deleted seven of Katagami's own monitors, whose
/// runbooks name the Railway service. A Datadog UI save turns "metric alert" into "query alert", and that must
/// not make a deploy delete and recreate the monitor.
#[test]
fn datadog_monitor_deploy_owns_only_tagged_or_file_named_monitors() {
    const PROBE: &str = r#"
import importlib.util, json, sys, types
sys.modules["requests"] = types.SimpleNamespace(Response=object, HTTPError=Exception)
spec = importlib.util.spec_from_file_location("deploy_monitors", sys.argv[1])
deploy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deploy)
monitors = json.loads(sys.argv[2])
print(json.dumps({
    "owned": [deploy.is_temperpaw_owned_monitor(m, {"[Temper] In The File"}) for m in monitors],
    "same_type": [deploy.same_monitor_type(*pair) for pair in json.loads(sys.argv[3])],
}))
"#;
    let monitors = serde_json::json!([
        {
            "name": "[Katagami] Temper is unreachable",
            "message": "Check the Railway service openpaw before the app.",
            "query": "logs(\"service:openpaw status:error\").last(\"5m\") > 0",
            "tags": ["katagami"]
        },
        {"name": "[Temper] Retired Monitor", "message": "", "query": "", "tags": ["team:temperpaw"]},
        {"name": "[Temper] In The File", "message": "", "query": "", "tags": []}
    ]);
    let type_pairs = serde_json::json!([
        ["query alert", "metric alert"],
        ["metric alert", "query alert"],
        ["log alert", "log alert"],
        ["log alert", "metric alert"]
    ]);
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(PROBE)
        .arg(repo_root().join("scripts/deploy_monitors.py"))
        .arg(monitors.to_string())
        .arg(type_pairs.to_string())
        .output()
        .expect("python3 should run deploy_monitors.py");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        answer["owned"],
        serde_json::json!([false, true, true]),
        "only the team tag or a name from the file makes a monitor TemperPaw's"
    );
    assert_eq!(
        answer["same_type"],
        serde_json::json!([true, true, true, false]),
        "metric alert and query alert are one type; other types still differ"
    );
}
