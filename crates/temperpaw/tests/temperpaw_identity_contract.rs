use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

const LEGACY_IDENTITY_TERMS: [&str; 8] = [
    "OPENPAW", "OpenPAW", "OpenPaw", "Open Paw", "openpaw", "open paw", "open_paw", "open-paw",
];

const LEGACY_IDENTITY_ALLOWLIST: [(&str, &str, &str); 28] = [
    (
        "crates/temperpaw/tests/datadog_observability_contract.rs",
        "\"openpaw.\"",
        "test-only assertions for Datadog legacy cleanup paths",
    ),
    (
        "crates/temperpaw/tests/datadog_observability_contract.rs",
        "\"legacy_openpaw_monitor\"",
        "test-only assertions for Datadog legacy cleanup paths",
    ),
    (
        "crates/temperpaw/tests/datadog_observability_contract.rs",
        "\"legacy_openpaw_dashboard\"",
        "test-only assertions for Datadog legacy cleanup paths",
    ),
    (
        "crates/temperpaw/tests/datadog_observability_contract.rs",
        "\"slack-openpaw-alerts\"",
        "test-only assertions for Datadog legacy cleanup paths",
    ),
    (
        "crates/temperpaw/tests/datadog_observability_contract.rs",
        "\"service:openpaw\"",
        "test-only assertions for Datadog legacy cleanup paths",
    ),
    (
        "scripts/deploy_monitors.py",
        "LEGACY_OPENPAW_MONITOR_TERMS",
        "Datadog monitor deploy must find and delete live legacy monitors",
    ),
    (
        "scripts/deploy_monitors.py",
        "\"OpenPaw\"",
        "Datadog monitor deploy must find and delete live legacy monitors",
    ),
    (
        "scripts/deploy_monitors.py",
        "\"OpenPAW\"",
        "Datadog monitor deploy must find and delete live legacy monitors",
    ),
    (
        "scripts/deploy_monitors.py",
        "\"openpaw\"",
        "Datadog monitor deploy must find and delete live legacy monitors",
    ),
    (
        "scripts/deploy_monitors.py",
        "\"service:openpaw\"",
        "Datadog monitor deploy must find and delete live legacy monitors",
    ),
    (
        "scripts/deploy_monitors.py",
        "\"slack-openpaw-alerts\"",
        "Datadog monitor deploy must find and delete live legacy monitors",
    ),
    (
        "scripts/deploy_monitors.py",
        "legacy_openpaw_monitor",
        "Datadog monitor deploy must find and delete live legacy monitors",
    ),
    (
        "scripts/deploy_monitors.py",
        "legacy OpenPaw identity",
        "Datadog monitor deploy must document legacy cleanup matching",
    ),
    (
        "scripts/deploy_pipelines.py",
        "legacy openpaw",
        "Datadog pipeline deploy must delete live legacy log metrics",
    ),
    (
        "scripts/deploy_pipelines.py",
        "LEGACY_LOG_METRIC_PREFIXES",
        "Datadog pipeline deploy must delete live legacy log metrics",
    ),
    (
        "scripts/deploy_dashboard.py",
        "LEGACY_DASHBOARD_TERMS",
        "Datadog dashboard deploy must find and delete live legacy dashboards",
    ),
    (
        "scripts/deploy_dashboard.py",
        "\"OpenPaw\"",
        "Datadog dashboard deploy must find and delete live legacy dashboards",
    ),
    (
        "scripts/deploy_dashboard.py",
        "\"OpenPAW\"",
        "Datadog dashboard deploy must find and delete live legacy dashboards",
    ),
    (
        "scripts/deploy_dashboard.py",
        "\"openpaw\"",
        "Datadog dashboard deploy must find and delete live legacy dashboards",
    ),
    (
        "scripts/deploy_dashboard.py",
        "\"service:openpaw\"",
        "Datadog dashboard deploy must find and delete live legacy dashboards",
    ),
    (
        "scripts/deploy_dashboard.py",
        "\"slack-openpaw-alerts\"",
        "Datadog dashboard deploy must find and delete live legacy dashboards",
    ),
    (
        "scripts/deploy_dashboard.py",
        "legacy_openpaw_dashboard",
        "Datadog dashboard deploy must find and delete live legacy dashboards",
    ),
    (
        "docs/temperpaw-datadog-observability-guide.md",
        "PUBLISHED_BLOB_BUCKET=openpaw-fs-seshendranalla",
        "operator guide documents the current live bucket/domain migration gap",
    ),
    (
        "docs/temperpaw-datadog-observability-guide.md",
        "service:openpaw OR OpenPAW OR OpenPaw",
        "operator guide records the legacy-query proof used to verify cleanup",
    ),
    (
        "docs/temperpaw-legacy-identity-allowlist.md",
        "Railway project slug: `openpaw-seshendranalla`",
        "external Railway project name is intentionally allowlisted until a planned cutover",
    ),
    (
        "docs/temperpaw-legacy-identity-allowlist.md",
        "Railway service name: `openpaw`",
        "external Railway service name is intentionally allowlisted until a planned cutover",
    ),
    (
        "docs/temperpaw-legacy-identity-allowlist.md",
        "Railway generated domain: `openpaw-production.up.railway.app`",
        "external Railway generated domain is intentionally allowlisted until a planned cutover",
    ),
    (
        "docs/temperpaw-legacy-identity-allowlist.md",
        "R2 bucket: `openpaw-fs-seshendranalla`",
        "external R2 bucket name is intentionally allowlisted until a planned storage cutover",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn is_historical_or_proof(path: &Path) -> bool {
    let path = path.to_string_lossy();
    path.starts_with("docs/adrs/")
        || path.contains("/adrs/")
        || path.ends_with("crates/temperpaw/tests/temperpaw_identity_contract.rs")
        || path.ends_with("docs/temperpaw-identity-and-observability-success-contract.md")
}

fn is_generated_or_build_artifact(path: &Path) -> bool {
    path.components().any(|component| {
        let value = component.as_os_str();
        value == OsStr::new("target") || value == OsStr::new("node_modules")
    }) || path
        .file_name()
        .is_some_and(|name| name == OsStr::new("Cargo.lock"))
}

fn is_text_candidate(path: &Path) -> bool {
    matches!(
        path.extension().and_then(OsStr::to_str),
        Some(
            "cedar"
                | "env"
                | "example"
                | "json"
                | "md"
                | "py"
                | "rs"
                | "sh"
                | "svelte"
                | "toml"
                | "ts"
                | "txt"
                | "yaml"
                | "yml"
        )
    ) || path.file_name().is_some_and(|name| {
        matches!(
            name.to_str(),
            Some("Dockerfile" | "Makefile" | "AGENTS.md" | "README.md")
        )
    })
}

fn is_allowlisted_legacy_reference(path: &Path, line: &str) -> bool {
    let path = path.to_string_lossy();
    LEGACY_IDENTITY_ALLOWLIST
        .iter()
        .any(|(allowed_path, allowed_line, _reason)| {
            path.as_ref() == *allowed_path
                && (allowed_line.is_empty() || line.contains(allowed_line))
        })
}

#[test]
fn legacy_external_resource_allowlist_documents_live_runtime_residue() {
    let allowlist_path = repo_root().join("docs/temperpaw-legacy-identity-allowlist.md");
    let allowlist = fs::read_to_string(&allowlist_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", allowlist_path.display()));

    for required in [
        "Railway project slug: `openpaw-seshendranalla`",
        "Railway service name: `openpaw`",
        "Railway generated domain: `openpaw-production.up.railway.app`",
        "R2 bucket: `openpaw-fs-seshendranalla`",
        "Observed product identity must remain `service:temperpaw`",
        "Do not create new resources with legacy names.",
        "Migration requires a planned cutover window",
    ] {
        assert!(
            allowlist.contains(required),
            "legacy external-resource allowlist must document `{required}`"
        );
    }
}

fn collect_files(root: &Path, relative_dir: &Path, files: &mut Vec<PathBuf>) {
    let dir = root.join(relative_dir);
    let entries =
        fs::read_dir(&dir).unwrap_or_else(|err| panic!("failed to read {}: {err}", dir.display()));

    for entry in entries {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read dir entry: {err}"));
        let relative_path = relative_dir.join(entry.file_name());
        let file_type = entry
            .file_type()
            .unwrap_or_else(|err| panic!("failed to stat {}: {err}", relative_path.display()));

        if is_generated_or_build_artifact(&relative_path) || is_historical_or_proof(&relative_path)
        {
            continue;
        }

        if file_type.is_dir() {
            collect_files(root, &relative_path, files);
        } else if file_type.is_file() && is_text_candidate(&relative_path) {
            files.push(relative_path);
        }
    }
}

#[test]
fn active_surfaces_do_not_use_legacy_openpaw_identity() {
    let root = repo_root();
    let mut files = Vec::new();

    for dir in [
        Path::new(".github"),
        Path::new("crates"),
        Path::new("dashboard"),
        Path::new("dd-dashboards"),
        Path::new("dd-log-metrics"),
        Path::new("dd-monitors"),
        Path::new("dd-pipelines"),
        Path::new("docs"),
        Path::new("os-apps"),
        Path::new("scripts"),
    ] {
        collect_files(&root, dir, &mut files);
    }

    for file in [
        Path::new(".env.example"),
        Path::new("DEPLOYMENT.md"),
        Path::new("Dockerfile"),
        Path::new("README.md"),
        Path::new("railway.toml"),
    ] {
        files.push(file.to_path_buf());
    }

    let mut failures = Vec::new();
    for relative_path in files {
        let path = root.join(&relative_path);
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", relative_path.display()));
        for (line_idx, line) in content.lines().enumerate() {
            if LEGACY_IDENTITY_TERMS.iter().any(|term| line.contains(term))
                && !is_allowlisted_legacy_reference(&relative_path, line)
            {
                failures.push(format!(
                    "{}:{}: {}",
                    relative_path.display(),
                    line_idx + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "active TemperPaw surfaces still contain legacy OpenPAW identity:\n{}",
        failures.join("\n")
    );
}

#[test]
fn dockerignore_excludes_local_runtime_state_from_production_images() {
    let dockerignore = fs::read_to_string(repo_root().join(".dockerignore"))
        .expect(".dockerignore should be readable");

    for required in [
        ".git",
        "target",
        "**/target",
        "dashboard/node_modules",
        "**/node_modules",
        ".env",
        ".wrangler",
    ] {
        assert!(
            dockerignore.lines().any(|line| line.trim() == required),
            ".dockerignore must exclude `{required}` so production image contexts do not include local state or proof artifacts"
        );
    }
}

#[test]
fn temperpaw_runtime_uses_bounded_large_stack_workers_for_wasm_loopback_io() {
    let main_rs =
        fs::read_to_string(repo_root().join("crates/temperpaw/src/main.rs")).expect("read main.rs");

    assert!(
        main_rs.contains("TOKIO_WORKER_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024"),
        "TemperPaw must keep Tokio worker stack size explicit and bounded for WASM loopback OData requests"
    );
    assert!(
        main_rs.contains(".thread_stack_size(TOKIO_WORKER_THREAD_STACK_BYTES)"),
        "TemperPaw must build the Tokio runtime with the bounded worker stack instead of using the default macro runtime"
    );
}

#[test]
fn dockerfile_prunes_wasm_build_outputs_before_runtime_copy() {
    let dockerfile =
        fs::read_to_string(repo_root().join("Dockerfile")).expect("Dockerfile should be readable");

    let wasm_build_idx = dockerfile
        .find("cd /app/os-apps/paw-patrol/wasm && bash build.sh")
        .expect("Dockerfile should build Paw runtime WASM modules before pruning");
    let prune_idx = dockerfile
        .find("RUN find os-apps -type d -name target -prune -exec rm -rf {} +")
        .expect(
            "Dockerfile should prune nested WASM target directories before the runtime image copy",
        );
    let runtime_copy_idx = dockerfile
        .find("COPY --from=rust-build /app/os-apps ./os-apps")
        .expect("Dockerfile should copy os-apps into the runtime image");

    assert!(
        wasm_build_idx < prune_idx && prune_idx < runtime_copy_idx,
        "Dockerfile must remove nested WASM target directories after building modules and before copying os-apps into the runtime image"
    );

    for forbidden in [
        "github.com/arni-labs/katagami.git",
        "KATAGAMI_REF",
        "/tmp/katagami",
        "os-apps/katagami-curation/wasm",
        "os-apps/katagami-commons",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "Dockerfile must not bake Katagami from GitHub or local os-app folders; found `{forbidden}`"
        );
    }
}

#[test]
fn app_required_wasm_build_scripts_publish_module_local_artifacts() {
    let root = repo_root();

    let invokes_aggregate = |source: &str, docker: bool| {
        source.lines().any(|line| {
            let line = line.trim().trim_end_matches('\\').trim_end();
            if docker {
                let command = line
                    .strip_prefix("&& ")
                    .or_else(|| line.strip_prefix("RUN "));
                command.is_some_and(|command| {
                    command.split_whitespace().eq([
                        "cd",
                        "/app/os-apps/paw-fs/wasm",
                        "&&",
                        "bash",
                        "build.sh",
                    ])
                })
            } else {
                line == "os-apps/paw-fs/wasm/build.sh"
            }
        })
    };
    for (path, docker) in [("Dockerfile", true), (".github/workflows/ci.yml", false)] {
        let source = fs::read_to_string(root.join(path)).unwrap();
        assert!(
            invokes_aggregate(&source, docker),
            "{path} must invoke the paw-fs aggregate builder"
        );
        for replacement in ["os-apps/paw-fs/wasm/blob_adapter", "# removed aggregate"] {
            let mutant = source.replace("os-apps/paw-fs/wasm", replacement);
            assert!(
                !invokes_aggregate(&mutant, docker),
                "{path} caller mutation escaped"
            );
        }
    }
    let manifest: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("os-apps/paw-fs/app.toml")).unwrap()).unwrap();
    let modules = manifest["wasm_modules"].as_array().unwrap();
    assert!(!modules.is_empty());
    for entry in modules {
        if entry["criticality"].as_str() != Some("app-required") {
            continue;
        }
        let module = entry["name"].as_str().unwrap();
        let script_path = format!("os-apps/paw-fs/wasm/{module}/build.sh");
        let script = fs::read_to_string(root.join(&script_path))
            .unwrap_or_else(|err| panic!("failed to read {script_path}: {err}"));
        assert!(
            script.contains(&format!("{module}.wasm")),
            "{script_path} must publish the compiled {module}.wasm artifact"
        );
        assert!(
            script.contains(&format!("\"$SCRIPT_DIR/{module}.wasm\""))
                || script.contains(&format!("\"$(dirname \"$0\")/{module}.wasm\"")),
            "{script_path} must copy {module}.wasm into the module directory so production target pruning does not remove the only discoverable artifact"
        );
    }
}

#[test]
fn os_app_wasm_build_scripts_preserve_temper_host_imports() {
    let root = repo_root();
    let build_env = fs::read_to_string(root.join("os-apps/wasm-build-env.sh"))
        .expect("shared os-app WASM build environment should be readable");
    assert!(
        build_env.contains("link-arg=--allow-undefined"),
        "Temper WASM guest builds must preserve unresolved host functions as imports"
    );

    for script_path in [
        "os-apps/paw-agent/wasm/build.sh",
        "os-apps/paw-channels/wasm/build.sh",
        "os-apps/paw-fs/wasm/artifact_batch_apply/build.sh",
        "os-apps/paw-fs/wasm/blob_adapter/build.sh",
        "os-apps/paw-fs/wasm/workspace_fs/build.sh",
        "os-apps/paw-foresight/wasm/build.sh",
        "os-apps/paw-ingest/wasm/build.sh",
        "os-apps/paw-managed-agents/wasm/build.sh",
        "os-apps/paw-media/wasm/build.sh",
        "os-apps/paw-patrol/wasm/build.sh",
        "os-apps/paw-research/wasm/build.sh",
        "os-apps/paw-skills/wasm/build.sh",
    ] {
        let script = fs::read_to_string(root.join(script_path))
            .unwrap_or_else(|err| panic!("failed to read {script_path}: {err}"));
        assert!(
            script.contains("wasm-build-env.sh"),
            "{script_path} must source os-apps/wasm-build-env.sh so Temper host imports link in CI and deployment builds"
        );
    }
}

#[test]
fn railway_deploy_dockerfile_uses_image_tag_variable() {
    let deploy_dockerfile = fs::read_to_string(repo_root().join("Dockerfile.deploy"))
        .expect("Dockerfile.deploy should be readable");
    let railway_config = fs::read_to_string(repo_root().join("railway.toml"))
        .expect("railway.toml should be readable");

    assert!(
        deploy_dockerfile.contains("ARG IMAGE_TAG="),
        "Dockerfile.deploy must declare IMAGE_TAG so Railway deployments can select the exact verified GHCR image"
    );
    assert!(
        deploy_dockerfile.contains("FROM ghcr.io/nerdsane/temperpaw:${IMAGE_TAG}"),
        "Dockerfile.deploy must pull ghcr.io/nerdsane/temperpaw using IMAGE_TAG instead of a hard-coded tag"
    );
    assert!(
        !deploy_dockerfile.contains("ghcr.io/nerdsane/temperpaw:edge"),
        "Dockerfile.deploy must not hard-code edge because production proofs require a pinned image tag"
    );
    assert!(
        railway_config.contains("builder = \"DOCKERFILE\""),
        "railway.toml must use Railway's explicit DOCKERFILE builder instead of Railpack"
    );
    assert!(
        railway_config.contains("dockerfilePath = \"Dockerfile.deploy\""),
        "railway.toml must upload Dockerfile.deploy as the production deployment source"
    );
    assert!(
        railway_config.contains("healthcheckPath = \"/healthz\""),
        "Railway cutover must use process liveness; /readyz remains the stronger post-cutover readiness proof"
    );

    let deploy_rs = fs::read_to_string(repo_root().join("crates/temperpaw/src/deploy.rs"))
        .expect("deploy.rs should be readable");
    assert!(
        deploy_rs.contains("healthcheckPath = \\\"/healthz\\\""),
        "generated Railway manifests must also use /healthz for process liveness"
    );
    assert!(
        !deploy_rs.contains("healthcheckPath = \\\"/readyz\\\""),
        "generated Railway manifests must not cut over on /readyz"
    );
}

#[test]
fn railway_redeploy_uses_current_deployment_api() {
    let setup_api = fs::read_to_string(repo_root().join("crates/temperpaw/src/setup_api.rs"))
        .expect("setup_api.rs should be readable");

    assert!(
        setup_api.contains("deploymentRedeploy(id: $deploymentId)"),
        "setup API redeploy must use Railway's deploymentRedeploy mutation with variables"
    );
    assert!(
        setup_api.contains("skipDeploys"),
        "setup API must upsert IMAGE_TAG without triggering an extra variable-change deployment"
    );
    for required in [
        "build_sha",
        "BUILD_SHA",
        "BUILD_VERSION",
        "DD_VERSION",
        "OTEL_RESOURCE_ATTRIBUTES",
        "datadog_app_otel_resource_attributes(build_version)",
        "deployment_runtime_vars",
    ] {
        assert!(
            setup_api.contains(required),
            "setup API redeploy must keep Railway runtime version variables aligned with the selected image: {required}"
        );
    }
    assert!(
        !setup_api.contains("serviceInstanceRedeploy"),
        "setup API must not use Railway's removed serviceInstanceRedeploy mutation"
    );
}

#[test]
fn manual_railway_redeploy_workflow_is_secret_backed_and_version_proven() {
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/railway-redeploy.yml"))
        .expect("manual Railway redeploy workflow should be readable");

    for required in [
        "workflow_dispatch",
        "environment: production",
        "RAILWAY_TOKEN",
        "RAILWAY_PROJECT_ID",
        "RAILWAY_ENVIRONMENT_ID",
        "RAILWAY_SERVICE_ID",
        "TEMPERPAW_BASE_URL",
        "VariableUpsertInput",
        "skipDeploys: true",
        // The deploy sets the service's image and asserts the running one.
        // deploymentRedeploy replays the previous image and is exactly the
        // mechanism that left production on a stale tag for two days.
        "serviceInstanceUpdate",
        "source: { image: $image }",
        "serviceInstanceDeployV2",
        "latestDeployment { id status }",
        "\"${status}\" = \"SUCCESS\"",
        // expected_sha is proven by the tag naming that build; the version
        // endpoint is not consulted while it returns 503 (ARN-508).
        "does not match expected_sha",
        "expected_sha cannot be verified for",
        "expected_sha",
        // Build identity is the image's: BUILD_SHA/BUILD_VERSION are baked and
        // the entrypoint derives DD_VERSION and the OTEL attributes. The
        // workflow writes none of them (asserted below).
        "run_artifact_batch_e2e",
        "scripts/production_artifact_batch_e2e.sh",
        "PACKAGED_WASM_PATH",
    ] {
        assert!(
            workflow.contains(required),
            "Railway redeploy workflow must contain {required}"
        );
    }

    assert!(
        workflow.contains("^(edge|latest|sha-[0-9a-f]{7,40})$"),
        "Railway redeploy workflow must restrict deployable tags to an exact shape"
    );
    for shadowing in [
        "upsert_var BUILD_SHA",
        "upsert_var BUILD_VERSION",
        "upsert_var DD_VERSION",
        "upsert_var OTEL_RESOURCE_ATTRIBUTES",
    ] {
        assert!(
            !workflow.contains(shadowing),
            "Railway redeploy workflow must not write build identity ({shadowing}): the image bakes it and the entrypoint derives the rest; a variable would shadow a per-build value"
        );
    }
    assert!(
        !workflow.contains("deploymentRedeploy"),
        "Railway redeploy workflow must not redeploy the previous deployment: that replays its image and ignores the requested tag"
    );
}

#[test]
fn railway_agent_tool_uses_project_scoped_variable_lookup() {
    let railway_tool =
        fs::read_to_string(repo_root().join("os-apps/paw-agent/wasm/monty_repl/src/railway.rs"))
            .expect("railway.rs should be readable");
    let session_spec =
        fs::read_to_string(repo_root().join("os-apps/paw-agent/specs/session.ioa.toml"))
            .expect("session.ioa.toml should be readable");

    assert!(
        railway_tool.contains(
            "query variables($projectId: String!, $environmentId: String!, $serviceId: String)"
        ),
        "railway variables lookup must include Railway's required projectId and environmentId"
    );
    assert!(
        railway_tool.contains("project(id: $projectId)"),
        "railway deployment_status must use a variableized project lookup"
    );
    assert!(
        railway_tool.contains("deploymentRedeploy(id: $deploymentId)"),
        "railway redeploy action must use deploymentRedeploy with a deployment id"
    );
    assert!(
        !railway_tool.contains("variables(serviceId:"),
        "railway variables lookup must not use the old serviceId-only API shape"
    );

    for required_secret in [
        "railway_project_id = \"{secret:railway_project_id}\"",
        "railway_environment_id = \"{secret:railway_environment_id}\"",
        "railway_service_id = \"{secret:railway_service_id}\"",
    ] {
        assert!(
            session_spec.contains(required_secret),
            "Session run_tools config must expose `{required_secret}`"
        );
    }
}

#[test]
fn docker_image_metadata_uses_temperpaw_identity() {
    let workflow_path = repo_root().join(".github/workflows/docker.yml");
    let workflow = fs::read_to_string(&workflow_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", workflow_path.display()));

    assert!(
        workflow.contains("org.opencontainers.image.description=TemperPaw"),
        "Docker OCI description must be pinned to TemperPaw so metadata-action cannot inherit stale repository descriptions"
    );

    assert!(
        workflow.contains("annotations: |\n            org.opencontainers.image.title=TemperPaw\n            org.opencontainers.image.description=TemperPaw - Agent daemon built on Temper platform"),
        "Docker manifest annotations must be pinned to TemperPaw so GHCR package metadata cannot inherit stale repository descriptions"
    );

    assert!(
        workflow.contains("annotations: ${{ steps.meta.outputs.annotations }}"),
        "Docker build-push-action must publish docker/metadata-action annotations"
    );

    assert!(
        workflow.contains("DOCKER_METADATA_ANNOTATIONS_LEVELS: manifest")
            && !workflow.contains("DOCKER_METADATA_ANNOTATIONS_LEVELS: manifest,index"),
        "Docker annotations must target manifest level only because the single-platform build cannot export index annotations"
    );

    assert!(
        !workflow.contains("org.opencontainers.image.description=Open Paw")
            && !workflow.contains("org.opencontainers.image.description=OpenPaw")
            && !workflow.contains("org.opencontainers.image.description=OpenPAW"),
        "Docker OCI description must not carry legacy OpenPAW identity"
    );
}
