//! Temper Paw 9-phase startup sequence.
//!
//! Replicates the Temper CLI's boot flow (`temper serve`) in an embedded context.
//! The daemon boots the Temper platform, installs Paw OS apps, seeds souls,
//! and starts the Discord transport.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::response::IntoResponse;
use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::{KeyValue, global};
use temper_platform::PlatformState;
use temper_platform::genesis_install::{
    GenesisRegistryInstallRequest, install_genesis_app_from_registry,
    restore_genesis_registry_cache_roots,
};
use temper_platform::os_apps::{
    InstallResult, OsAppReconcileResult, get_os_app, list_startup_os_apps, reconcile_os_app,
    resolve_os_app_install_order,
};
use temper_platform::recovery::{
    InstalledAppRuntimeRecoveryOutcome, InstalledAppsRuntimeRecoverySummary,
    recover_cedar_policies, recover_installed_app_runtime_state,
    recover_installed_apps_runtime_state,
};
use temper_platform::router::build_platform_router;
use temper_runtime::scheduler::sim_now;
use temper_runtime::tenant::TenantId;
use temper_server::platform_store::{InstalledAppRecord, PlatformStore, SpecVerificationUpdate};
use temper_server::registry::{EntityLevelSummary, EntityVerificationResult, VerificationStatus};
use temper_server::registry_bootstrap::restore_registry_from_platform_store;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::storage::PawStorage;

const DEFAULT_AGENT_TOOLS_ENABLED: &str = "temper_create,temper_get,temper_list,temper_action,temper_patch,temper_submit_specs,temper_show_spec,temper_specs,temper_upload_wasm,temper_get_trajectories,temper_get_insights,temper_get_decisions,temper_poll_decision,temper_approve_decision,temper_deny_decision,temper_submit_policy,temper_list_policies,temper_get_policy,temper_update_policy,temper_delete_policy,temper_search_apps,temper_install_app,temper_publish_app,temper_update_app,temper_list_apps,temper_spawn_session,temper_list_sessions,temper_abort_session,temper_steer_session,temper_save_memory,temper_recall_memory,temper_write,temper_write_many,temper_read,temper_run_coding_agent,temper_get_secret,temper_datadog_query,temper_railway,temper_vercel,temper_web_search,temper_web_fetch,temper_image_generate,read,write,edit,bash";
const DEFAULT_AGENT_WORKDIR: &str = "/workspace";
const STARTUP_PHASE_DURATION_METRIC: &str = "temper_startup_phase_duration_ms";
const STARTUP_TIME_TO_READY_METRIC: &str = "temper_startup_time_to_healthy_ms";
const STARTUP_LIVE_RESTORE_ENTITIES_METRIC: &str = "temper_startup_live_restore_entities_total";
const OS_APP_RECONCILE_TOTAL_METRIC: &str = "temper_os_app_reconcile_total";
const OS_APP_RECONCILE_DURATION_METRIC: &str = "temper_os_app_reconcile_duration_ms";
const WASM_MODULE_LOAD_FAILURES_METRIC: &str = "temper_wasm_module_load_failures_total";
const DEFAULT_ORPHANED_SESSION_RECOVERY_LIMIT: usize = 25;
const DEFAULT_GENESIS_REGISTRY_URL: &str = "https://genesis-production-164d.up.railway.app";
const DEFAULT_GENESIS_CACHE_RESTORE_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_GENESIS_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(60);

fn running_on_railway() -> bool {
    std::env::var_os("RAILWAY_ENVIRONMENT").is_some()
        || std::env::var_os("RAILWAY_PROJECT_ID").is_some()
        || std::env::var_os("RAILWAY_SERVICE_ID").is_some()
}

struct StartupMetrics {
    phase_duration_ms: Histogram<f64>,
    time_to_ready_ms: Histogram<f64>,
    live_restore_entities_total: Counter<u64>,
    os_app_reconcile_total: Counter<u64>,
    os_app_reconcile_duration_ms: Histogram<f64>,
    wasm_module_load_failures_total: Counter<u64>,
}

fn startup_metrics() -> &'static StartupMetrics {
    static METRICS: std::sync::OnceLock<StartupMetrics> = std::sync::OnceLock::new();
    METRICS.get_or_init(|| {
        let meter = global::meter("temperpaw.startup");
        StartupMetrics {
            phase_duration_ms: meter
                .f64_histogram(STARTUP_PHASE_DURATION_METRIC)
                .with_unit("ms")
                .with_description("TemperPaw startup phase duration.")
                .build(),
            time_to_ready_ms: meter
                .f64_histogram(STARTUP_TIME_TO_READY_METRIC)
                .with_unit("ms")
                .with_description(
                    "TemperPaw process startup duration until the deployment is ready.",
                )
                .build(),
            live_restore_entities_total: meter
                .u64_counter(STARTUP_LIVE_RESTORE_ENTITIES_METRIC)
                .with_description("Entities restored into runtime indexes during startup.")
                .build(),
            os_app_reconcile_total: meter
                .u64_counter(OS_APP_RECONCILE_TOTAL_METRIC)
                .with_description("Startup OS-app reconcile attempts by app and result.")
                .build(),
            os_app_reconcile_duration_ms: meter
                .f64_histogram(OS_APP_RECONCILE_DURATION_METRIC)
                .with_unit("ms")
                .with_description("Startup OS-app reconcile duration by app and result.")
                .build(),
            wasm_module_load_failures_total: meter
                .u64_counter(WASM_MODULE_LOAD_FAILURES_METRIC)
                .with_description("Required WASM module load/build failures during startup.")
                .build(),
        }
    })
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn record_startup_phase_duration(phase: &'static str, duration: Duration) {
    startup_metrics().phase_duration_ms.record(
        duration_ms(duration),
        &[KeyValue::new("phase", phase.to_string())],
    );
}

fn record_startup_time_to_ready(duration: Duration, tenant: &str) {
    startup_metrics().time_to_ready_ms.record(
        duration_ms(duration),
        &[KeyValue::new("tenant", tenant.to_string())],
    );
}

fn record_startup_live_restore_entities(tenant: &str, count: u64) {
    startup_metrics()
        .live_restore_entities_total
        .add(count, &[KeyValue::new("tenant", tenant.to_string())]);
}

fn record_os_app_reconcile(app: &str, result: &'static str, duration: Duration) {
    let attrs = [
        KeyValue::new("app", app.to_string()),
        KeyValue::new("result", result.to_string()),
    ];
    startup_metrics().os_app_reconcile_total.add(1, &attrs);
    startup_metrics()
        .os_app_reconcile_duration_ms
        .record(duration_ms(duration), &attrs);
}

fn record_wasm_module_load_failure(stage: &'static str) {
    startup_metrics()
        .wasm_module_load_failures_total
        .add(1, &[KeyValue::new("stage", stage.to_string())]);
}

fn installed_app_runtime_recovery_result(
    summary: &InstalledAppsRuntimeRecoverySummary,
) -> &'static str {
    if summary.store_error > 0 || summary.missing_bundle > 0 {
        "error"
    } else if summary.needs_reconcile > 0 {
        "needs_reconcile"
    } else if summary.healed > 0 {
        "healed"
    } else {
        "ready"
    }
}

/// What to do with a configured Genesis bootstrap app at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootstrapAction {
    /// Leave the already-installed version in place; do not touch it.
    KeepInstalled,
    /// (Re)install the env-pinned bootstrap ref.
    InstallPinned,
}

/// A record is a candidate to KEEP only if it is a completed Genesis install.
fn is_keep_eligible_genesis_install(record: &InstalledAppRecord) -> bool {
    record.source_kind == "genesis" && record.status == "installed"
}

/// Decide whether to keep an already-installed Genesis bootstrap app or (re)install the
/// env-pinned ref, from the DURABLE RECORD ALONE.
///
/// The env-pinned bootstrap ref (`TEMPERPAW_GENESIS_BOOTSTRAP_REFS`) is a **floor, not a
/// ceiling**: it seeds a cold start but never overrides an installed Genesis version. If a
/// Genesis `installed` record exists (any hash), KEEP it — so a redeploy can never revert a
/// newer agent-published version to the pinned ref. The pin is (re)installed only when there is
/// nothing installed to keep: no record, a non-Genesis record, or a record not in the
/// `installed` status.
///
/// Deliberately record-only, NOT runtime-readiness-gated (see decision log D9). The keep decision
/// runs at boot, while the runtime state it would probe — the process-global app catalog, Cedar
/// policies, WASM registration — is still being restored concurrently, so ANY readiness probe is
/// racy: a transient restore hiccup or another tenant's catalog activity looks like "not ready"
/// and triggers a downgrade to the older pin — the exact bug this change exists to kill. The
/// durable `InstalledAppRecord` is tenant-scoped and race-free, and it is the source of truth for
/// "what is installed". Healing a genuinely-broken install is the recovery path's job (reconcile
/// the INSTALLED version), not the bootstrap's — reverting to an older env pin is a downgrade
/// masquerading as a heal.
fn classify_bootstrap_action(installed: Option<&InstalledAppRecord>) -> BootstrapAction {
    match installed {
        Some(record) if is_keep_eligible_genesis_install(record) => BootstrapAction::KeepInstalled,
        _ => BootstrapAction::InstallPinned,
    }
}

#[cfg(test)]
fn runtime_indexes_required_before_reconcile(
    summary: &InstalledAppsRuntimeRecoverySummary,
) -> bool {
    summary.store_error > 0 || summary.missing_bundle > 0 || summary.needs_reconcile > 0
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StartupSurfaceRuntimeRecoverySummary {
    ready: usize,
    healed: usize,
    cold: usize,
    needs_reconcile: usize,
    missing_bundle: usize,
    store_error: usize,
}

fn startup_surface_runtime_recovery_result(
    summary: &StartupSurfaceRuntimeRecoverySummary,
) -> &'static str {
    if summary.store_error > 0 || summary.missing_bundle > 0 {
        "error"
    } else if summary.needs_reconcile > 0 {
        "needs_reconcile"
    } else if summary.healed > 0 {
        "healed"
    } else if summary.cold > 0 && summary.ready == 0 {
        "cold"
    } else {
        "ready"
    }
}

fn startup_surface_runtime_indexes_required_before_reconcile(
    summary: &StartupSurfaceRuntimeRecoverySummary,
) -> bool {
    summary.store_error > 0 || summary.missing_bundle > 0 || summary.needs_reconcile > 0
}

async fn recover_startup_surface_runtime_state(
    state: &PlatformState,
    ps: &dyn PlatformStore,
    tenant: &str,
    startup_app_order: &[String],
) -> StartupSurfaceRuntimeRecoverySummary {
    let mut summary = StartupSurfaceRuntimeRecoverySummary::default();
    for app_name in startup_app_order {
        match ps.get_installed_app(tenant, app_name).await {
            Ok(None) => {
                summary.cold += 1;
                tracing::info!(
                    tenant,
                    app = %app_name,
                    "Startup OS app has no durable install record; cold reconcile will install it"
                );
                continue;
            }
            Err(error) => {
                summary.store_error += 1;
                tracing::warn!(
                    tenant,
                    app = %app_name,
                    error = %error,
                    "Failed to read startup OS app metadata during scoped runtime recovery"
                );
                continue;
            }
            Ok(Some(_)) => {}
        }

        match recover_installed_app_runtime_state(state, ps, tenant, app_name).await {
            InstalledAppRuntimeRecoveryOutcome::Ready => summary.ready += 1,
            InstalledAppRuntimeRecoveryOutcome::Healed => summary.healed += 1,
            InstalledAppRuntimeRecoveryOutcome::NeedsReconcile => summary.needs_reconcile += 1,
            InstalledAppRuntimeRecoveryOutcome::MissingBundle => summary.missing_bundle += 1,
            InstalledAppRuntimeRecoveryOutcome::StoreError => summary.store_error += 1,
        }
    }
    summary
}

fn app_required_wasm_failure(app_name: &str, install: &InstallResult) -> Option<String> {
    if install.wasm_failures.is_empty() {
        return None;
    }

    Some(format!(
        "{app_name}: required WASM module(s) failed to load or validate: {}",
        install.wasm_failures.join(", ")
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeRecoveryStep {
    PopulateTypeIndex {
        tenant: String,
        entity_type: &'static str,
    },
}

const STARTUP_RUNTIME_INDEX_ENTITY_TYPES: &[&str] = &["App", "Agent", "Soul"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalWasmStartupPolicy {
    BuildIfMissing,
    LoadPersistedOnly,
}

#[derive(Clone, Default)]
struct StartupReadiness {
    ready: Arc<AtomicBool>,
}

impl StartupReadiness {
    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    fn mark_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
    }
}

fn registry_tenant_ids(state: &PlatformState) -> Vec<TenantId> {
    let registry = state.registry.read().unwrap(); // ci-ok: infallible lock
    registry.tenant_ids().into_iter().cloned().collect()
}

fn local_wasm_startup_policy(raw: Option<&str>) -> LocalWasmStartupPolicy {
    match raw.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if matches!(value.as_str(), "build" | "build-if-missing" | "true" | "1") => {
            LocalWasmStartupPolicy::BuildIfMissing
        }
        Some(value) if matches!(value.as_str(), "load-only" | "persisted" | "false" | "0") => {
            LocalWasmStartupPolicy::LoadPersistedOnly
        }
        _ => LocalWasmStartupPolicy::LoadPersistedOnly,
    }
}

fn runtime_recovery_plan(tenant_ids: &[TenantId]) -> Vec<RuntimeRecoveryStep> {
    let mut steps = Vec::with_capacity(tenant_ids.len() * STARTUP_RUNTIME_INDEX_ENTITY_TYPES.len());
    for tenant_id in tenant_ids {
        for entity_type in STARTUP_RUNTIME_INDEX_ENTITY_TYPES {
            steps.push(RuntimeRecoveryStep::PopulateTypeIndex {
                tenant: tenant_id.as_str().to_string(),
                entity_type,
            });
        }
    }
    steps
}

fn startup_os_apps() -> Vec<String> {
    if !genesis_bootstrap_app_refs().is_empty() {
        return Vec::new();
    }
    list_startup_os_apps()
}

fn default_agent_specs_bootstrap_needed(startup_apps: &[String]) -> bool {
    !startup_apps.iter().any(|app| app == "paw-agent")
}

fn genesis_bootstrap_app_refs() -> Vec<String> {
    let configured = std::env::var("TEMPERPAW_GENESIS_BOOTSTRAP_REFS")
        .or_else(|_| std::env::var("TEMPER_GENESIS_BOOTSTRAP_REFS"))
        .unwrap_or_default();
    configured
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn genesis_bootstrap_app_names() -> Vec<String> {
    genesis_bootstrap_app_refs()
        .into_iter()
        .filter_map(|app_ref| pinned_app_ref_name(&app_ref).map(ToString::to_string))
        .collect()
}

fn genesis_registry_url() -> String {
    std::env::var("TEMPERPAW_GENESIS_REGISTRY_URL")
        .or_else(|_| std::env::var("TEMPER_GENESIS_REGISTRY_URL"))
        .or_else(|_| std::env::var("GENESIS_REGISTRY_URL"))
        .unwrap_or_else(|_| DEFAULT_GENESIS_REGISTRY_URL.to_string())
        .trim()
        .trim_end_matches('/')
        .to_string()
}

fn genesis_registry_tenant() -> String {
    std::env::var("TEMPERPAW_GENESIS_REGISTRY_TENANT")
        .or_else(|_| std::env::var("TEMPER_GENESIS_REGISTRY_TENANT"))
        .unwrap_or_else(|_| "default".to_string())
}

fn genesis_cache_restore_timeout() -> Duration {
    std::env::var("TEMPERPAW_GENESIS_CACHE_RESTORE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_GENESIS_CACHE_RESTORE_TIMEOUT)
}

fn genesis_bootstrap_timeout() -> Duration {
    std::env::var("TEMPERPAW_GENESIS_BOOTSTRAP_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_GENESIS_BOOTSTRAP_TIMEOUT)
}

fn pinned_app_ref_name(app_ref: &str) -> Option<&str> {
    let (owner_and_name, hash) = app_ref.split_once('@')?;
    if hash.trim().is_empty() {
        return None;
    }
    let (_owner, name) = owner_and_name.split_once('/')?;
    let name = name.trim();
    if name.is_empty() { None } else { Some(name) }
}

fn pinned_app_ref_owner(app_ref: &str) -> Option<&str> {
    let (owner_and_name, hash) = app_ref.split_once('@')?;
    if hash.trim().is_empty() {
        return None;
    }
    let (owner, name) = owner_and_name.split_once('/')?;
    if owner.trim().is_empty() || name.trim().is_empty() {
        return None;
    }
    Some(owner.trim())
}

async fn bootstrap_configured_genesis_apps(
    state: &PlatformState,
    platform_store: &dyn PlatformStore,
    tenant: &str,
) -> Result<usize> {
    let app_refs = genesis_bootstrap_app_refs();
    if app_refs.is_empty() {
        return Ok(0);
    }

    let registry_url = genesis_registry_url();
    if !(registry_url.starts_with("http://") || registry_url.starts_with("https://")) {
        anyhow::bail!("TEMPERPAW_GENESIS_REGISTRY_URL must start with http:// or https://");
    }
    let registry_tenant = genesis_registry_tenant();
    let mut installed = 0usize;

    for app_ref in app_refs {
        let Some(app_name) = pinned_app_ref_name(&app_ref).map(ToString::to_string) else {
            anyhow::bail!(
                "TEMPERPAW_GENESIS_BOOTSTRAP_REFS must contain pinned owner/app@hash refs; got '{app_ref}'"
            );
        };
        let Some(_owner) = pinned_app_ref_owner(&app_ref) else {
            anyhow::bail!(
                "TEMPERPAW_GENESIS_BOOTSTRAP_REFS must contain pinned owner/app@hash refs; got '{app_ref}'"
            );
        };

        match platform_store.get_installed_app(tenant, &app_name).await {
            Ok(record) => {
                // Record-only keep decision (see decision log D9): keep any Genesis `installed`
                // record regardless of hash; the env pin is a floor. No runtime-readiness probe —
                // at boot the runtime state is still being restored, so probing it is racy and
                // can wrongly downgrade a healthy newer install.
                match classify_bootstrap_action(record.as_ref()) {
                    BootstrapAction::KeepInstalled => {
                        let installed_ref = record
                            .as_ref()
                            .map(|r| r.app_ref.as_str())
                            .unwrap_or_default();
                        if installed_ref != app_ref {
                            // The env pin is a floor: an install at a different hash is kept and the
                            // pin is NOT force-applied. Surface this so an operator who bumped the
                            // pin expecting an upgrade can see why it did not take — explicit
                            // install is the upgrade path.
                            tracing::warn!(
                                app = %app_name,
                                pinned_ref = %app_ref,
                                installed_ref = %installed_ref,
                                "Keeping installed Genesis app at a different hash than the env pin; the pin bump did NOT upgrade it (env pin is a floor, not a ceiling — use an explicit install to change the running version)"
                            );
                        } else {
                            tracing::info!(
                                app = %app_name,
                                pinned_ref = %app_ref,
                                installed_ref = %installed_ref,
                                "Keeping installed Genesis bootstrap app (matches env pin)"
                            );
                        }
                        continue;
                    }
                    BootstrapAction::InstallPinned => match record.as_ref() {
                        Some(record) => tracing::info!(
                            app = %app_name,
                            previous_ref = %record.app_ref,
                            next_ref = %app_ref,
                            source_kind = %record.source_kind,
                            status = %record.status,
                            "Installing pinned Genesis bootstrap ref (no keep-eligible Genesis install record)"
                        ),
                        None => tracing::info!(
                            app = %app_name,
                            app_ref = %app_ref,
                            "Installing fresh Genesis bootstrap app"
                        ),
                    },
                }
            }
            Err(error) => {
                // An indeterminate read must NOT trigger a (re)install: a transient store outage
                // during redeploy would otherwise overwrite a healthy newer Genesis version with
                // the stale env pin — the exact downgrade this change exists to prevent. Skip and
                // let the next boot reconcile once the store is readable. A genuinely fresh install
                // reads Ok(None), not Err, so this does not strand first installs.
                tracing::warn!(
                    app = %app_name,
                    app_ref = %app_ref,
                    error = %error,
                    "Could not read installed app metadata before Genesis bootstrap; skipping to avoid downgrading a possibly-healthy install (will retry next boot)"
                );
                continue;
            }
        }

        match install_genesis_app_from_registry(
            state,
            GenesisRegistryInstallRequest {
                tenant: tenant.to_string(),
                app_ref: app_ref.clone(),
                registry_url: registry_url.clone(),
                registry_tenant: registry_tenant.clone(),
                follow_policy: "pinned".to_string(),
            },
        )
        .await
        {
            Ok(_) => {
                installed += 1;
            }
            Err(error) => {
                tracing::warn!(
                    app = %app_name,
                    app_ref = %app_ref,
                    error = %error,
                    "Genesis bootstrap install/reconcile failed; continuing startup with durable app recovery"
                );
            }
        }
    }

    Ok(installed)
}

#[cfg(test)]
fn startup_discord_connect_result(result: anyhow::Result<String>) -> Option<String> {
    match result {
        Ok(interaction_url) => Some(interaction_url),
        Err(error) => {
            tracing::error!(
                error = %error,
                "Discord transport failed during startup; continuing without Discord"
            );
            None
        }
    }
}

fn startup_discord_summary_label(
    configured: bool,
    status: &crate::transport_manager::TransportStatus,
) -> Option<String> {
    if !configured {
        return None;
    }

    match status {
        crate::transport_manager::TransportStatus::Connected { .. } => {
            Some("✓ Discord connected".to_string())
        }
        crate::transport_manager::TransportStatus::Connecting => {
            Some("~ Discord configured; connecting".to_string())
        }
        crate::transport_manager::TransportStatus::Disconnected
        | crate::transport_manager::TransportStatus::Error { .. } => {
            Some("~ Discord configured; reconnect pending".to_string())
        }
    }
}

fn spawn_runtime_server(
    listener: tokio::net::TcpListener,
    router: axum::Router,
) -> JoinHandle<std::io::Result<()>> {
    tokio::spawn(async move { axum::serve(listener, router).await })
}

async fn startup_gate_middleware(
    readiness: StartupReadiness,
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = request.uri().path();
    if readiness.is_ready() || path == "/healthz" || path == "/readyz" {
        return next.run(request).await;
    }

    axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
}

fn runtime_router_with_startup_gates(
    router: axum::Router,
    readiness: StartupReadiness,
    setup_state: Option<crate::setup_api::SetupApiState>,
) -> axum::Router {
    let readyz_state = readiness.clone();
    let readyz_setup_state = setup_state.clone();
    router
        .layer(axum::middleware::from_fn(move |request, next| {
            let readiness = readiness.clone();
            async move { startup_gate_middleware(readiness, request, next).await }
        }))
        .route(
            "/readyz",
            axum::routing::get(move || {
                let readiness = readyz_state.clone();
                let setup_state = readyz_setup_state.clone();
                async move {
                    if !readiness.is_ready() {
                        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }

                    if let Some(setup_state) = setup_state {
                        crate::setup_api::get_readyz(axum::extract::State(setup_state))
                            .await
                            .into_response()
                    } else {
                        axum::http::StatusCode::OK.into_response()
                    }
                }
            }),
        )
}

async fn wait_for_runtime_server(url: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let client = reqwest::Client::new();

    loop {
        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(_) | Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Ok(response) => {
                anyhow::bail!(
                    "Runtime server did not become ready: GET {url} -> {}",
                    response.status()
                )
            }
            Err(error) => anyhow::bail!("Runtime server did not become ready at {url}: {error}"),
        }
    }
}

async fn recover_runtime_indexes(state: &PlatformState, tenant_ids: &[TenantId]) {
    for step in runtime_recovery_plan(tenant_ids) {
        match step {
            RuntimeRecoveryStep::PopulateTypeIndex {
                tenant,
                entity_type,
            } => {
                let tenant_id = TenantId::new(&tenant);
                let count = state
                    .server
                    .populate_index_from_store_by_type(&tenant_id, entity_type)
                    .await as u64;
                record_startup_live_restore_entities(&tenant, count);
                tracing::info!(
                    tenant = %tenant,
                    entity_type,
                    count,
                    "live restore: populate_typed_index"
                );
            }
        }
    }
}

fn orphaned_session_recovery_limit() -> Option<usize> {
    let enabled = std::env::var("TEMPERPAW_ORPHANED_SESSION_RECOVERY")
        .ok()
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off" | "disabled" | "none"
            )
        })
        .unwrap_or(true);
    if !enabled {
        return None;
    }

    Some(
        std::env::var("TEMPERPAW_ORPHANED_SESSION_RECOVERY_MAX")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|limit| *limit > 0)
            .unwrap_or(DEFAULT_ORPHANED_SESSION_RECOVERY_LIMIT),
    )
}

async fn recover_orphaned_sessions(state: &PlatformState, tenant: &str) {
    let Some(recovery_limit) = orphaned_session_recovery_limit() else {
        tracing::info!(
            tenant,
            "Orphaned session recovery skipped because TEMPERPAW_ORPHANED_SESSION_RECOVERY is disabled"
        );
        return;
    };

    let terminal_states: HashSet<&str> = ["Completed", "Failed", "Cancelled"].into_iter().collect();
    let recoverable_states: HashSet<&str> = [
        "Thinking",
        "Executing",
        "Compacting",
        "Steering",
        "WaitingForApproval",
    ]
    .into_iter()
    .collect();
    let tenant_id = TenantId::new(tenant);
    state
        .server
        .populate_index_from_store_by_type(&tenant_id, "Session")
        .await;
    let session_ids: Vec<String> = {
        let index = state.server.entity_index.read().unwrap(); // ci-ok: infallible lock
        let index_key = format!("{tenant_id}:Session");
        index
            .get(&index_key)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect()
    };
    if session_ids.len() > recovery_limit {
        tracing::warn!(
            tenant,
            total_sessions = session_ids.len(),
            recovery_limit,
            skipped_sessions = session_ids.len() - recovery_limit,
            "Orphaned session recovery bounded to avoid post-ready actor hydration storm"
        );
    }

    let mut failed = 0u32;
    let mut recovering = 0u32;
    let mut inspected = 0u32;
    for session_id in session_ids.iter().take(recovery_limit) {
        inspected += 1;
        match state
            .server
            .get_tenant_entity_state(&tenant_id, "Session", session_id)
            .await
        {
            Ok(resp) if recoverable_states.contains(resp.state.status.as_str()) => {
                let status = &resp.state.status;
                tracing::info!(session_id, status, "Recovering session from restart");
                let params = serde_json::json!({
                    "error_message": format!("process restart — recovering from {status}")
                });
                match state
                    .server
                    .dispatch_tenant_action(
                        &tenant_id,
                        "Session",
                        session_id,
                        "RecoverFromRestart",
                        params.clone(),
                        &temper_server::request_context::AgentContext::system(),
                    )
                    .await
                {
                    Ok(_) => recovering += 1,
                    Err(e) => {
                        tracing::warn!(session_id, %e, "RecoverFromRestart failed, falling back to Fail");
                        let _ = state
                            .server
                            .dispatch_tenant_action(
                                &tenant_id,
                                "Session",
                                session_id,
                                "Fail",
                                params,
                                &temper_server::request_context::AgentContext::system(),
                            )
                            .await;
                        failed += 1;
                    }
                }
            }
            Ok(resp) if !terminal_states.contains(resp.state.status.as_str()) => {
                let status = &resp.state.status;
                tracing::info!(session_id, status, "Failing orphaned session");
                let params = serde_json::json!({
                    "error_message": format!("process restart — session recovered from {status} state")
                });
                let _ = state
                    .server
                    .dispatch_tenant_action(
                        &tenant_id,
                        "Session",
                        session_id,
                        "Fail",
                        params,
                        &temper_server::request_context::AgentContext::system(),
                    )
                    .await;
                failed += 1;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(session_id, %e, "Failed to read session state"),
        }
    }
    tracing::info!(
        tenant,
        inspected,
        recovering,
        failed,
        "Session recovery complete"
    );
}

fn spawn_deferred_session_recovery(state: PlatformState, tenant: String) {
    tokio::spawn(async move {
        tracing::info!("Deferred session recovery scheduled after readiness");
        let started = Instant::now();
        recover_orphaned_sessions(&state, &tenant).await;
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            "Deferred session recovery complete"
        );
    });
}

fn spawn_query_projection_backfill(
    server: temper_server::state::ServerState,
    tenant_ids: Vec<TenantId>,
) {
    if !query_projection_backfill_on_startup() {
        tracing::info!(
            tenants = tenant_ids.len(),
            "Background query projection backfill disabled for startup"
        );
        return;
    }

    tokio::spawn(async move {
        let delay = query_projection_backfill_delay();
        if !delay.is_zero() {
            tracing::info!(
                tenants = tenant_ids.len(),
                delay_secs = delay.as_secs(),
                "Background query projection backfill delayed"
            );
            tokio::time::sleep(delay).await;
        }
        tracing::info!(
            tenants = tenant_ids.len(),
            "Background query projection backfill scheduled"
        );
        for tenant_id in tenant_ids {
            server.populate_field_index_from_snapshots(&tenant_id).await;
            tracing::info!(tenant = %tenant_id, "Background query projection backfill complete");
        }
    });
}

/// Backfill `entity_key_index` for declared-key entity types (ADR-0153), independent
/// of the heavy field-index re-scan above. K (1-3) tiny key rows per entity, so it is
/// cheap and runs on a short default delay. Lets existing Files/Directories/etc become
/// keyed (so their point reads stop hitting the 413/QueryTooLarge scan) without
/// enabling the expensive projection backfill.
fn spawn_key_index_backfill(server: temper_server::state::ServerState, tenant_ids: Vec<TenantId>) {
    if !key_index_backfill_on_startup() {
        tracing::info!(
            tenants = tenant_ids.len(),
            "Background key-index backfill disabled for startup"
        );
        return;
    }

    tokio::spawn(async move {
        let delay = key_index_backfill_delay();
        if !delay.is_zero() {
            tracing::info!(
                tenants = tenant_ids.len(),
                delay_secs = delay.as_secs(),
                "Background key-index backfill delayed"
            );
            tokio::time::sleep(delay).await;
        }
        tracing::info!(
            tenants = tenant_ids.len(),
            "Background key-index backfill scheduled"
        );
        for tenant_id in tenant_ids {
            server.populate_key_index_from_snapshots(&tenant_id).await;
            tracing::info!(tenant = %tenant_id, "Background key-index backfill complete");
        }
    });
}

fn key_index_backfill_on_startup() -> bool {
    std::env::var("TEMPERPAW_KEY_INDEX_BACKFILL_ON_STARTUP")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
}

fn key_index_backfill_delay() -> Duration {
    const DEFAULT_KEY_INDEX_BACKFILL_DELAY_SECS: u64 = 30;

    let configured = std::env::var("TEMPERPAW_KEY_INDEX_BACKFILL_DELAY_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_KEY_INDEX_BACKFILL_DELAY_SECS);

    Duration::from_secs(configured)
}

fn query_projection_backfill_on_startup() -> bool {
    std::env::var("TEMPERPAW_QUERY_PROJECTION_BACKFILL_ON_STARTUP")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
}

fn query_projection_backfill_delay() -> Duration {
    const DEFAULT_QUERY_PROJECTION_BACKFILL_DELAY_SECS: u64 = 300;

    let configured = std::env::var("TEMPERPAW_QUERY_PROJECTION_BACKFILL_DELAY_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_QUERY_PROJECTION_BACKFILL_DELAY_SECS);

    Duration::from_secs(configured)
}

/// Run the Temper Paw daemon.
///
/// If `force_soul_setup` is true, the soul personalization interview runs
/// after boot regardless of current configuration (used by `temperpaw setup`).
pub async fn run(mut config: Config, force_soul_setup: bool) -> Result<()> {
    let startup_started = Instant::now();
    let port = config.port;
    let tenant = config.tenant.clone();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let data_dir = Path::new(&home).join(".local/share/temperpaw");
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("Failed to create data dir: {}", data_dir.display()))?;
    let api_key_path = data_dir.join("api.key");
    config.temper_api_key = Some(resolve_temper_api_key(&config, &api_key_path).await?);

    // Phase 0: Config setup (API key + messaging — runs pre-boot)
    let needs_soul_setup = if crate::setup::needs_setup(&data_dir, &config) {
        let setup_result = crate::setup::run_setup_config(&config).await?;
        crate::setup::merge_setup_into_config(&mut config, setup_result);
        true
    } else {
        force_soul_setup
    };

    // Reserve the API listener before bootstrapping any app config that needs a local base URL.
    // This gives us the real port up front and prevents other local helper processes from
    // stealing the preferred port while startup is still seeding secrets and entity config.
    tracing::info!("Phase 0.5: Reserving API listener...");
    let listener = match tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await {
        Ok(l) => l,
        Err(_) => {
            tracing::warn!(port, "Port {port} in use — binding to a free port");
            tokio::net::TcpListener::bind("0.0.0.0:0")
                .await
                .context("Failed to bind to any port")?
        }
    };
    let actual_port = listener.local_addr()?.port();
    if actual_port != port {
        tracing::info!("Using port {actual_port} instead of {port}");
    }

    // Phase 1: Storage backend
    tracing::info!("Phase 1: Initializing storage...");
    let default_db_path = data_dir.join("paw.db");
    let storage = PawStorage::connect(&config, &default_db_path).await?;
    let platform_store = storage.platform_store();

    // Phase 2: Build empty registry
    tracing::info!("Phase 2: Building spec registry...");
    let registry = temper_server::SpecRegistry::new();

    // Phase 3: Set OS apps directory + reference apps
    tracing::info!("Phase 3: Loading OS apps from ./os-apps/...");
    let os_apps_dir = PathBuf::from("os-apps");
    if os_apps_dir.exists() {
        temper_platform::os_apps::set_os_apps_dir(os_apps_dir.clone());
    } else {
        tracing::warn!("os-apps/ directory not found — OS apps will not be available");
    }

    // Local app directories are retained for development and tests only.
    // Normal agent-facing app discovery/install goes through Genesis pinned refs.
    let reference_apps_dir = PathBuf::from("reference-projects/deep-sci-fi");
    if reference_apps_dir.exists() {
        temper_platform::os_apps::add_os_apps_dir(reference_apps_dir);
        tracing::info!("  Reference apps directory registered (available for install)");
    }

    // Register Kotowari teaching platform apps
    let kotowari_dir = PathBuf::from("reference-projects/kotowari");
    if kotowari_dir.exists() {
        temper_platform::os_apps::add_os_apps_dir(kotowari_dir);
        tracing::info!("  Kotowari apps directory registered (available for install)");
    }

    // GitHub/submodule/symlink app sources are intentionally not synced here.
    // Genesis is the app source of truth; direct Git is a registry transport,
    // not an extra install catalog.

    // Phase 4: Assemble PlatformState
    tracing::info!("Phase 4: Assembling platform state...");
    let llm_api_key = config
        .anthropic_api_key
        .clone()
        .or_else(|| config.openrouter_api_key.clone())
        .or_else(|| config.huggingface_api_key.clone())
        .or_else(|| config.fireworks_api_key.clone())
        .or_else(|| config.sakana_fugu_api_key.clone())
        .or_else(|| config.openai_compatible_api_key.clone())
        .or_else(|| config.openai_compatible_api_url.clone())
        .or_else(|| config.local_openai_api_url.clone())
        .or_else(|| config.openai_api_key.clone())
        .or_else(|| config.openai_codex_token.clone());
    let mut state = PlatformState::with_registry(registry, llm_api_key);
    state.api_token = config.temper_api_key.clone();
    state.server.data_dir = data_dir.clone();
    state.server.set_storage_stack(storage.storage_stack());

    {
        let restored = restore_registry_guarded(&state, platform_store).await?;
        if restored > 0 {
            tracing::info!(
                backend = storage.backend_name(),
                restored,
                "Restored specs from storage"
            );
        }
        let restored_verifications =
            restore_persisted_spec_verification_statuses(&state, platform_store).await?;
        if restored_verifications > 0 {
            tracing::info!(
                restored = restored_verifications,
                "Restored persisted spec verification statuses"
            );
        }
    }

    // Phase 4b: Bootstrap system + agent specs (GovernanceDecision, Agent, Plan, etc.)
    // Required for Cedar authorization to work — temper-system needs GovernanceDecision.
    {
        let sys_cache = platform_store
            .load_verification_cache("temper-system")
            .await
            .unwrap_or_default();
        let sys_hashes = temper_platform::bootstrap_system_tenant(&state, &sys_cache);
        temper_platform::persist_system_verification(platform_store, &sys_hashes, &sys_cache).await;

        let startup_apps = startup_os_apps();
        let genesis_bootstrap_apps = genesis_bootstrap_app_names();
        if default_agent_specs_bootstrap_needed(&startup_apps)
            && default_agent_specs_bootstrap_needed(&genesis_bootstrap_apps)
        {
            let agent_cache = platform_store
                .load_verification_cache(&tenant)
                .await
                .unwrap_or_default();
            let agent_hashes =
                temper_platform::bootstrap_agent_specs(&state, &tenant, true, &agent_cache);
            temper_platform::persist_agent_verification(
                platform_store,
                &tenant,
                &agent_hashes,
                &agent_cache,
            )
            .await;
        } else {
            tracing::info!(
                tenant = %tenant,
                "Skipping built-in default agent specs bootstrap; paw-agent OS app owns default agent specs"
            );
            // paw-agent owns the agent lifecycle specs but NOT the platform
            // IDENTITY specs (TrustedIssuer, PrincipalGeneration) that back
            // kernel JWT verification (ARN-255). The skipped default bootstrap
            // is the only place those load, so on this host they must be
            // registered explicitly — otherwise `tdata/TrustedIssuer` returns
            // EntitySetNotFound and `bootstrap_trusted_issuer_from_env` below is
            // inert. The kernel's narrow `bootstrap_identity_specs` API registers
            // exactly those two, cascade-free (so it does not reintroduce the OOM
            // the skip exists to prevent) and merged (so paw-agent's specs are
            // preserved). A failure here means JWT verification would be inert, so
            // it fails readiness rather than booting silently broken.
            temper_platform::bootstrap_identity_specs(&state, &tenant)
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to register platform identity specs for tenant '{tenant}': {e}"
                    )
                })?;
        }
        // Bootstrap the operator credential so the deployment's TEMPER_API_KEY
        // resolves to a real tenant AgentCredential on the credential-bound
        // auth edge (ADR-0157). Without this the kernel edge has no credential
        // to resolve and every authenticated request fails closed with 401.
        if let Some(ref api_key) = config.temper_api_key {
            // ARN-255 cold-boot ordering: on a virgin store paw-agent has not yet
            // registered AgentType / AgentCredential, so the credential bootstrap
            // would dispatch against missing tables and (previously) silently
            // swallow the error — a 401 until the second boot. Register those two
            // specs first (cascade-free, merged, idempotent) so the dispatch below
            // resolves on the very first boot regardless of branch.
            temper_platform::bootstrap_operator_credential_specs(&state, &tenant)
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to register AgentType/AgentCredential specs for tenant '{tenant}': {e}"
                    )
                })?;
            // Propagate the credential bootstrap error instead of discarding it:
            // a failure here means authenticated requests fail closed with 401,
            // so fail readiness loudly rather than boot silently broken.
            temper_platform::bootstrap_operator_credential(&state, api_key, &tenant)
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to bootstrap operator credential for tenant '{tenant}': {e}"
                    )
                })?;
            tracing::info!(tenant = %tenant, "Bootstrapped operator credential for TEMPER_API_KEY");
        } else {
            tracing::warn!(
                "No TEMPER_API_KEY resolved; operator credential not bootstrapped — \
                 authenticated requests will fail closed on the credential-bound edge"
            );
        }

        // ARN-255: activate kernel JWT verification for this deployment by
        // registering the trusted issuer from the environment. This dispatches
        // RegisterIssuer on the TrustedIssuer entity, which requires that spec
        // to be registered — guaranteed above (default agent-specs bootstrap in
        // the taken branch, or the explicit identity-spec registration in the
        // skip branch). The kernel API classifies the TEMPER_TRUSTED_ISSUER_*
        // env authoritatively: none set is a safe no-op; a partial config or a
        // registration failure is an error we fail readiness on — never a
        // "registering…" log followed by a silent no-op.
        match temper_platform::bootstrap_trusted_issuer_from_env(&state, &tenant).await {
            Ok(temper_platform::IssuerBootstrapOutcome::Registered) => {
                tracing::info!(
                    tenant = %tenant,
                    "Registered trusted JWT issuer from TEMPER_TRUSTED_ISSUER_* environment (ARN-255)"
                );
            }
            Ok(temper_platform::IssuerBootstrapOutcome::Skipped) => {
                tracing::info!(
                    tenant = %tenant,
                    "No TEMPER_TRUSTED_ISSUER_* env set; kernel JWT verification inactive for this deployment"
                );
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Trusted issuer bootstrap failed for tenant '{tenant}': {e}"
                ));
            }
        }

        tracing::info!("Bootstrapped startup platform specs for temper-system and {tenant}");
    }

    // Phase 5: Secrets vault
    tracing::info!("Phase 5: Configuring secrets vault...");
    let vault_key_bytes: [u8; 32] = {
        let vault_key_path = data_dir.join("vault.key");
        let key_bytes: [u8; 32] = if let Some(ref key_b64) = config.vault_key {
            use base64::Engine as _;

            match base64::engine::general_purpose::STANDARD.decode(key_b64) {
                Ok(decoded) if decoded.len() == 32 => {
                    tracing::info!("Vault key loaded from TEMPER_VAULT_KEY env var");
                    decoded.try_into().unwrap()
                }
                Ok(decoded) => {
                    let mut key = [0u8; 32];
                    rand::fill(&mut key);
                    tracing::warn!(
                        actual_len = decoded.len(),
                        "TEMPER_VAULT_KEY was not 32 bytes after base64 decode — using ephemeral vault key"
                    );
                    key
                }
                Err(error) => {
                    let mut key = [0u8; 32];
                    rand::fill(&mut key);
                    tracing::warn!(
                        %error,
                        "TEMPER_VAULT_KEY was invalid base64 — using ephemeral vault key"
                    );
                    key
                }
            }
        } else if vault_key_path.exists() {
            // Load persisted vault key from file
            use base64::Engine as _;
            match std::fs::read_to_string(&vault_key_path) {
                Ok(contents) => {
                    match base64::engine::general_purpose::STANDARD.decode(contents.trim()) {
                        Ok(decoded) if decoded.len() == 32 => {
                            tracing::info!(
                                path = %vault_key_path.display(),
                                "Vault key loaded from file"
                            );
                            decoded.try_into().unwrap()
                        }
                        _ => {
                            tracing::warn!(
                                path = %vault_key_path.display(),
                                "Vault key file was corrupt — generating new key"
                            );

                            generate_and_save_vault_key(&vault_key_path)?
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        path = %vault_key_path.display(),
                        "Failed to read vault key file — generating new key"
                    );
                    generate_and_save_vault_key(&vault_key_path)?
                }
            }
        } else {
            // First run: generate and persist a new vault key
            tracing::info!(
                path = %vault_key_path.display(),
                "No vault key found — generating and saving new key"
            );
            generate_and_save_vault_key(&vault_key_path)?
        };
        // If we generated a new key (no env var) and Railway is available, persist it
        // so the key survives across container redeploys (Railway has no persistent disk).
        if config.vault_key.is_none()
            && let (Some(token), Some(project_id), Some(env_id), Some(service_id)) = (
                &config.railway_token,
                &config.railway_project_id,
                &config.railway_environment_id,
                &config.railway_service_id,
            )
        {
            use base64::Engine as _;
            let key_b64 = base64::engine::general_purpose::STANDARD.encode(key_bytes);
            match persist_vault_key_to_railway(token, project_id, env_id, service_id, &key_b64)
                .await
            {
                Ok(()) => {
                    tracing::info!("Vault key persisted to Railway env var TEMPER_VAULT_KEY");
                }
                Err(e) => {
                    tracing::warn!(
                        %e,
                        "Failed to persist vault key to Railway — account data will be lost on next redeploy"
                    );
                }
            }
        }

        let vault = temper_server::secrets::vault::SecretsVault::new(&key_bytes);
        state.server.secrets_vault = Some(Arc::new(vault));
        key_bytes
    };

    // Phase 5b: Restore secrets from durable storage (before env seeding so env vars take priority)
    if let Some(ref vault) = state.server.secrets_vault {
        restore_secrets_as_platform(vault, &storage, &tenant).await;
        if tenant != "default" {
            // Migration shim for older deployments that stored shared startup
            // secrets under the legacy "default" tenant bucket.
            restore_secrets_as_platform(vault, &storage, "default").await;
        }
    }

    // Phase 5c: Seed secrets from env (env vars override stored values)
    //
    // Each secret is cached in-memory AND persisted so it survives
    // restarts even if the env var is later removed.
    if let Some(ref vault) = state.server.secrets_vault {
        /// Helper to seed a shared platform secret from an optional env value.
        macro_rules! seed_secret {
            ($vault:expr, $store:expr, $tenant:expr, $key:expr, $value:expr) => {
                if let Some(ref val) = $value {
                    cache_platform_and_persist_secret($vault, $store, $tenant, $key, val.clone())
                        .await;
                }
            };
        }

        seed_secret!(
            vault,
            &storage,
            &tenant,
            "anthropic_api_key",
            config.anthropic_api_key
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "openrouter_api_key",
            config.openrouter_api_key
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "openrouter_api_url",
            config.openrouter_api_url
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "huggingface_api_key",
            config.huggingface_api_key
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "huggingface_api_url",
            config.huggingface_api_url
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "fireworks_api_key",
            config.fireworks_api_key
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "fireworks_api_url",
            config.fireworks_api_url
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "sakana_fugu_api_key",
            config.sakana_fugu_api_key
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "sakana_fugu_api_url",
            config.sakana_fugu_api_url
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "openai_compatible_api_key",
            config.openai_compatible_api_key
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "openai_compatible_api_url",
            config.openai_compatible_api_url
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "openai_compatible_headers_json",
            config.openai_compatible_headers_json
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "local_openai_api_url",
            config.local_openai_api_url
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "openai_api_key",
            config.openai_api_key
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "openai_codex_token",
            config.openai_codex_token
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "llm_provider",
            config.llm_provider
        );
        seed_secret!(vault, &storage, &tenant, "llm_model", config.llm_model);
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "tensorlake_api_key",
            config.tensorlake_api_key
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "sandbox_provider",
            config.sandbox_provider
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "modal_token_id",
            config.modal_token_id
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "modal_token_secret",
            config.modal_token_secret
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "modal_bridge_url",
            config.modal_bridge_url
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "github_token",
            config.github_token
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "genesis_token",
            config.genesis_token
        );
        seed_secret!(vault, &storage, &tenant, "dd_api_key", config.dd_api_key);
        seed_secret!(vault, &storage, &tenant, "dd_app_key", config.dd_app_key);
        seed_secret!(vault, &storage, &tenant, "exa_api_key", config.exa_api_key);
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "temper_api_key",
            config.temper_api_key
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "discord_bot_token",
            config.discord_bot_token
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "discord_public_key",
            config.discord_public_key
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "discord_guild_id",
            config.discord_guild_id
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "discord_feed_channel_id",
            config.discord_feed_channel_id
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "discord_forum_channel_id",
            config.discord_forum_channel_id
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "slack_bot_token",
            config.slack_bot_token
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "slack_app_token",
            config.slack_app_token
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "fly_api_token",
            config.fly_api_token
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "railway_token",
            config.railway_token
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "railway_project_id",
            config.railway_project_id
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "railway_environment_id",
            config.railway_environment_id
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "railway_otel_service_id",
            config.railway_otel_service_id
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "railway_datadog_runtime_agent_service_id",
            config.railway_datadog_runtime_agent_service_id
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "railway_service_id",
            config.railway_service_id
        );
        seed_secret!(
            vault,
            &storage,
            &tenant,
            "vercel_token",
            config.vercel_token
        );

        // dd_site always has a value (defaults to "datadoghq.com")
        cache_platform_and_persist_secret(
            vault,
            &storage,
            &tenant,
            "dd_site",
            config.dd_site.clone(),
        )
        .await;

        // temper_api_url — always set to local server
        let api_url = format!("http://127.0.0.1:{actual_port}");
        let _ = vault.cache_platform_secret("temper_api_url", api_url);

        if let Some(worker_id) = std::env::var("LOCAL_CODEX_WORKER_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            let _ = vault.cache_platform_secret("local_codex_worker_id", worker_id);
        }
        if let Some(worktree_root) = std::env::var("LOCAL_CODEX_WORKTREE_ROOT")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            let _ = vault.cache_platform_secret("local_codex_worktree_root", worktree_root);
        }

        // Sandbox URL: explicit override for testing, otherwise Tensorlake provisions on demand.
        if let Some(sandbox_url) = std::env::var("SANDBOX_URL").ok().filter(|s| !s.is_empty()) {
            cache_platform_and_persist_secret(
                vault,
                &storage,
                &tenant,
                "sandbox_url",
                sandbox_url.clone(),
            )
            .await;
        } else {
            let provider = resolve_startup_secret(
                Some(vault),
                &tenant,
                "sandbox_provider",
                config.sandbox_provider.clone(),
            )
            .unwrap_or_else(|| "tensorlake".to_string());
            let tensorlake_api_key = resolve_startup_secret(
                Some(vault),
                &tenant,
                "tensorlake_api_key",
                config.tensorlake_api_key.clone(),
            );
            let modal_token_id = resolve_startup_secret(
                Some(vault),
                &tenant,
                "modal_token_id",
                config.modal_token_id.clone(),
            );
            let modal_token_secret = resolve_startup_secret(
                Some(vault),
                &tenant,
                "modal_token_secret",
                config.modal_token_secret.clone(),
            );
            let modal_bridge_url = resolve_startup_secret(
                Some(vault),
                &tenant,
                "modal_bridge_url",
                config.modal_bridge_url.clone(),
            );
            match provider.as_str() {
                "tensorlake" if tensorlake_api_key.is_some() => {
                    tracing::info!("Sandbox provider: tensorlake (API key configured)");
                }
                "modal"
                    if modal_token_id.is_some()
                        && modal_token_secret.is_some()
                        && modal_bridge_url.is_some() =>
                {
                    tracing::info!("Sandbox provider: modal (token + bridge configured)");
                }
                "modal" if modal_token_id.is_some() && modal_token_secret.is_some() => {
                    tracing::warn!(
                        "Sandbox provider is 'modal' but MODAL_BRIDGE_URL / modal_bridge_url is not set; TemperPaw deploy should provision it automatically"
                    );
                }
                "modal" => {
                    tracing::warn!(
                        "Sandbox provider is 'modal' but MODAL_TOKEN_ID or MODAL_TOKEN_SECRET not set"
                    );
                }
                "tensorlake" => {
                    tracing::warn!("No TL_API_KEY or SANDBOX_URL — sandbox provisioning will fail");
                }
                other => {
                    tracing::warn!(
                        "Unsupported SANDBOX_PROVIDER={other} — use 'tensorlake' or 'modal'"
                    );
                }
            }
        }

        // Blob store for TemperFS content uploads/downloads.
        //
        // Production must use an external S3/R2-compatible object store. Local
        // development can use Temper's internal route, which now writes through
        // Temper's filesystem object store rather than Turso DB blobs.
        let blob_endpoint = if let Ok(url) = std::env::var("BLOB_ENDPOINT") {
            url
        } else if running_on_railway() {
            anyhow::bail!(
                "BLOB_ENDPOINT is required on Railway; blob bytes must be stored in R2/S3, not in the database"
            );
        } else {
            format!("http://127.0.0.1:{actual_port}/_internal/blobs")
        };
        let blob_bucket = std::env::var("BLOB_BUCKET").unwrap_or_else(|_| "temper-fs".into());
        let _ = vault.cache_platform_secret("blob_endpoint", blob_endpoint.clone());
        let _ = vault.cache_platform_secret("blob_bucket", blob_bucket);

        // Generic public artifact storage. This is separate from the private
        // TemperFS blob bucket so read-only public surfaces can serve immutable
        // published bytes without exposing governed working files.
        let published_blob_endpoint = std::env::var("PUBLISHED_BLOB_ENDPOINT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                if running_on_railway() {
                    None
                } else {
                    Some(blob_endpoint.clone())
                }
            });
        let published_blob_bucket = std::env::var("PUBLISHED_BLOB_BUCKET")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "published-artifacts".into());
        let published_blob_public_base_url = std::env::var("PUBLISHED_BLOB_PUBLIC_BASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                if running_on_railway() {
                    None
                } else {
                    published_blob_endpoint.as_ref().map(|endpoint| {
                        format!(
                            "{}/{}",
                            endpoint.trim_end_matches('/'),
                            published_blob_bucket.trim_matches('/')
                        )
                    })
                }
            });

        if let Some(endpoint) = published_blob_endpoint {
            let _ = vault.cache_platform_secret("published_blob_endpoint", endpoint);
            let _ = vault.cache_platform_secret("published_blob_bucket", published_blob_bucket);
        } else {
            tracing::warn!(
                "PUBLISHED_BLOB_ENDPOINT is not configured; public artifact publishing is disabled"
            );
        }
        if let Some(public_base_url) = published_blob_public_base_url {
            let _ = vault.cache_platform_secret("published_blob_public_base_url", public_base_url);
        } else if running_on_railway() {
            tracing::warn!(
                "PUBLISHED_BLOB_PUBLIC_BASE_URL is not configured; public artifact publishing is disabled"
            );
        }

        if let Ok(key) = std::env::var("PUBLISHED_BLOB_ACCESS_KEY") {
            cache_platform_and_persist_secret(
                vault,
                &storage,
                &tenant,
                "published_blob_access_key",
                key.clone(),
            )
            .await;
        }
        if let Ok(key) = std::env::var("PUBLISHED_BLOB_SECRET_KEY") {
            cache_platform_and_persist_secret(
                vault,
                &storage,
                &tenant,
                "published_blob_secret_key",
                key.clone(),
            )
            .await;
        }

        // HMAC credentials for GCS (or any S3-compatible blob store).
        if let Ok(key) = std::env::var("BLOB_ACCESS_KEY") {
            cache_platform_and_persist_secret(
                vault,
                &storage,
                &tenant,
                "blob_access_key",
                key.clone(),
            )
            .await;
        }
        if let Ok(key) = std::env::var("BLOB_SECRET_KEY") {
            cache_platform_and_persist_secret(
                vault,
                &storage,
                &tenant,
                "blob_secret_key",
                key.clone(),
            )
            .await;
        }
    }

    let startup_readiness = StartupReadiness::default();
    let _ = state.server.listen_port.set(actual_port);

    let transport_manager = Arc::new(crate::transport_manager::TransportManager::new(
        tenant.clone(),
        actual_port,
        config.temper_api_key.clone(),
        config.public_base_url.clone(),
        config.ngrok_bin.clone(),
        config.ngrok_authtoken.clone(),
    ));

    let cookie_secure = config
        .public_base_url
        .as_deref()
        .map(|url| url.starts_with("https://"))
        .unwrap_or(false);
    let auth_state = crate::auth::AuthState::new(
        storage.clone(),
        state
            .server
            .secrets_vault
            .as_ref()
            .context("Vault must be initialized before auth")?
            .clone(),
        vault_key_bytes.to_vec(),
        tenant.clone(),
        cookie_secure,
    );

    let router = build_platform_router(state.clone());
    let setup_state = crate::setup_api::SetupApiState {
        platform: state.clone(),
        storage: storage.clone(),
        transport_manager: transport_manager.clone(),
        tenant: tenant.clone(),
        agents_dir: PathBuf::from("os-apps/paw-agent/agents"),
        base_url: format!("http://127.0.0.1:{actual_port}"),
        build_version: config.build_version.clone(),
        build_sha: config.build_sha.clone(),
    };
    let webhook_api = paw_transport::PawApiClient::new(paw_transport::PawApiConfig {
        base_url: format!("http://127.0.0.1:{actual_port}"),
        tenant: tenant.clone(),
        api_key: config.temper_api_key.clone(),
    });
    let router = router
        .merge(crate::setup_api::router(setup_state.clone()))
        .merge(crate::auth::router(auth_state.clone()))
        .merge(paw_transport::webhook::router(webhook_api));

    let router = router.layer(axum::extract::DefaultBodyLimit::max(50 * 1024 * 1024));

    let router = if std::path::Path::new("dashboard/build").exists() {
        use tower_http::services::{ServeDir, ServeFile};
        router.nest_service(
            "/dashboard",
            ServeDir::new("dashboard/build").fallback(ServeFile::new("dashboard/build/index.html")),
        )
    } else {
        router
    };
    let router = router.layer(axum::middleware::from_fn_with_state(
        auth_state,
        crate::auth::middleware,
    ));
    let router =
        runtime_router_with_startup_gates(router, startup_readiness.clone(), Some(setup_state));

    let serve_handle = spawn_runtime_server(listener, router);
    wait_for_runtime_server(
        format!("http://127.0.0.1:{actual_port}/healthz").as_str(),
        Duration::from_secs(5),
    )
    .await
    .context("Temper Paw HTTP API failed to become reachable during startup")?;

    let genesis_cache_restore_timeout = genesis_cache_restore_timeout();
    let restored_genesis_registry_caches = match tokio::time::timeout(
        genesis_cache_restore_timeout,
        restore_genesis_registry_cache_roots(&state),
    )
    .await
    {
        Ok(restored) => restored,
        Err(_) => {
            tracing::warn!(
                timeout_ms = genesis_cache_restore_timeout.as_millis(),
                "Genesis registry cache recovery timed out; continuing startup without restored cache roots"
            );
            0
        }
    };
    if restored_genesis_registry_caches > 0 {
        tracing::info!(
            restored = restored_genesis_registry_caches,
            "Restored Genesis registry app cache roots"
        );
    }

    // Phase 6a: Recover persisted WASM modules + Cedar policies BEFORE app install.
    //
    // OS app install (Phase 6b) runs an integrity check that verifies each
    // spec's [[integration]] references a WASM module in the registry. On
    // subsequent startups the modules live in durable storage, not in the app bundle,
    // so the registry must be populated first. Cedar policies are similarly
    // needed for any effects dispatched during install (e.g. workspace
    // bootstrap actions).
    let app_runtime_recovery = {
        let phase_started = Instant::now();
        tracing::info!(
            "Phase 6a: Recovering runtime app state, persisted WASM modules + Cedar policies..."
        );
        recover_cedar_policies(&state, platform_store).await;
        let app_runtime_recovery =
            recover_installed_apps_runtime_state(&state, platform_store).await;
        tracing::info!(
            ready = app_runtime_recovery.ready,
            healed = app_runtime_recovery.healed,
            needs_reconcile = app_runtime_recovery.needs_reconcile,
            missing_bundle = app_runtime_recovery.missing_bundle,
            store_error = app_runtime_recovery.store_error,
            result = installed_app_runtime_recovery_result(&app_runtime_recovery),
            "Installed OS app runtime recovery complete"
        );
        if let Err(error) = state.server.load_wasm_modules().await {
            record_wasm_module_load_failure("phase_6a_pre_recovery");
            return Err(anyhow::anyhow!("Failed to recover WASM modules: {error}"));
        }
        record_startup_phase_duration("phase_6a_pre_recovery", phase_started.elapsed());
        tracing::info!(
            elapsed_ms = phase_started.elapsed().as_millis(),
            "phase_6a_pre_recovery complete"
        );
        app_runtime_recovery
    };

    // Genesis bootstrap install/reconcile — runs AFTER Phase 6a on purpose. The env-pinned ref is
    // a floor, not a ceiling. The keep-or-reinstall decision is RECORD-ONLY (decision log D9):
    // it reads the durable InstalledAppRecord and never probes runtime readiness — at boot the
    // runtime is still being restored concurrently, so a probe can misread a healthy newer
    // install and wrongly downgrade it to the env pin, the exact redeploy-downgrade this change
    // exists to prevent. Bootstrap still belongs after Phase 6a because bootstrap IS an install
    // and sits with the other install/reconcile phases, after persisted-state recovery.
    let genesis_bootstrap_timeout = genesis_bootstrap_timeout();
    let genesis_bootstrap_installs = match tokio::time::timeout(
        genesis_bootstrap_timeout,
        bootstrap_configured_genesis_apps(&state, platform_store, &tenant),
    )
    .await
    {
        Ok(installs) => installs?,
        Err(_) => {
            tracing::warn!(
                timeout_ms = genesis_bootstrap_timeout.as_millis(),
                "Genesis bootstrap install/reconcile timed out; continuing startup without completing bootstrap"
            );
            0
        }
    };
    if genesis_bootstrap_installs > 0 {
        tracing::info!(
            installed = genesis_bootstrap_installs,
            "Installed configured Genesis bootstrap apps"
        );
    }

    let startup_apps = startup_os_apps();
    tracing::info!(apps = ?startup_apps, "Startup OS app surface resolved from manifests");
    let startup_app_order = resolve_os_app_install_order(&startup_apps)
        .map_err(|error| anyhow::anyhow!("Failed to resolve startup OS app order: {error}"))?;
    tracing::info!(apps = ?startup_app_order, "Startup OS app reconcile order resolved");
    let startup_surface_runtime_recovery =
        recover_startup_surface_runtime_state(&state, platform_store, &tenant, &startup_app_order)
            .await;
    tracing::info!(
        ready = startup_surface_runtime_recovery.ready,
        healed = startup_surface_runtime_recovery.healed,
        cold = startup_surface_runtime_recovery.cold,
        needs_reconcile = startup_surface_runtime_recovery.needs_reconcile,
        missing_bundle = startup_surface_runtime_recovery.missing_bundle,
        store_error = startup_surface_runtime_recovery.store_error,
        global_ready = app_runtime_recovery.ready,
        global_healed = app_runtime_recovery.healed,
        global_needs_reconcile = app_runtime_recovery.needs_reconcile,
        result = startup_surface_runtime_recovery_result(&startup_surface_runtime_recovery),
        "Startup OS app runtime recovery scoped to readiness surface complete"
    );

    // Phase 6a.5: Runtime index recovery before app reconcile.
    //
    // Changed/cold app reconcile may bootstrap durable TemperFS content, whose
    // helpers may need a few startup entity indexes. These are recovered by
    // entity type so deploys never do a whole-tenant event/index replay.
    let tenant_ids = registry_tenant_ids(&state);
    let recover_indexes_before_reconcile =
        startup_surface_runtime_indexes_required_before_reconcile(
            &startup_surface_runtime_recovery,
        );
    if recover_indexes_before_reconcile {
        let phase_started = Instant::now();
        tracing::info!("Phase 6a.5: Recovering startup runtime indexes before app reconcile...");
        recover_runtime_indexes(&state, &tenant_ids).await;
        record_startup_phase_duration("phase_6a5_runtime_index_recovery", phase_started.elapsed());
        tracing::info!(
            elapsed_ms = phase_started.elapsed().as_millis(),
            "phase_6a5_runtime_index_recovery complete"
        );
    } else {
        let phase_started = Instant::now();
        record_startup_phase_duration("phase_6a5_runtime_index_recovery", phase_started.elapsed());
        tracing::info!(
            elapsed_ms = phase_started.elapsed().as_millis(),
            "phase_6a5_runtime_index_recovery skipped; runtime indexes recover lazily by type"
        );
    }

    // Phase 6b: Reconcile Paw OS apps
    let phase_started = Instant::now();
    tracing::info!("Phase 6b: Reconciling Paw OS apps...");
    let wasm_policy = local_wasm_startup_policy(
        std::env::var("TEMPERPAW_WASM_STARTUP_POLICY")
            .ok()
            .as_deref(),
    );
    tracing::info!(?wasm_policy, "WASM startup policy selected");
    if wasm_policy == LocalWasmStartupPolicy::BuildIfMissing
        && let Err(error) = build_missing_wasm_modules(&os_apps_dir, &startup_app_order)
    {
        record_wasm_module_load_failure("phase_6b_local_build");
        tracing::error!(%error, "Failed to build local OS app WASM artifacts");
    }

    let mut reconcile_errors = Vec::new();
    for app_name in &startup_app_order {
        let app_started = Instant::now();
        if get_os_app(app_name).is_none() {
            record_os_app_reconcile(app_name, "missing", app_started.elapsed());
            let error = format!("OS app '{app_name}' bundle is missing or invalid");
            tracing::error!("{error}");
            reconcile_errors.push(error);
            continue;
        }
        match reconcile_os_app(&state, &tenant, app_name).await {
            Ok(OsAppReconcileResult::Skipped { bundle_digest, .. }) => {
                persist_os_app_verification(&state, platform_store, &tenant, app_name).await;
                record_os_app_reconcile(app_name, "skipped", app_started.elapsed());
                tracing::info!(
                    app = %app_name,
                    bundle_digest = %bundle_digest,
                    "  Skipped unchanged OS app"
                );
            }
            Ok(OsAppReconcileResult::Installed { install, .. }) => {
                persist_os_app_verification(&state, platform_store, &tenant, app_name).await;
                if let Some(error) = app_required_wasm_failure(app_name, &install) {
                    record_wasm_module_load_failure("phase_6b_required_app_wasm");
                    reconcile_errors.push(error);
                }
                record_os_app_reconcile(app_name, "installed", app_started.elapsed());
                tracing::info!("  Reconciled {app_name}: {install:?}");
            }
            Err(error) => {
                record_os_app_reconcile(app_name, "error", app_started.elapsed());
                tracing::error!("  Failed to reconcile {app_name}: {error}");
                reconcile_errors.push(format!("{app_name}: {error}"));
            }
        }
    }

    if !reconcile_errors.is_empty() {
        anyhow::bail!(
            "Startup OS app reconcile failed for {} app(s): {}",
            reconcile_errors.len(),
            reconcile_errors.join("; ")
        );
    }

    // Safety net: commit all specs for the tenant.
    // install_os_app() calls commit_specs() internally, but if a previous daemon
    // run crashed between upsert (committed=0) and commit (committed=1), specs
    // would be left uncommitted and deleted on the NEXT restart by
    // delete_uncommitted_specs(). This explicit commit ensures all OS app specs
    // are durable before we proceed to entity hydration.
    if let Err(e) = platform_store.commit_specs(&tenant).await {
        tracing::error!("Failed to commit specs after OS app install: {e}");
    } else {
        tracing::info!("Specs committed for tenant {tenant}");
    }
    record_startup_phase_duration("phase_6b_os_app_reconcile", phase_started.elapsed());
    tracing::info!(
        elapsed_ms = phase_started.elapsed().as_millis(),
        "phase_6b_os_app_reconcile complete"
    );

    // Phase 7: Refresh reaction dispatcher after any app reconcile changes.
    let phase_started = Instant::now();
    tracing::info!("Phase 7: Runtime dispatcher refresh...");
    state.server.rebuild_reaction_dispatcher();
    record_startup_phase_duration("phase_7_dispatcher_refresh", phase_started.elapsed());
    tracing::info!(
        elapsed_ms = phase_started.elapsed().as_millis(),
        "phase_7_dispatcher_refresh complete"
    );

    // Phase 7b: Session recovery — recover or fail orphaned sessions (ADR-0025).
    if recover_indexes_before_reconcile {
        recover_orphaned_sessions(&state, &tenant).await;
    } else {
        tracing::info!("Session recovery deferred until after readiness");
    }

    // Phase 8: Banner (printed after bind so we show the actual port)
    tracing::info!("Phase 8: Bootstrap complete");

    // Phase 9: Finish runtime bring-up on the already-running HTTP server.
    tracing::info!("Phase 9: Finalizing runtime bring-up...");

    // Spawn webhook trigger (ONE entity, ONE action per request).
    spawn_webhook_trigger(&tenant, actual_port, config.temper_api_key.clone());

    // Cron scheduling is now handled by the platform's schedule_at effect —
    // CronJob entities self-schedule via ActivateComplete/TriggerComplete.

    // Start transports from vault (env vars were seeded into vault in Phase 5).
    // The TransportManager enables runtime connect/disconnect via the /paw/ API.
    {
        let vault = state.server.secrets_vault.as_ref();
        let discord_token = vault.and_then(|v| v.get_secret(&tenant, "discord_bot_token"));
        if let Some(token) = discord_token {
            let configured_public_key = vault
                .and_then(|v| v.get_secret(&tenant, "discord_public_key"))
                .or_else(|| config.discord_public_key.clone());
            let _public_key = if let Some(vault) = vault {
                match crate::setup_api::resolve_and_persist_discord_public_key(
                    vault,
                    &storage,
                    &tenant,
                    &token,
                    configured_public_key.as_deref(),
                )
                .await
                {
                    Ok(public_key) => {
                        if configured_public_key.as_deref() != Some(public_key.as_str()) {
                            tracing::info!(
                                "Refreshed Discord verify_key from the Discord API during startup"
                            );
                        }
                        Some(public_key)
                    }
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            "Failed to refresh Discord verify_key during startup; using the configured value if present"
                        );
                        configured_public_key
                    }
                }
            } else {
                configured_public_key
            };
            let feed_channel_id = vault
                .and_then(|v| v.get_secret(&tenant, "discord_feed_channel_id"))
                .or_else(|| config.discord_feed_channel_id.clone());
            let forum_channel_id = vault
                .and_then(|v| v.get_secret(&tenant, "discord_forum_channel_id"))
                .or_else(|| config.discord_forum_channel_id.clone());

            match crate::setup_api::schedule_discord_reconcile(&state, &tenant).await {
                Ok(()) => {
                    tracing::info!("Discord transport reconcile scheduled from TransportConnection")
                }
                Err(error) => tracing::error!(
                    %error,
                    "Discord transport reconcile could not be scheduled during startup"
                ),
            }

            // Spawn Discord observer (SSE → Discord feed/forum).
            if feed_channel_id.is_some() || forum_channel_id.is_some() {
                let observer_api = paw_transport::PawApiClient::new(paw_transport::PawApiConfig {
                    base_url: format!("http://127.0.0.1:{actual_port}"),
                    tenant: tenant.clone(),
                    api_key: config.temper_api_key.clone(),
                });
                let observer_config = paw_transport::discord::ObserverConfig {
                    bot_token: token,
                    feed_channel_id,
                    forum_channel_id,
                };
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    if let Err(e) =
                        paw_transport::discord::run_observer(observer_api, observer_config).await
                    {
                        tracing::error!("Discord observer failed: {e}");
                    }
                });
            }
        } else {
            tracing::warn!("No discord_bot_token in vault — Discord transport not started");
        }

        let slack_bot = vault.and_then(|v| v.get_secret(&tenant, "slack_bot_token"));
        let slack_app = vault.and_then(|v| v.get_secret(&tenant, "slack_app_token"));
        if let (Some(app_token), Some(bot_token)) = (slack_app, slack_bot) {
            let signing_secret = vault
                .and_then(|v| v.get_secret(&tenant, "slack_signing_secret"))
                .or_else(|| config.slack_signing_secret.clone())
                .unwrap_or_default();
            transport_manager
                .connect_slack(crate::transport_manager::SlackConnectParams {
                    app_token,
                    bot_token,
                    signing_secret,
                })
                .await;
        } else {
            tracing::warn!("No slack tokens in vault — Slack transport not started");
        }
    }

    // Spawn background loops
    state.server.spawn_runtime_metrics_loop();
    spawn_actor_passivation_loop(&state);

    // Resolve LLM config from vault/dashboard before env. Runtime must not
    // invent provider/model defaults.
    let resolved_llm_provider = state
        .server
        .secrets_vault
        .as_ref()
        .and_then(|v| v.get_secret(&tenant, "llm_provider"))
        .or_else(|| config.llm_provider.clone());
    let resolved_llm_model = state
        .server
        .secrets_vault
        .as_ref()
        .and_then(|v| v.get_secret(&tenant, "llm_model"))
        .or_else(|| config.llm_model.clone());
    let preserve_personalized_paw_soul = state
        .server
        .secrets_vault
        .as_ref()
        .and_then(|v| v.get_secret(&tenant, "paw_personalized_soul"))
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"));

    if let (Some(llm_provider), Some(llm_model)) = (resolved_llm_provider, resolved_llm_model) {
        spawn_soul_bootstrap(
            actual_port,
            tenant.clone(),
            config.temper_api_key.clone(),
            llm_provider,
            llm_model,
            preserve_personalized_paw_soul,
        );
    } else {
        tracing::warn!(
            "Skipping agent bootstrap because llm_provider and llm_model are not both configured"
        );
    }

    // Print startup summary
    {
        let vault = state.server.secrets_vault.as_ref();
        let has_api_key = vault
            .and_then(|v| {
                v.get_secret(&tenant, "anthropic_api_key")
                    .or_else(|| v.get_secret(&tenant, "openrouter_api_key"))
                    .or_else(|| v.get_secret(&tenant, "huggingface_api_key"))
                    .or_else(|| v.get_secret(&tenant, "hf_token"))
                    .or_else(|| v.get_secret(&tenant, "fireworks_api_key"))
                    .or_else(|| v.get_secret(&tenant, "sakana_fugu_api_key"))
                    .or_else(|| v.get_secret(&tenant, "openai_compatible_api_key"))
                    .or_else(|| v.get_secret(&tenant, "openai_compatible_api_url"))
                    .or_else(|| v.get_secret(&tenant, "local_openai_api_url"))
                    .or_else(|| v.get_secret(&tenant, "openai_api_key"))
                    .or_else(|| v.get_secret(&tenant, "openai_codex_token"))
            })
            .is_some();
        let has_discord = vault
            .and_then(|v| v.get_secret(&tenant, "discord_bot_token"))
            .is_some_and(|value| !value.trim().is_empty());
        let has_slack = vault
            .and_then(|v| v.get_secret(&tenant, "slack_bot_token"))
            .is_some();
        let transport_status = transport_manager.status().await;

        println!();
        println!("  Temper Paw is running.");
        println!();
        println!("  API:       http://localhost:{actual_port}/tdata");
        println!("  Dashboard: http://localhost:{actual_port}/dashboard");
        println!(
            "  API key:   {}",
            config.temper_api_key.as_deref().unwrap_or("")
        );
        println!();
        if has_api_key {
            println!("  \u{2713} LLM API key");
        }
        if let Some(label) = startup_discord_summary_label(has_discord, &transport_status.discord) {
            println!("  {label}");
            if let Some(interaction_url) = transport_manager.discord_interaction_public_url().await
            {
                println!("  Discord interactions: {interaction_url}");
            }
        }
        if has_slack {
            println!("  \u{2713} Slack");
        }
        if !has_api_key && !has_discord && !has_slack {
            println!("  Run setup: cargo run -- setup");
        }
        println!();
    }
    startup_readiness.mark_ready();
    record_startup_time_to_ready(startup_started.elapsed(), &tenant);
    tracing::info!("Temper Paw listening on port {actual_port}");
    tracing::info!(elapsed_ms = startup_started.elapsed().as_millis(), tenant = %tenant, "startup: time to ready");
    if !recover_indexes_before_reconcile {
        spawn_deferred_session_recovery(state.clone(), tenant.clone());
    }
    // Query projections are repaired as optional maintenance, not startup work.
    // Incremental projection writes remain active on entity changes.
    spawn_query_projection_backfill(state.server.clone(), tenant_ids.clone());
    // ADR-0153/ARN-68: independent, cheap key-index backfill (own flag), so existing
    // declared-key entities become keyed without the heavy field-index re-scan.
    spawn_key_index_backfill(state.server.clone(), tenant_ids.clone());

    // Phase 10: Soul personalization (post-boot, writes to TemperFS via OData)
    if needs_soul_setup {
        let api_key = state
            .server
            .secrets_vault
            .as_ref()
            .and_then(|v| {
                v.get_secret(&tenant, "anthropic_api_key")
                    .or_else(|| v.get_secret(&tenant, "openrouter_api_key"))
                    .or_else(|| v.get_secret(&tenant, "huggingface_api_key"))
                    .or_else(|| v.get_secret(&tenant, "hf_token"))
                    .or_else(|| v.get_secret(&tenant, "fireworks_api_key"))
                    .or_else(|| v.get_secret(&tenant, "sakana_fugu_api_key"))
                    .or_else(|| v.get_secret(&tenant, "openai_compatible_api_key"))
                    .or_else(|| v.get_secret(&tenant, "openai_compatible_api_url"))
                    .or_else(|| v.get_secret(&tenant, "local_openai_api_url"))
                    .or_else(|| v.get_secret(&tenant, "openai_api_key"))
                    .or_else(|| v.get_secret(&tenant, "openai_codex_token"))
            })
            .unwrap_or_default();
        let provider_name = state
            .server
            .secrets_vault
            .as_ref()
            .and_then(|v| v.get_secret(&tenant, "llm_provider"))
            .or_else(|| config.llm_provider.clone());
        let model_name = state
            .server
            .secrets_vault
            .as_ref()
            .and_then(|v| v.get_secret(&tenant, "llm_model"))
            .or_else(|| config.llm_model.clone());
        let setup_auth = crate::setup::SetupRequestAuth::from_cookie(
            crate::auth::issue_session_cookie_value(&vault_key_bytes, "bootstrap@local.temperpaw")?,
        );

        if let (Some(provider_name), Some(model_name)) = (provider_name, model_name) {
            if let Err(e) = crate::setup::run_setup_soul(
                actual_port,
                &api_key,
                &provider_name,
                &model_name,
                &tenant,
                setup_auth,
            )
            .await
            {
                tracing::warn!("Soul setup failed: {e}");
            }
        } else {
            tracing::warn!(
                "Skipping soul setup because llm_provider and llm_model are not both configured"
            );
        }

        serve_handle.await??;
    } else {
        serve_handle.await??;
    }

    Ok(())
}

/// Cache a shared platform secret in-memory and persist it under the configured
/// tenant bucket so it survives restarts.
async fn cache_platform_and_persist_secret(
    vault: &temper_server::secrets::vault::SecretsVault,
    store: &PawStorage,
    tenant: &str,
    key: &str,
    value: String,
) {
    let _ = vault.cache_platform_secret(key, value.clone());
    match vault.encrypt(value.as_bytes()) {
        Ok((ciphertext, nonce)) => {
            if let Err(e) = store.upsert_secret(tenant, key, &ciphertext, &nonce).await {
                tracing::warn!(key, tenant, %e, "Failed to persist secret");
            }
        }
        Err(e) => {
            tracing::warn!(key, tenant, %e, "Failed to encrypt secret for persistence");
        }
    }
}

/// Restore persisted shared secrets from durable storage into the platform cache.
///
/// The first restored value wins so a configured tenant bucket takes
/// precedence over the legacy `"default"` bucket during migration.
async fn restore_secrets_as_platform(
    vault: &temper_server::secrets::vault::SecretsVault,
    store: &PawStorage,
    tenant: &str,
) {
    match store.load_secrets_for_tenant(tenant).await {
        Ok(rows) => {
            let mut restored = 0u32;
            for (key_name, ciphertext, nonce) in rows {
                match vault.decrypt(&ciphertext, &nonce) {
                    Ok(plaintext) => {
                        if let Ok(value) = String::from_utf8(plaintext)
                            && vault.get_platform_secret(&key_name).is_none()
                        {
                            let _ = vault.cache_platform_secret(&key_name, value);
                            restored += 1;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            key = key_name,
                            tenant,
                            %e,
                            "Failed to decrypt persisted secret — skipping"
                        );
                    }
                }
            }
            if restored > 0 {
                tracing::info!(tenant, restored, "Restored secrets from durable storage");
            }
        }
        Err(e) => {
            tracing::warn!(tenant, %e, "Failed to load secrets from durable storage");
        }
    }
}

fn resolve_startup_secret(
    vault: Option<&temper_server::secrets::vault::SecretsVault>,
    tenant: &str,
    key: &str,
    configured_value: Option<String>,
) -> Option<String> {
    configured_value
        .filter(|value| !value.trim().is_empty())
        .or_else(|| vault.and_then(|vault| vault.get_platform_secret(key)))
        .or_else(|| vault.and_then(|vault| vault.get_secret(tenant, key)))
}

/// Generate a random 32-byte vault key, save it to disk as base64, and return the raw bytes.
fn generate_and_save_vault_key(path: &Path) -> Result<[u8; 32]> {
    use base64::Engine as _;

    let mut key = [0u8; 32];
    rand::fill(&mut key);
    let encoded = base64::engine::general_purpose::STANDARD.encode(key);
    std::fs::write(path, &encoded)
        .with_context(|| format!("Failed to write vault key to {}", path.display()))?;

    // Set file permissions to owner-only (0o600) on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)
            .with_context(|| format!("Failed to set permissions on {}", path.display()))?;
    }

    tracing::info!(path = %path.display(), "Saved new vault key to file");
    Ok(key)
}

/// Persist the vault key to Railway as an environment variable so it survives container redeploys.
/// Railway containers have no persistent disk, so without this the vault key is regenerated
/// on every deploy and all encrypted secrets (including user accounts) become unreadable.
async fn persist_vault_key_to_railway(
    token: &str,
    project_id: &str,
    environment_id: &str,
    service_id: &str,
    vault_key_b64: &str,
) -> Result<()> {
    persist_service_variable_to_railway(
        token,
        project_id,
        environment_id,
        service_id,
        "TEMPER_VAULT_KEY",
        vault_key_b64,
    )
    .await
}

async fn persist_temper_api_key_to_railway(
    token: &str,
    project_id: &str,
    environment_id: &str,
    service_id: &str,
    api_key: &str,
) -> Result<()> {
    persist_service_variable_to_railway(
        token,
        project_id,
        environment_id,
        service_id,
        "TEMPER_API_KEY",
        api_key,
    )
    .await
}

async fn persist_service_variable_to_railway(
    token: &str,
    project_id: &str,
    environment_id: &str,
    service_id: &str,
    key: &str,
    value: &str,
) -> Result<()> {
    let client = reqwest::Client::new();
    let query = serde_json::json!({
        "query": "mutation($input: VariableUpsertInput!) { variableUpsert(input: $input) }",
        "variables": {
            "input": {
                "projectId": project_id,
                "environmentId": environment_id,
                "serviceId": service_id,
                "name": key,
                "value": value,
                "skipDeploys": true
            }
        }
    });

    let resp = client
        .post("https://backboard.railway.com/graphql/v2")
        .bearer_auth(token)
        .json(&query)
        .send()
        .await
        .context("Railway GraphQL request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Railway API returned {status}: {body}");
    }

    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if let Some(errors) = body.get("errors") {
        anyhow::bail!("Railway GraphQL errors: {errors}");
    }

    Ok(())
}

async fn fetch_service_variable_from_railway(
    token: &str,
    project_id: &str,
    environment_id: &str,
    service_id: &str,
    key: &str,
) -> Result<Option<String>> {
    let client = reqwest::Client::new();
    let query = serde_json::json!({
        "query": "query variables($projectId: String!, $environmentId: String!, $serviceId: String) { variables(projectId: $projectId, environmentId: $environmentId, serviceId: $serviceId) }",
        "variables": {
            "projectId": project_id,
            "environmentId": environment_id,
            "serviceId": service_id,
        }
    });

    let resp = client
        .post("https://backboard.railway.com/graphql/v2")
        .bearer_auth(token)
        .json(&query)
        .send()
        .await
        .context("Railway GraphQL request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Railway API returned {status}: {body}");
    }

    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if let Some(errors) = body.get("errors") {
        anyhow::bail!("Railway GraphQL errors: {errors}");
    }

    Ok(body
        .get("data")
        .and_then(|data| data.get("variables"))
        .and_then(|variables| variables.get(key))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string()))
}

async fn resolve_temper_api_key(config: &Config, path: &Path) -> Result<String> {
    if let Some(key) = config.temper_api_key.clone() {
        return Ok(key);
    }

    if let (Some(token), Some(project_id), Some(env_id), Some(service_id)) = (
        &config.railway_token,
        &config.railway_project_id,
        &config.railway_environment_id,
        &config.railway_service_id,
    ) {
        match fetch_service_variable_from_railway(
            token,
            project_id,
            env_id,
            service_id,
            "TEMPER_API_KEY",
        )
        .await
        {
            Ok(Some(key)) if !key.trim().is_empty() => {
                if let Err(error) = save_temper_api_key(path, &key) {
                    tracing::warn!(
                        %error,
                        path = %path.display(),
                        "Loaded TEMPER_API_KEY from Railway but failed to refresh local cache file"
                    );
                }
                tracing::info!("Using TEMPER_API_KEY from Railway env var");
                return Ok(key);
            }
            Ok(_) => {
                let key = load_or_create_temper_api_key(None, path)?;
                match persist_temper_api_key_to_railway(token, project_id, env_id, service_id, &key)
                    .await
                {
                    Ok(()) => {
                        tracing::info!("Bootstrapped TEMPER_API_KEY into Railway env var");
                    }
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            "Failed to bootstrap TEMPER_API_KEY into Railway — current process will use a generated key only"
                        );
                    }
                }
                return Ok(key);
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "Failed to read TEMPER_API_KEY from Railway — falling back to local key cache"
                );
            }
        }
    }

    load_or_create_temper_api_key(None, path)
}

fn load_or_create_temper_api_key(explicit_key: Option<String>, path: &Path) -> Result<String> {
    if let Some(key) = explicit_key {
        return Ok(key);
    }

    if path.exists() {
        let key = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read API key from {}", path.display()))?;
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(key);
        }
    }

    let key = generate_temper_api_key();
    save_temper_api_key(path, &key)?;
    tracing::info!(path = %path.display(), "Saved new API key to file");
    Ok(key)
}

fn generate_temper_api_key() -> String {
    use rand::Rng;

    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

fn save_temper_api_key(path: &Path, key: &str) -> Result<()> {
    std::fs::write(path, key)
        .with_context(|| format!("Failed to write API key to {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)
            .with_context(|| format!("Failed to set permissions on {}", path.display()))?;
    }

    Ok(())
}

async fn persist_os_app_verification(
    state: &PlatformState,
    store: &dyn PlatformStore,
    tenant: &str,
    app_name: &str,
) {
    let Some(bundle) = get_os_app(app_name) else {
        return;
    };
    let verified_at = sim_now().to_rfc3339();
    let tenant_id = TenantId::new(tenant);

    for (entity_type, _) in &bundle.specs {
        if let Err(error) = store
            .persist_spec_verification(
                tenant,
                entity_type,
                SpecVerificationUpdate {
                    status: "completed",
                    verified: true,
                    levels_passed: None,
                    levels_total: None,
                    verification_result_json: None,
                },
            )
            .await
        {
            tracing::warn!(
                tenant,
                app = app_name,
                entity_type,
                error = %error,
                "Failed to persist OS app verification status"
            );
        }

        let mut registry = state.registry.write().unwrap(); // ci-ok: infallible lock
        registry.set_verification_status(
            &tenant_id,
            entity_type,
            VerificationStatus::Completed(EntityVerificationResult {
                all_passed: true,
                levels: vec![EntityLevelSummary {
                    level: "Bootstrap".to_string(),
                    passed: true,
                    summary: format!("Pre-verified via os-app install ({app_name})"),
                    details: None,
                }],
                verified_at: verified_at.clone(),
            }),
        );
    }
}

/// Bootstrap Paw souls into the entity system.
///
/// Reads soul files from `os-apps/paw-agent/agents/` directory, creates TemperFS File entities
/// for the content, and registers Soul entities. Runs once on first boot;
/// skips if souls already exist.
fn spawn_soul_bootstrap(
    port: u16,
    tenant: String,
    api_key: Option<String>,
    llm_provider: String,
    llm_model: String,
    preserve_personalized_paw_soul: bool,
) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;

        let api_url = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();

        // Check for personalized Paw soul from `temperpaw setup`
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let generated_dir = Path::new(&home).join(".local/share/temperpaw/generated");
        let gen_soul = generated_dir.join("paw-soul.md");
        let gen_style = generated_dir.join("paw-style.md");
        let gen_user = generated_dir.join("user.md");

        // Build Paw's soul paths: prefer generated files, always include AGENT.md for operations
        let mut paw_paths: Vec<String> = Vec::new();
        if gen_soul.exists() {
            paw_paths.push(gen_soul.to_string_lossy().to_string());
            if gen_style.exists() {
                paw_paths.push(gen_style.to_string_lossy().to_string());
            }
            if gen_user.exists() {
                paw_paths.push(gen_user.to_string_lossy().to_string());
            }
            tracing::info!("Using personalized Paw soul from setup");
        } else {
            paw_paths.push("os-apps/paw-agent/agents/paw/SOUL.md".to_string());
            paw_paths.push("os-apps/paw-agent/agents/paw/STYLE.md".to_string());
        }
        // AGENT.md always included — operational instructions don't change with personalization
        paw_paths.push("os-apps/paw-agent/agents/paw/AGENT.md".to_string());
        let paw_path_refs: Vec<&str> = paw_paths.iter().map(|s| s.as_str()).collect();

        // Agent definitions: (name, role, description, soul_paths)
        // Agent is the primary entity. Soul is optional — attached to Agent by ID.
        let agents: Vec<(&str, &str, &str, Option<Vec<&str>>)> = vec![
            (
                "Paw",
                "chief-of-staff",
                "Paw chief of staff agent",
                Some(paw_path_refs),
            ),
            (
                "SWE",
                "developer",
                "Software developer agent",
                Some(vec!["os-apps/paw-agent/agents/swe/AGENT.md"]),
            ),
            (
                "SRE",
                "sre",
                "Site reliability engineering agent",
                Some(vec!["os-apps/paw-agent/agents/sre/AGENT.md"]),
            ),
            (
                "Probe",
                "probe",
                "Foresight probe agent for projecting product futures",
                Some(vec!["os-apps/paw-agent/agents/probe/AGENT.md"]),
            ),
        ];

        let default_config = default_agent_config(&api_url, &api_key, &llm_provider, &llm_model);

        for (name, role, description, soul_paths) in &agents {
            // Step 1: Create Agent entity (agent-first)
            let agent_id = match bootstrap_agent(
                &client,
                &api_url,
                &tenant,
                &api_key,
                name,
                role,
                description,
                &default_config,
            )
            .await
            {
                Ok(id) => {
                    tracing::info!("  Agent '{name}' ready: {id}");
                    id
                }
                Err(e) => {
                    tracing::error!("  Failed to bootstrap agent '{name}': {e}");
                    continue;
                }
            };

            // Step 2: Optionally create/attach Soul
            if let Some(paths) = soul_paths {
                match bootstrap_soul(
                    &client,
                    &api_url,
                    &tenant,
                    &api_key,
                    &agent_id,
                    name,
                    description,
                    paths,
                    preserve_personalized_paw_soul && *name == "Paw",
                )
                .await
                {
                    Ok(soul_id) => {
                        // Attach Soul to Agent by ID
                        if let Err(e) = attach_soul_to_agent(
                            &client, &api_url, &tenant, &api_key, &agent_id, &soul_id,
                        )
                        .await
                        {
                            tracing::warn!("  Could not attach soul to agent '{name}': {e}");
                        } else {
                            tracing::info!(
                                "  Soul '{name}' ({soul_id}) attached to Agent {agent_id}"
                            );
                        }
                    }
                    Err(e) => tracing::warn!("  Failed to bootstrap soul for '{name}': {e}"),
                }
            }
        }

        // Skills are now bootstrapped as TemperFS files by the OS app installer
        // (install_os_app → bootstrap_skills). No separate skill bootstrap needed.

        // Point the global AgentRoute to the Paw Agent entity (by ID, not by name)
        if let Err(e) = set_default_agent(&client, &api_url, &tenant, &api_key, "Paw").await {
            tracing::warn!("Could not set default agent on AgentRoute: {e}");
        }
    });
}

// Restore spec registry from durable platform storage. The write guard must outlive the await
// because the upstream bootstrap helper needs `&mut RegistryGuard`. This runs
// once at startup before any request path touches the registry, so the
// lock-across-await clippy guidance doesn't apply.
#[allow(clippy::await_holding_lock)]
async fn restore_registry_guarded(
    state: &PlatformState,
    store: &dyn PlatformStore,
) -> Result<usize> {
    let mut registry = state.registry.write().unwrap();
    restore_registry_from_platform_store(&mut registry, store)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to restore registry from platform store: {e}"))
}

async fn restore_persisted_spec_verification_statuses(
    state: &PlatformState,
    store: &dyn PlatformStore,
) -> Result<usize> {
    let rows = store
        .load_specs()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load specs for verification restore: {e}"))?;
    if rows.is_empty() {
        return Ok(0);
    }

    let mut specs_by_tenant: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for row in rows {
        specs_by_tenant
            .entry(row.tenant)
            .or_default()
            .push((row.entity_type, row.content_hash));
    }

    let mut specs_to_restore: Vec<(String, String)> = Vec::new();
    for (tenant, specs) in &specs_by_tenant {
        let verification_cache = store
            .load_verification_cache(tenant)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    tenant,
                    error = %e,
                    "Failed to load persisted spec verification cache during startup restore"
                );
                BTreeMap::new()
            });
        if verification_cache.is_empty() {
            continue;
        }

        for (entity_type, content_hash) in specs {
            let Some((cached_hash, verified)) = verification_cache.get(entity_type.as_str()) else {
                continue;
            };
            if !*verified || cached_hash != content_hash {
                continue;
            }

            specs_to_restore.push((tenant.clone(), entity_type.clone()));
        }
    }

    let restored = specs_to_restore.len();
    if restored == 0 {
        return Ok(0);
    }

    let verified_at = sim_now().to_rfc3339();
    {
        let mut registry = state.registry.write().unwrap(); // ci-ok: infallible startup lock
        for (tenant, entity_type) in specs_to_restore {
            let tenant_id = TenantId::new(&tenant);
            registry.set_verification_status(
                &tenant_id,
                &entity_type,
                VerificationStatus::Completed(EntityVerificationResult {
                    all_passed: true,
                    levels: vec![EntityLevelSummary {
                        level: "DurableVerificationCache".to_string(),
                        passed: true,
                        summary: "Restored from matching persisted verification cache".to_string(),
                        details: None,
                    }],
                    verified_at: verified_at.clone(),
                }),
            );
        }
    }

    Ok(restored)
}

/// Create or find an Agent entity by name.
#[allow(clippy::too_many_arguments)]
async fn bootstrap_agent(
    client: &reqwest::Client,
    api_url: &str,
    tenant: &str,
    api_key: &Option<String>,
    name: &str,
    role: &str,
    description: &str,
    config: &serde_json::Value,
) -> Result<String> {
    let escaped_name = name.replace('\'', "''");
    let filter = format!("name eq '{escaped_name}' and Status eq 'Active'");
    let list_url = format!("{api_url}/tdata/Agents?$filter={filter}");
    let resp = odata_get(client, &list_url, tenant, api_key).await?;

    if let Some(items) = resp["value"].as_array()
        && !items.is_empty()
    {
        let id = entity_id_from_json(&items[0]).unwrap_or("unknown");
        tracing::info!("  Agent '{name}' already exists: {id}");
        for existing in items {
            if let Err(err) = repair_existing_default_agent(
                client, api_url, tenant, api_key, name, existing, config,
            )
            .await
            {
                tracing::warn!("  Could not repair existing Agent '{name}' tool config: {err}");
            }
        }
        return Ok(id.to_string());
    }

    // Create new Agent entity
    let create_resp = odata_post(
        client,
        &format!("{api_url}/tdata/Agents"),
        tenant,
        api_key,
        serde_json::json!({}),
    )
    .await?;
    let agent_id = create_resp["entity_id"]
        .as_str()
        .or_else(|| create_resp["fields"]["Id"].as_str())
        .or_else(|| create_resp["Id"].as_str())
        .context("Agent creation did not return Id")?
        .to_string();

    // Configure the agent
    let model = config["model"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .context("Agent bootstrap requires model in agent config")?;
    let provider = config["provider"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .context("Agent bootstrap requires provider in agent config")?;
    let tools_enabled = config["tools_enabled"].as_str().unwrap_or("");
    let max_turns = config["max_turns"].as_str().unwrap_or("24");

    odata_post(
        client,
        &format!("{api_url}/tdata/Agents('{agent_id}')/TemperPaw.Configure"),
        tenant,
        api_key,
        serde_json::json!({
            "name": name,
            "role": role,
            "description": description,
            "model": model,
            "provider": provider,
            "tools_enabled": tools_enabled,
            "max_turns": max_turns,
        }),
    )
    .await?;

    Ok(agent_id)
}

/// Repair persisted default Agent rows created before newer platform tools
/// existed. Sessions spawned from AgentRoutes prefer Agent.tools_enabled over
/// route config, so stale Agent rows can hide newly shipped tools indefinitely.
#[allow(clippy::too_many_arguments)]
async fn repair_existing_default_agent(
    client: &reqwest::Client,
    api_url: &str,
    tenant: &str,
    api_key: &Option<String>,
    name: &str,
    existing: &serde_json::Value,
    default_config: &serde_json::Value,
) -> Result<()> {
    let current_tools = entity_field_str(existing, &["ToolsEnabled", "tools_enabled"])
        .unwrap_or("")
        .trim();
    let Some(repaired_tools) = repair_default_agent_tools_enabled(current_tools) else {
        return Ok(());
    };
    let agent_id = entity_id_from_json(existing)
        .filter(|id| !id.is_empty())
        .context("existing Agent entity missing ID")?;

    let model = entity_field_str(existing, &["Model", "model"])
        .filter(|value| !value.trim().is_empty())
        .or_else(|| default_config["model"].as_str())
        .unwrap_or("");
    let provider = entity_field_str(existing, &["Provider", "provider"])
        .filter(|value| !value.trim().is_empty())
        .or_else(|| default_config["provider"].as_str())
        .unwrap_or("");
    let max_turns = entity_field_str(existing, &["MaxTurns", "max_turns"])
        .filter(|value| !value.trim().is_empty())
        .or_else(|| default_config["max_turns"].as_str())
        .unwrap_or("24");
    let description =
        entity_field_str(existing, &["Description", "description"]).unwrap_or_default();
    let instructions_file_id =
        entity_field_str(existing, &["InstructionsFileId", "instructions_file_id"])
            .unwrap_or_default();
    let temperature = entity_field_str(existing, &["Temperature", "temperature"]).unwrap_or("1.0");

    odata_post(
        client,
        &format!("{api_url}/tdata/Agents('{agent_id}')/TemperPaw.Update"),
        tenant,
        api_key,
        serde_json::json!({
            "description": description,
            "instructions_file_id": instructions_file_id,
            "model": model,
            "provider": provider,
            "temperature": temperature,
            "tools_enabled": repaired_tools,
            "max_turns": max_turns,
        }),
    )
    .await?;
    tracing::info!("  Repaired Agent '{name}' tools_enabled for Genesis app workflow");

    Ok(())
}

/// Attach a Soul entity to an Agent by updating the Agent's soul_id field.
async fn attach_soul_to_agent(
    client: &reqwest::Client,
    api_url: &str,
    tenant: &str,
    api_key: &Option<String>,
    agent_id: &str,
    soul_id: &str,
) -> Result<()> {
    odata_post(
        client,
        &format!("{api_url}/tdata/Agents('{agent_id}')/TemperPaw.Update"),
        tenant,
        api_key,
        serde_json::json!({ "soul_id": soul_id }),
    )
    .await?;
    Ok(())
}

/// Create or find a Soul entity for the given soul files.
///
/// Multiple paths are concatenated with `\n\n` separators (e.g. SOUL.md + STYLE.md + SKILL.md).
#[allow(clippy::too_many_arguments)]
async fn bootstrap_soul(
    client: &reqwest::Client,
    api_url: &str,
    tenant: &str,
    api_key: &Option<String>,
    agent_id: &str,
    name: &str,
    description: &str,
    paths: &[&str],
    preserve_existing_content: bool,
) -> Result<String> {
    let content = paths
        .iter()
        .map(|p| {
            std::fs::read_to_string(p).with_context(|| format!("Failed to read soul file: {p}"))
        })
        .collect::<Result<Vec<_>>>()?
        .join("\n\n");

    let agent_resp = odata_get(
        client,
        &format!("{api_url}/tdata/Agents('{agent_id}')"),
        tenant,
        api_key,
    )
    .await?;
    if let Some(attached_soul_id) = entity_field_str(&agent_resp, &["soul_id", "SoulId"]) {
        let soul_resp = odata_get(
            client,
            &format!("{api_url}/tdata/Souls('{attached_soul_id}')"),
            tenant,
            api_key,
        )
        .await?;
        if let Some(file_id) = entity_field_str(&soul_resp, &["ContentFileId", "content_file_id"]) {
            if should_preserve_paw_soul_content(
                client,
                api_url,
                tenant,
                api_key,
                name,
                file_id,
                preserve_existing_content,
            )
            .await
            {
                tracing::info!("  Preserving existing soul '{name}': {attached_soul_id}");
                return Ok(attached_soul_id.to_string());
            }
            let upload_url = format!("{api_url}/tdata/Files('{file_id}')/$value");
            odata_put_bytes(
                client,
                &upload_url,
                tenant,
                api_key,
                "text/markdown",
                content.clone().into_bytes(),
            )
            .await
            .with_context(|| format!("Failed to refresh attached soul content for '{name}'"))?;
            tracing::info!("  Soul '{name}' already attached: {attached_soul_id}");
            return Ok(attached_soul_id.to_string());
        }
    }

    for filter in soul_lookup_filters(name) {
        let list_url = format!("{api_url}/tdata/Souls?$filter={filter}");
        let resp = odata_get(client, &list_url, tenant, api_key).await?;
        if let Some(existing) = resp["value"].as_array().and_then(|items| items.first()) {
            let id = entity_id_from_json(existing).unwrap_or("unknown");
            if let Some(file_id) = entity_field_str(existing, &["ContentFileId", "content_file_id"])
            {
                if should_preserve_paw_soul_content(
                    client,
                    api_url,
                    tenant,
                    api_key,
                    name,
                    file_id,
                    preserve_existing_content,
                )
                .await
                {
                    tracing::info!("  Preserving existing soul '{name}': {id}");
                    return Ok(id.to_string());
                }
                let upload_url = format!("{api_url}/tdata/Files('{file_id}')/$value");
                odata_put_bytes(
                    client,
                    &upload_url,
                    tenant,
                    api_key,
                    "text/markdown",
                    content.into_bytes(),
                )
                .await
                .with_context(|| format!("Failed to refresh existing soul content for '{name}'"))?;
            }
            tracing::info!("  Soul '{name}' already exists: {id}");
            return Ok(id.to_string());
        }
    }

    let file_resp = odata_post(
        client,
        &format!("{api_url}/tdata/Files"),
        tenant,
        api_key,
        serde_json::json!({
            "Name": format!("{name}.soul.md"),
            "MimeType": "text/markdown"
        }),
    )
    .await?;
    let file_id = file_resp["entity_id"]
        .as_str()
        .or_else(|| file_resp["fields"]["Id"].as_str())
        .or_else(|| file_resp["Id"].as_str())
        .context("File creation did not return Id")?
        .to_string();

    let upload_url = format!("{api_url}/tdata/Files('{file_id}')/$value");
    odata_put_bytes(
        client,
        &upload_url,
        tenant,
        api_key,
        "text/markdown",
        content.into_bytes(),
    )
    .await?;

    let soul_resp = odata_post(
        client,
        &format!("{api_url}/tdata/Souls"),
        tenant,
        api_key,
        serde_json::json!({
            "Name": name,
            "Description": description,
            "ContentFileId": file_id
        }),
    )
    .await?;
    let soul_id = soul_resp["entity_id"]
        .as_str()
        .or_else(|| soul_resp["fields"]["Id"].as_str())
        .or_else(|| soul_resp["Id"].as_str())
        .context("Soul creation did not return Id")?
        .to_string();

    odata_post(
        client,
        &format!("{api_url}/tdata/Souls('{soul_id}')/TemperPaw.Publish"),
        tenant,
        api_key,
        serde_json::json!({}),
    )
    .await?;

    Ok(soul_id)
}

async fn should_preserve_paw_soul_content(
    client: &reqwest::Client,
    api_url: &str,
    tenant: &str,
    api_key: &Option<String>,
    name: &str,
    file_id: &str,
    preserve_existing_content: bool,
) -> bool {
    if preserve_existing_content {
        return true;
    }

    if name != "Paw" {
        return false;
    }

    let Ok(default_content) = crate::setup::default_paw_soul_content() else {
        return false;
    };
    let Ok(current_content) = odata_get_text(
        client,
        &format!("{api_url}/tdata/Files('{file_id}')/$value"),
        tenant,
        api_key,
    )
    .await
    else {
        return false;
    };

    paw_soul_content_is_personalized(&current_content, &default_content)
}

fn paw_soul_content_is_personalized(current_content: &str, default_content: &str) -> bool {
    current_content.trim() != default_content.trim()
}

fn soul_lookup_filters(name: &str) -> [String; 2] {
    let escaped_name = name.replace('\'', "''");
    let escaped_lower_name = name.to_lowercase().replace('\'', "''");
    [
        format!("Name eq '{escaped_name}'"),
        format!("name eq '{escaped_lower_name}'"),
    ]
}

/// Point the global AgentRoute to the named Agent entity (by ID).
async fn set_default_agent(
    client: &reqwest::Client,
    api_url: &str,
    tenant: &str,
    api_key: &Option<String>,
    agent_name: &str,
) -> Result<()> {
    // Find the Agent entity by name
    let escaped_name = agent_name.replace('\'', "''");
    let agents_resp = odata_get(
        client,
        &format!("{api_url}/tdata/Agents?$filter=name eq '{escaped_name}' and Status eq 'Active'"),
        tenant,
        api_key,
    )
    .await?;
    let agents = agents_resp["value"]
        .as_array()
        .context("Failed to list active agents")?;

    let target_agent = agents
        .first()
        .context(format!("Agent '{agent_name}' not found"))?;
    let target_agent_id = entity_id_from_json(target_agent)
        .context("Agent entity missing ID")?
        .to_string();

    let routes_resp = odata_get(
        client,
        &format!("{api_url}/tdata/AgentRoutes"),
        tenant,
        api_key,
    )
    .await?;

    let mut has_global_route = false;
    if let Some(routes) = routes_resp["value"].as_array() {
        for route in routes {
            let route_id = entity_id_from_json(route).unwrap_or("");
            let current_agent_id = entity_field_str(route, &["AgentId", "agent_id"]).unwrap_or("");
            let channel_id = entity_field_str(route, &["ChannelId", "channel_id"]).unwrap_or("");
            let current_config =
                entity_field_str(route, &["AgentConfig", "agent_config"]).unwrap_or("");

            // Repair: update agent_id if missing or pointing to wrong agent
            let needs_repair = current_agent_id.is_empty() || current_agent_id != target_agent_id;
            if needs_repair && !route_id.is_empty() {
                odata_post(
                    client,
                    &format!("{api_url}/tdata/AgentRoutes('{route_id}')/Paw.Channel.Update"),
                    tenant,
                    api_key,
                    serde_json::json!({ "agent_id": target_agent_id }),
                )
                .await
                .ok();
                tracing::info!("  Set agent_id={target_agent_id} on AgentRoute {route_id}");
            }
            if !route_id.is_empty()
                && let Some(repaired_config) =
                    repaired_agent_config(current_config, api_url, api_key, channel_id.is_empty())
            {
                odata_post(
                    client,
                    &format!("{api_url}/tdata/AgentRoutes('{route_id}')/Paw.Channel.Update"),
                    tenant,
                    api_key,
                    serde_json::json!({ "agent_config": repaired_config }),
                )
                .await
                .ok();
                tracing::info!("  Repaired agent_config on AgentRoute {route_id}");
            }
            if channel_id.is_empty() {
                has_global_route = true;
            }
        }
    }

    // Ensure a global fallback AgentRoute exists pointing to the Agent entity.
    if !has_global_route {
        tracing::info!(
            "  No global AgentRoute found — creating one with agent '{agent_name}' ({target_agent_id})"
        );
        let create_resp = odata_post(
            client,
            &format!("{api_url}/tdata/AgentRoutes"),
            tenant,
            api_key,
            serde_json::json!({}),
        )
        .await;
        if let Ok(created) = create_resp {
            let route_id = entity_id_from_json(&created).unwrap_or("");
            if !route_id.is_empty() {
                let target_model =
                    entity_field_str(target_agent, &["Model", "model"]).unwrap_or("");
                let target_provider =
                    entity_field_str(target_agent, &["Provider", "provider"]).unwrap_or("");
                let agent_config =
                    default_agent_config(api_url, api_key, target_provider, target_model);
                odata_post(
                    client,
                    &format!("{api_url}/tdata/AgentRoutes('{route_id}')/Paw.Channel.Register"),
                    tenant,
                    api_key,
                    serde_json::json!({
                        "binding_tier": "global",
                        "channel_id": "",
                        "guild_id": "",
                        "match_pattern": "",
                        "agent_config": agent_config.to_string(),
                        "agent_id": target_agent_id,
                    }),
                )
                .await
                .ok();
                tracing::info!(
                    "  Created global AgentRoute {route_id} with agent '{agent_name}' ({target_agent_id})"
                );
            }
        }
    }

    Ok(())
}

fn default_agent_config(
    api_url: &str,
    api_key: &Option<String>,
    llm_provider: &str,
    llm_model: &str,
) -> serde_json::Value {
    let mut config = serde_json::json!({
        "model": llm_model,
        "provider": llm_provider,
        "provider_options_json": "",
        "tools_enabled": DEFAULT_AGENT_TOOLS_ENABLED,
        "workdir": DEFAULT_AGENT_WORKDIR,
        "max_turns": "24",
        "temper_api_url": api_url,
        "max_follow_ups": "8",
    });
    if let Some(key) = api_key {
        config["temper_api_key"] = serde_json::Value::String(key.clone());
    }
    config
}

fn repaired_agent_config(
    raw: &str,
    api_url: &str,
    api_key: &Option<String>,
    is_global_route: bool,
) -> Option<String> {
    let original = raw.trim();
    let mut config = if original.is_empty() {
        serde_json::Map::new()
    } else {
        serde_json::from_str::<serde_json::Value>(original)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    };

    let original_normalized = serde_json::to_string(&config).ok();
    let normalized_tools = normalize_tools_enabled(
        config
            .get("tools_enabled")
            .and_then(|value| value.as_str())
            .unwrap_or(""),
        is_global_route,
    );
    let current_workdir = config
        .get("workdir")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();

    let needs_repair = is_global_route
        || normalized_tools.is_some()
        || original.is_empty()
        || normalize_legacy_workdir(&current_workdir).is_some();
    if !needs_repair {
        return None;
    }

    config.insert(
        "temper_api_url".to_string(),
        serde_json::Value::String(api_url.to_string()),
    );
    if let Some(key) = api_key {
        config.insert(
            "temper_api_key".to_string(),
            serde_json::Value::String(key.clone()),
        );
    }
    if let Some(normalized_workdir) = normalize_legacy_workdir(&current_workdir) {
        config.insert(
            "workdir".to_string(),
            serde_json::Value::String(normalized_workdir),
        );
    }
    if is_global_route {
        config.insert(
            "tools_enabled".to_string(),
            serde_json::Value::String(DEFAULT_AGENT_TOOLS_ENABLED.to_string()),
        );
    } else if let Some(tokens) = normalized_tools {
        config.insert(
            "tools_enabled".to_string(),
            serde_json::Value::String(tokens),
        );
    }

    let repaired = serde_json::to_string(&config).ok()?;
    if original == repaired || original_normalized.as_deref() == Some(&repaired) {
        None
    } else {
        Some(repaired)
    }
}

fn repair_default_agent_tools_enabled(raw: &str) -> Option<String> {
    let normalized = normalize_tools_enabled(raw, false);
    let mut changed = normalized.is_some();
    let source = normalized.as_deref().unwrap_or(raw);
    let mut tokens: Vec<String> = source
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect();

    for default_token in DEFAULT_AGENT_TOOLS_ENABLED
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        if !tokens.iter().any(|token| token == default_token) {
            tokens.push(default_token.to_string());
            changed = true;
        }
    }

    if changed {
        if tokens.is_empty() {
            Some(DEFAULT_AGENT_TOOLS_ENABLED.to_string())
        } else {
            Some(tokens.join(","))
        }
    } else {
        None
    }
}

fn normalize_tools_enabled(raw: &str, replace_all: bool) -> Option<String> {
    if raw.trim().is_empty() {
        return Some(DEFAULT_AGENT_TOOLS_ENABLED.to_string());
    }

    let mut changed = replace_all;
    let mut tokens = Vec::new();
    for token in raw
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let normalized = match token {
            "read_entity" => {
                changed = true;
                Some("temper_get")
            }
            "save_memory" => {
                changed = true;
                Some("temper_save_memory")
            }
            "recall_memory" => {
                changed = true;
                Some("temper_recall_memory")
            }
            "spawn_agent" => {
                changed = true;
                Some("temper_spawn_session")
            }
            "spawn_session" => {
                changed = true;
                Some("temper_spawn_session")
            }
            "temper_file_upload" => {
                changed = true;
                Some("temper_write")
            }
            "temper_get_agent_id" | "temper_done" | "temper_switch_provider" => {
                changed = true;
                None
            }
            other => Some(other),
        };

        if let Some(token) = normalized
            && !tokens.iter().any(|existing| existing == token)
        {
            tokens.push(token.to_string());
        }
    }

    if replace_all {
        return Some(DEFAULT_AGENT_TOOLS_ENABLED.to_string());
    }
    if changed {
        if tokens.is_empty() {
            Some(DEFAULT_AGENT_TOOLS_ENABLED.to_string())
        } else {
            Some(tokens.join(","))
        }
    } else {
        None
    }
}

fn normalize_legacy_workdir(current_workdir: &str) -> Option<String> {
    if current_workdir.is_empty() {
        return Some(DEFAULT_AGENT_WORKDIR.to_string());
    }

    if let Some(suffix) = current_workdir.strip_prefix("/tmp/workspace") {
        return Some(format!("{DEFAULT_AGENT_WORKDIR}{suffix}"));
    }

    if let Some(name) = current_workdir.strip_prefix("/tmp/temperpaw-") {
        return Some(format!("{DEFAULT_AGENT_WORKDIR}/temperpaw-{name}"));
    }

    None
}

/// OData GET helper with tenant + admin auth headers.
async fn odata_get(
    client: &reqwest::Client,
    url: &str,
    tenant: &str,
    api_key: &Option<String>,
) -> Result<serde_json::Value> {
    let mut req = client
        .get(url)
        .header("x-tenant-id", tenant)
        .header("x-temper-principal-kind", "admin");
    if let Some(key) = api_key {
        req = req.header("authorization", format!("Bearer {key}"));
    }
    let resp = req.send().await.context("OData GET failed")?;
    let status = resp.status();
    let body = resp.text().await.context("Failed to read response")?;
    if !status.is_success() {
        anyhow::bail!("OData GET {url} returned {status}: {body}");
    }
    serde_json::from_str(&body).context("Failed to parse JSON response")
}

/// OData POST helper with tenant + admin auth headers.
async fn odata_post(
    client: &reqwest::Client,
    url: &str,
    tenant: &str,
    api_key: &Option<String>,
    body: serde_json::Value,
) -> Result<serde_json::Value> {
    let mut req = client
        .post(url)
        .header("x-tenant-id", tenant)
        .header("x-temper-principal-kind", "admin")
        .header("content-type", "application/json")
        .json(&body);
    if let Some(key) = api_key {
        req = req.header("authorization", format!("Bearer {key}"));
    }
    let resp = req.send().await.context("OData POST failed")?;
    let status = resp.status();
    let text = resp.text().await.context("Failed to read response")?;
    if !status.is_success() {
        anyhow::bail!("OData POST {url} returned {status}: {text}");
    }
    Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
}

async fn odata_put_bytes(
    client: &reqwest::Client,
    url: &str,
    tenant: &str,
    api_key: &Option<String>,
    content_type: &str,
    body: Vec<u8>,
) -> Result<()> {
    let mut req = client
        .put(url)
        .header("x-tenant-id", tenant)
        .header("x-temper-principal-kind", "admin")
        .header("content-type", content_type)
        .body(body);
    if let Some(key) = api_key {
        req = req.header("authorization", format!("Bearer {key}"));
    }

    let resp = req.send().await.context("OData PUT failed")?;
    let status = resp.status();
    let text = resp.text().await.context("Failed to read PUT response")?;
    if !status.is_success() {
        anyhow::bail!("OData PUT {url} returned {status}: {text}");
    }
    Ok(())
}

async fn odata_get_text(
    client: &reqwest::Client,
    url: &str,
    tenant: &str,
    api_key: &Option<String>,
) -> Result<String> {
    let mut req = client
        .get(url)
        .header("x-tenant-id", tenant)
        .header("x-temper-principal-kind", "admin");
    if let Some(key) = api_key {
        req = req.header("authorization", format!("Bearer {key}"));
    }

    let resp = req.send().await.context("OData text GET failed")?;
    let status = resp.status();
    let body = resp.text().await.context("Failed to read text response")?;
    if !status.is_success() {
        anyhow::bail!("OData GET {url} returned {status}: {body}");
    }
    Ok(body)
}

fn entity_id_from_json(value: &serde_json::Value) -> Option<&str> {
    value
        .get("entity_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("Id").and_then(serde_json::Value::as_str))
        .or_else(|| {
            value
                .get("fields")
                .and_then(|fields| fields.get("Id"))
                .and_then(serde_json::Value::as_str)
        })
}

fn entity_field_str<'a>(value: &'a serde_json::Value, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                value
                    .get("fields")
                    .and_then(|fields| fields.get(*name))
                    .and_then(serde_json::Value::as_str)
            })
    })
}

fn build_missing_wasm_modules(os_apps_dir: &Path, startup_apps: &[String]) -> Result<()> {
    for build_script in wasm_build_scripts(os_apps_dir, startup_apps)? {
        let build_dir = build_script
            .parent()
            .context("build.sh path missing parent directory")?;
        if !wasm_build_needed(build_dir)? {
            continue;
        }

        tracing::info!(path = %build_script.display(), "Building local WASM modules");
        let script_name = build_script
            .file_name()
            .and_then(OsStr::to_str)
            .context("build.sh path missing file name")?;
        let status = std::process::Command::new("bash")
            .arg(script_name)
            .current_dir(build_dir)
            .status()
            .with_context(|| format!("Failed to run {}", build_script.display()))?;
        if !status.success() {
            anyhow::bail!("{} exited with status {status}", build_script.display());
        }
    }

    Ok(())
}

fn wasm_build_scripts(os_apps_dir: &Path, startup_apps: &[String]) -> Result<Vec<PathBuf>> {
    let mut scripts = Vec::new();
    let startup_app_set: HashSet<&str> = startup_apps.iter().map(String::as_str).collect();

    for app_entry in std::fs::read_dir(os_apps_dir)? {
        let app_dir = match app_entry {
            Ok(entry) if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) => entry.path(),
            _ => continue,
        };
        let app_name = app_dir
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default();
        if !startup_app_set.contains(app_name) {
            continue;
        }
        let wasm_dir = app_dir.join("wasm");
        if !wasm_dir.is_dir() {
            continue;
        }

        let root_build = wasm_dir.join("build.sh");
        if root_build.is_file() {
            scripts.push(root_build);
            continue;
        }

        for child in std::fs::read_dir(&wasm_dir)? {
            let child_dir = match child {
                Ok(entry) if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) => {
                    entry.path()
                }
                _ => continue,
            };
            let child_build = child_dir.join("build.sh");
            if child_build.is_file() {
                scripts.push(child_build);
            }
        }
    }

    scripts.sort();
    Ok(scripts)
}

fn wasm_build_needed(build_dir: &Path) -> Result<bool> {
    if build_dir.join("Cargo.toml").is_file() {
        let module_name = build_dir
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default();
        return Ok(find_wasm_binary(build_dir, module_name).is_none());
    }

    for child in std::fs::read_dir(build_dir)? {
        let child_dir = match child {
            Ok(entry) if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) => entry.path(),
            _ => continue,
        };
        if !child_dir.join("Cargo.toml").is_file() {
            continue;
        }

        let module_name = child_dir
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default();
        if find_wasm_binary(&child_dir, module_name).is_none() {
            return Ok(true);
        }
    }

    Ok(false)
}

fn find_wasm_binary(module_dir: &Path, module_name: &str) -> Option<PathBuf> {
    if module_name.is_empty() {
        return None;
    }

    // Check both wasm32-unknown-unknown and wasm32-wasip1 targets.
    // WASI modules (e.g., monty_repl) compile to wasip1; all others
    // use unknown-unknown. The Temper WASM engine auto-detects which
    // linker to use based on the module's imports.
    let release_dir = module_dir.join("target/wasm32-unknown-unknown/release");
    let wasi_release_dir = module_dir.join("target/wasm32-wasip1/release");
    let candidates = [
        release_dir.join(format!("{module_name}.wasm")),
        release_dir.join(format!("{}.wasm", module_name.replace('_', "-"))),
        wasi_release_dir.join(format!("{module_name}.wasm")),
        wasi_release_dir.join(format!("{}.wasm", module_name.replace('_', "-"))),
        module_dir.join(format!("{module_name}.wasm")),
        module_dir.join(format!("{}.wasm", module_name.replace('_', "-"))),
    ];

    candidates.into_iter().find(|path| path.is_file())
}

/// Spawn the webhook trigger (HTTP endpoint for external webhooks).
///
/// Listens on port+12 for POST /triggers/webhook/{route_key}.
/// ONE entity, ONE action — everything else is WASM integrations.
fn spawn_webhook_trigger(tenant: &str, port: u16, api_key: Option<String>) {
    use paw_transport::PawApiConfig;
    use paw_transport::webhook::{WebhookTrigger, WebhookTriggerConfig};

    let tenant = tenant.to_string();
    let api_url = format!("http://127.0.0.1:{port}");
    let trigger_port = port + 12;
    tracing::info!("Webhook trigger: listening on port {trigger_port} (tenant={tenant})");

    tokio::spawn(async move {
        let api = paw_transport::PawApiClient::new(PawApiConfig {
            base_url: api_url,
            tenant,
            api_key,
        });
        let config = WebhookTriggerConfig { port: trigger_port };
        let trigger = WebhookTrigger::new(config, api);
        if let Err(e) = trigger.run().await {
            tracing::error!("Webhook trigger fatal error: {e}");
        }
    });
}

fn actor_passivation_check_interval_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(60)
        .clamp(1, 86_400)
}

fn spawn_actor_passivation_loop(state: &PlatformState) {
    let interval_secs = actor_passivation_check_interval_secs(
        std::env::var("TEMPER_PASSIVATION_CHECK_INTERVAL")
            .ok()
            .as_deref(),
    );

    let server = state.server.clone();
    tokio::spawn(async move {
        // determinism-ok: background task for resource management
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await; // consume immediate tick

        loop {
            ticker.tick().await;
            server.passivate_idle_actors().await;
        }
    });
}

// Transport spawning is now handled by TransportManager (see transport_manager.rs).

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{Method, Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::any;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use anyhow::anyhow;
    use serde_json::Value;
    use temper_platform::recovery::InstalledAppsRuntimeRecoverySummary;
    use temper_runtime::tenant::TenantId;
    use temper_server::secrets::vault::SecretsVault;

    use super::{
        BootstrapAction, DEFAULT_AGENT_TOOLS_ENABLED, DEFAULT_GENESIS_BOOTSTRAP_TIMEOUT,
        DEFAULT_GENESIS_CACHE_RESTORE_TIMEOUT, LocalWasmStartupPolicy,
        OS_APP_RECONCILE_DURATION_METRIC, OS_APP_RECONCILE_TOTAL_METRIC, RuntimeRecoveryStep,
        STARTUP_LIVE_RESTORE_ENTITIES_METRIC, STARTUP_PHASE_DURATION_METRIC,
        STARTUP_TIME_TO_READY_METRIC, StartupReadiness, StartupSurfaceRuntimeRecoverySummary,
        WASM_MODULE_LOAD_FAILURES_METRIC, actor_passivation_check_interval_secs,
        app_required_wasm_failure, bootstrap_soul, classify_bootstrap_action,
        default_agent_specs_bootstrap_needed, genesis_bootstrap_app_names,
        genesis_bootstrap_timeout, genesis_cache_restore_timeout,
        installed_app_runtime_recovery_result, load_or_create_temper_api_key,
        local_wasm_startup_policy, orphaned_session_recovery_limit,
        paw_soul_content_is_personalized, repair_default_agent_tools_enabled,
        resolve_startup_secret, runtime_indexes_required_before_reconcile, runtime_recovery_plan,
        runtime_router_with_startup_gates, soul_lookup_filters, spawn_runtime_server,
        startup_discord_connect_result, startup_discord_summary_label, startup_os_apps,
        startup_surface_runtime_indexes_required_before_reconcile, wait_for_runtime_server,
    };
    use crate::transport_manager::TransportStatus;

    static ORPHANED_SESSION_ENV_LOCK: Mutex<()> = Mutex::new(());
    static GENESIS_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn empty_install_result() -> temper_platform::os_apps::InstallResult {
        temper_platform::os_apps::InstallResult {
            added: Vec::new(),
            updated: Vec::new(),
            skipped: Vec::new(),
            wasm_modules: Vec::new(),
            wasm_skipped: Vec::new(),
            wasm_failures: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            adrs_bootstrapped: Vec::new(),
            seed_instances: Vec::new(),
        }
    }

    #[test]
    fn actor_passivation_interval_defaults_and_clamps() {
        assert_eq!(actor_passivation_check_interval_secs(None), 60);
        assert_eq!(actor_passivation_check_interval_secs(Some("0")), 1);
        assert_eq!(actor_passivation_check_interval_secs(Some("garbage")), 60);
        assert_eq!(actor_passivation_check_interval_secs(Some("5")), 5);
        assert_eq!(
            actor_passivation_check_interval_secs(Some("999999")),
            86_400
        );
    }

    #[test]
    fn orphaned_session_recovery_is_enabled_by_default() {
        let _guard = ORPHANED_SESSION_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("TEMPERPAW_ORPHANED_SESSION_RECOVERY");
            std::env::remove_var("TEMPERPAW_ORPHANED_SESSION_RECOVERY_MAX");
        }

        assert_eq!(
            orphaned_session_recovery_limit(),
            Some(super::DEFAULT_ORPHANED_SESSION_RECOVERY_LIMIT)
        );
    }

    #[test]
    fn orphaned_session_recovery_can_be_explicitly_disabled() {
        let _guard = ORPHANED_SESSION_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("TEMPERPAW_ORPHANED_SESSION_RECOVERY", "false");
            std::env::remove_var("TEMPERPAW_ORPHANED_SESSION_RECOVERY_MAX");
        }

        assert_eq!(orphaned_session_recovery_limit(), None);

        unsafe {
            std::env::remove_var("TEMPERPAW_ORPHANED_SESSION_RECOVERY");
        }
    }

    #[test]
    fn runtime_recovery_populates_entity_indexes_before_post_boot_tasks() {
        let tenants = vec![TenantId::new("default"), TenantId::new("temper-system")];
        let plan = runtime_recovery_plan(&tenants);

        assert_eq!(
            plan,
            vec![
                RuntimeRecoveryStep::PopulateTypeIndex {
                    tenant: "default".to_string(),
                    entity_type: "App",
                },
                RuntimeRecoveryStep::PopulateTypeIndex {
                    tenant: "default".to_string(),
                    entity_type: "Agent",
                },
                RuntimeRecoveryStep::PopulateTypeIndex {
                    tenant: "default".to_string(),
                    entity_type: "Soul",
                },
                RuntimeRecoveryStep::PopulateTypeIndex {
                    tenant: "temper-system".to_string(),
                    entity_type: "App",
                },
                RuntimeRecoveryStep::PopulateTypeIndex {
                    tenant: "temper-system".to_string(),
                    entity_type: "Agent",
                },
                RuntimeRecoveryStep::PopulateTypeIndex {
                    tenant: "temper-system".to_string(),
                    entity_type: "Soul",
                },
            ]
        );
    }

    #[test]
    fn local_wasm_policy_defaults_and_overrides() {
        assert_eq!(
            local_wasm_startup_policy(Some("load-only")),
            LocalWasmStartupPolicy::LoadPersistedOnly
        );
        assert_eq!(
            local_wasm_startup_policy(Some("build")),
            LocalWasmStartupPolicy::BuildIfMissing
        );
        assert_eq!(
            local_wasm_startup_policy(Some("0")),
            LocalWasmStartupPolicy::LoadPersistedOnly
        );
        assert_eq!(
            local_wasm_startup_policy(Some("1")),
            LocalWasmStartupPolicy::BuildIfMissing
        );
        assert_eq!(
            local_wasm_startup_policy(None),
            LocalWasmStartupPolicy::LoadPersistedOnly
        );
    }

    #[test]
    fn startup_skips_builtin_default_agent_specs_when_paw_agent_owns_them() {
        assert!(!default_agent_specs_bootstrap_needed(&[
            "paw-fs".to_string(),
            "paw-agent".to_string(),
            "paw-channels".to_string(),
        ]));
        assert!(default_agent_specs_bootstrap_needed(&[
            "paw-fs".to_string(),
            "paw-channels".to_string(),
        ]));
    }

    #[test]
    fn genesis_bootstrap_refs_replace_local_startup_surface() {
        let _guard = GENESIS_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(
                "TEMPERPAW_GENESIS_BOOTSTRAP_REFS",
                "temperpaw/paw-agent@abc123, katagami/katagami-commons@def456",
            );
            std::env::remove_var("TEMPER_GENESIS_BOOTSTRAP_REFS");
        }

        assert_eq!(
            genesis_bootstrap_app_names(),
            vec!["paw-agent".to_string(), "katagami-commons".to_string()]
        );
        assert!(startup_os_apps().is_empty());

        unsafe {
            std::env::remove_var("TEMPERPAW_GENESIS_BOOTSTRAP_REFS");
        }
    }

    #[test]
    fn default_agent_tool_repair_adds_genesis_app_workflow_tokens() {
        let legacy_tools = "temper_create,temper_get,temper_list,temper_action,temper_patch,temper_submit_specs,temper_show_spec,temper_specs,temper_upload_wasm,temper_get_trajectories,temper_get_insights,temper_get_decisions,temper_poll_decision,temper_approve_decision,temper_deny_decision,temper_submit_policy,temper_list_policies,temper_get_policy,temper_update_policy,temper_delete_policy,temper_install_app,temper_list_apps,temper_spawn_session,temper_list_sessions,temper_abort_session,temper_steer_session,temper_save_memory,temper_recall_memory,temper_write,temper_read,temper_run_coding_agent,temper_get_secret,temper_datadog_query,temper_railway,temper_vercel,temper_web_search,temper_web_fetch,read,write,edit,bash";

        let repaired = repair_default_agent_tools_enabled(legacy_tools)
            .expect("legacy default Agent tools should be repaired");
        let tokens: Vec<&str> = repaired.split(',').collect();

        for required in [
            "temper_search_apps",
            "temper_publish_app",
            "temper_update_app",
            "temper_install_app",
            "temper_list_apps",
        ] {
            assert!(
                tokens.contains(&required),
                "repaired default Agent tools should contain {required}"
            );
        }
    }

    #[test]
    fn default_agent_tool_repair_is_noop_for_current_default() {
        assert_eq!(
            repair_default_agent_tools_enabled(DEFAULT_AGENT_TOOLS_ENABLED),
            None
        );
    }

    #[test]
    fn genesis_cache_restore_timeout_defaults_and_overrides() {
        let _guard = GENESIS_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("TEMPERPAW_GENESIS_CACHE_RESTORE_TIMEOUT_SECS");
        }
        assert_eq!(
            genesis_cache_restore_timeout(),
            DEFAULT_GENESIS_CACHE_RESTORE_TIMEOUT
        );

        unsafe {
            std::env::set_var("TEMPERPAW_GENESIS_CACHE_RESTORE_TIMEOUT_SECS", "3");
        }
        assert_eq!(genesis_cache_restore_timeout(), Duration::from_secs(3));

        unsafe {
            std::env::set_var("TEMPERPAW_GENESIS_CACHE_RESTORE_TIMEOUT_SECS", "0");
        }
        assert_eq!(
            genesis_cache_restore_timeout(),
            DEFAULT_GENESIS_CACHE_RESTORE_TIMEOUT
        );

        unsafe {
            std::env::remove_var("TEMPERPAW_GENESIS_CACHE_RESTORE_TIMEOUT_SECS");
        }
    }

    #[test]
    fn genesis_bootstrap_timeout_defaults_and_overrides() {
        let _guard = GENESIS_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("TEMPERPAW_GENESIS_BOOTSTRAP_TIMEOUT_SECS");
        }
        assert_eq!(
            genesis_bootstrap_timeout(),
            DEFAULT_GENESIS_BOOTSTRAP_TIMEOUT
        );

        unsafe {
            std::env::set_var("TEMPERPAW_GENESIS_BOOTSTRAP_TIMEOUT_SECS", "7");
        }
        assert_eq!(genesis_bootstrap_timeout(), Duration::from_secs(7));

        unsafe {
            std::env::set_var("TEMPERPAW_GENESIS_BOOTSTRAP_TIMEOUT_SECS", "0");
        }
        assert_eq!(
            genesis_bootstrap_timeout(),
            DEFAULT_GENESIS_BOOTSTRAP_TIMEOUT
        );

        unsafe {
            std::env::remove_var("TEMPERPAW_GENESIS_BOOTSTRAP_TIMEOUT_SECS");
        }
    }

    #[test]
    fn classify_bootstrap_keeps_installed_genesis_regardless_of_hash() {
        use temper_server::platform_store::InstalledAppRecord;
        let genesis_installed = |app_ref: &str| InstalledAppRecord {
            source_kind: "genesis".to_string(),
            status: "installed".to_string(),
            app_ref: app_ref.to_string(),
            ..Default::default()
        };

        // A Genesis install at the pin's hash -> keep.
        assert_eq!(
            classify_bootstrap_action(Some(&genesis_installed("owner/app@pin"))),
            BootstrapAction::KeepInstalled
        );
        // A Genesis install at a DIFFERENT hash (a newer agent-published version) -> keep.
        // Regression guard: the env pin is a FLOOR, not a ceiling; a redeploy must never
        // revert a newer install back to the pinned ref. Record-only: no runtime-readiness
        // probe (it is racy at boot — see D9), so the durable record alone decides.
        assert_eq!(
            classify_bootstrap_action(Some(&genesis_installed("owner/app@newer"))),
            BootstrapAction::KeepInstalled
        );
        // Absent -> install the pin (cold-start seed).
        assert_eq!(
            classify_bootstrap_action(None),
            BootstrapAction::InstallPinned
        );
        // Non-genesis source (e.g. a baked/local install) -> reinstall from the pin.
        assert_eq!(
            classify_bootstrap_action(Some(&InstalledAppRecord {
                source_kind: "local".to_string(),
                status: "installed".to_string(),
                ..Default::default()
            })),
            BootstrapAction::InstallPinned
        );
        // Genesis but not in the "installed" status (e.g. failed/pending) -> reinstall the pin.
        assert_eq!(
            classify_bootstrap_action(Some(&InstalledAppRecord {
                source_kind: "genesis".to_string(),
                status: "failed".to_string(),
                ..Default::default()
            })),
            BootstrapAction::InstallPinned
        );
    }

    #[test]
    fn installed_app_runtime_recovery_result_prioritizes_bounded_hot_path_state() {
        assert_eq!(
            installed_app_runtime_recovery_result(&InstalledAppsRuntimeRecoverySummary {
                ready: 4,
                ..InstalledAppsRuntimeRecoverySummary::default()
            }),
            "ready"
        );
        assert_eq!(
            installed_app_runtime_recovery_result(&InstalledAppsRuntimeRecoverySummary {
                healed: 2,
                ..InstalledAppsRuntimeRecoverySummary::default()
            }),
            "healed"
        );
        assert_eq!(
            installed_app_runtime_recovery_result(&InstalledAppsRuntimeRecoverySummary {
                needs_reconcile: 1,
                healed: 2,
                ..InstalledAppsRuntimeRecoverySummary::default()
            }),
            "needs_reconcile"
        );
        assert_eq!(
            installed_app_runtime_recovery_result(&InstalledAppsRuntimeRecoverySummary {
                store_error: 1,
                needs_reconcile: 1,
                ..InstalledAppsRuntimeRecoverySummary::default()
            }),
            "error"
        );
    }

    #[test]
    fn runtime_indexes_are_deferred_when_installed_apps_are_runtime_ready() {
        assert!(!runtime_indexes_required_before_reconcile(
            &InstalledAppsRuntimeRecoverySummary {
                ready: 6,
                ..InstalledAppsRuntimeRecoverySummary::default()
            }
        ));
        assert!(!runtime_indexes_required_before_reconcile(
            &InstalledAppsRuntimeRecoverySummary {
                ready: 4,
                healed: 2,
                ..InstalledAppsRuntimeRecoverySummary::default()
            }
        ));
        assert!(runtime_indexes_required_before_reconcile(
            &InstalledAppsRuntimeRecoverySummary {
                ready: 5,
                needs_reconcile: 1,
                ..InstalledAppsRuntimeRecoverySummary::default()
            }
        ));
        assert!(runtime_indexes_required_before_reconcile(
            &InstalledAppsRuntimeRecoverySummary {
                store_error: 1,
                ..InstalledAppsRuntimeRecoverySummary::default()
            }
        ));
    }

    #[test]
    fn runtime_indexes_are_deferred_when_startup_surface_is_ready_despite_global_reconcile_work() {
        let global = InstalledAppsRuntimeRecoverySummary {
            ready: 6,
            needs_reconcile: 6,
            ..InstalledAppsRuntimeRecoverySummary::default()
        };
        let startup_surface = StartupSurfaceRuntimeRecoverySummary {
            ready: 4,
            healed: 2,
            cold: 1,
            ..StartupSurfaceRuntimeRecoverySummary::default()
        };

        assert_eq!(
            installed_app_runtime_recovery_result(&global),
            "needs_reconcile"
        );
        assert!(!startup_surface_runtime_indexes_required_before_reconcile(
            &startup_surface
        ));
        assert!(startup_surface_runtime_indexes_required_before_reconcile(
            &StartupSurfaceRuntimeRecoverySummary {
                needs_reconcile: 1,
                ..StartupSurfaceRuntimeRecoverySummary::default()
            }
        ));
    }

    #[test]
    fn startup_discord_connect_result_keeps_success() {
        assert_eq!(
            startup_discord_connect_result(Ok("https://example.com/discord/interaction".into())),
            Some("https://example.com/discord/interaction".into())
        );
    }

    #[test]
    fn startup_discord_connect_result_drops_failure() {
        assert_eq!(startup_discord_connect_result(Err(anyhow!("boom"))), None);
    }

    #[test]
    fn startup_discord_summary_distinguishes_configured_from_connected() {
        assert_eq!(
            startup_discord_summary_label(false, &TransportStatus::Disconnected),
            None
        );
        assert_eq!(
            startup_discord_summary_label(true, &TransportStatus::Disconnected).as_deref(),
            Some("~ Discord configured; reconnect pending")
        );
        assert_eq!(
            startup_discord_summary_label(
                true,
                &TransportStatus::Connected {
                    guild_id: Some("guild-123".to_string())
                }
            )
            .as_deref(),
            Some("✓ Discord connected")
        );
    }

    #[test]
    fn startup_os_apps_includes_core_apps() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        temper_platform::os_apps::set_os_apps_dir(repo_root.join("os-apps"));
        let apps = startup_os_apps();
        for expected in ["paw-agent", "paw-channels", "paw-fs", "paw-research"] {
            assert!(
                apps.iter().any(|app| app == expected),
                "expected startup OS app {expected} to be present in {apps:?}"
            );
        }
    }

    #[test]
    fn startup_os_app_order_dedupes_shared_dependencies() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        temper_platform::os_apps::set_os_apps_dir(repo_root.join("os-apps"));
        let apps = startup_os_apps();
        let order = temper_platform::os_apps::resolve_os_app_install_order(&apps)
            .expect("startup OS app order should resolve");
        let unique = order.iter().collect::<std::collections::HashSet<_>>();

        assert_eq!(
            order.len(),
            unique.len(),
            "startup OS app reconcile order should not reinstall duplicate shared dependencies"
        );
        for app in apps {
            assert!(
                order.iter().any(|candidate| candidate == &app),
                "startup reconcile order should include requested app {app}"
            );
        }
    }

    #[test]
    fn paw_agent_manifest_declares_hot_session_wasm_startup_policy() {
        #[derive(serde::Deserialize)]
        struct AppManifest {
            #[serde(default)]
            wasm_modules: Vec<WasmModuleManifest>,
        }

        #[derive(serde::Deserialize)]
        struct WasmModuleManifest {
            name: String,
            startup_loading: String,
        }

        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest_path = repo_root.join("os-apps/paw-agent/app.toml");
        let manifest_source =
            std::fs::read_to_string(&manifest_path).expect("paw-agent app.toml should be readable");
        let manifest: AppManifest =
            toml::from_str(&manifest_source).expect("paw-agent app.toml should parse");
        let modules_by_name: std::collections::BTreeMap<_, _> = manifest
            .wasm_modules
            .into_iter()
            .map(|module| (module.name, module.startup_loading))
            .collect();

        for module in [
            "workspace_provisioner",
            "context_preparer",
            "provider_auth_gate",
            "provider_caller",
            "provider_response_applier",
            "agent_reply",
            "emit_ots_trajectory",
        ] {
            assert_eq!(
                modules_by_name.get(module).map(String::as_str),
                Some("eager"),
                "paw-agent app.toml must eagerly load hot Session module {module}"
            );
        }

        for module in ["monty_repl", "session_link_monitor", "session_recoverer"] {
            assert_eq!(
                modules_by_name.get(module).map(String::as_str),
                Some("lazy"),
                "paw-agent app.toml should keep non-hot Session module {module} lazy"
            );
        }
    }

    #[test]
    fn paw_agent_build_script_builds_session_recoverer() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let build_script_path = repo_root.join("os-apps/paw-agent/wasm/build.sh");
        let build_script = std::fs::read_to_string(&build_script_path)
            .expect("paw-agent wasm build.sh should be readable");
        let build_tokens: std::collections::BTreeSet<_> = build_script
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .collect();

        assert!(
            build_tokens.contains("session_recoverer"),
            "paw-agent wasm build.sh must build session_recoverer"
        );
    }

    #[test]
    fn paw_channels_manifest_declares_route_reply_wasm_startup_policy() {
        #[derive(serde::Deserialize)]
        struct AppManifest {
            #[serde(default)]
            wasm_modules: Vec<WasmModuleManifest>,
        }

        #[derive(serde::Deserialize)]
        struct WasmModuleManifest {
            name: String,
            criticality: String,
            startup_loading: String,
        }

        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest_path = repo_root.join("os-apps/paw-channels/app.toml");
        let manifest_source = std::fs::read_to_string(&manifest_path)
            .expect("paw-channels app.toml should be readable");
        let manifest: AppManifest =
            toml::from_str(&manifest_source).expect("paw-channels app.toml should parse");
        let modules_by_name: std::collections::BTreeMap<_, _> = manifest
            .wasm_modules
            .into_iter()
            .map(|module| (module.name, (module.criticality, module.startup_loading)))
            .collect();

        for module in ["channel_connect", "route_message", "send_reply"] {
            let Some((criticality, startup_loading)) = modules_by_name.get(module) else {
                panic!("paw-channels app.toml must declare hot route/reply module {module}");
            };
            assert_eq!(
                criticality.as_str(),
                "app-required",
                "paw-channels hot route/reply module {module} must be app-required"
            );
            assert_eq!(
                startup_loading.as_str(),
                "eager",
                "paw-channels hot route/reply module {module} must eagerly load"
            );
        }

        assert_eq!(
            modules_by_name
                .get("transport_reconcile")
                .map(|(_, startup_loading)| startup_loading.as_str()),
            Some("lazy"),
            "transport_reconcile should stay lazy because it is not on the normal route/reply path"
        );
    }

    #[test]
    fn startup_metric_names_match_datadog_contract() {
        assert_eq!(
            STARTUP_PHASE_DURATION_METRIC,
            "temper_startup_phase_duration_ms"
        );
        assert_eq!(
            STARTUP_TIME_TO_READY_METRIC,
            "temper_startup_time_to_healthy_ms"
        );
        assert_eq!(
            STARTUP_LIVE_RESTORE_ENTITIES_METRIC,
            "temper_startup_live_restore_entities_total"
        );
        assert_eq!(
            OS_APP_RECONCILE_TOTAL_METRIC,
            "temper_os_app_reconcile_total"
        );
        assert_eq!(
            OS_APP_RECONCILE_DURATION_METRIC,
            "temper_os_app_reconcile_duration_ms"
        );
        assert_eq!(
            WASM_MODULE_LOAD_FAILURES_METRIC,
            "temper_wasm_module_load_failures_total"
        );
    }

    #[test]
    fn required_wasm_failures_block_readiness() {
        let mut install = empty_install_result();
        install.wasm_failures = vec!["route_message".to_string(), "send_reply".to_string()];

        let error = app_required_wasm_failure("paw-channels", &install)
            .expect("app-required WASM failures should block startup readiness");

        assert!(error.contains("paw-channels"));
        assert!(error.contains("route_message, send_reply"));
    }

    #[test]
    fn installed_apps_without_wasm_failures_do_not_block_readiness() {
        let install = empty_install_result();

        assert!(app_required_wasm_failure("paw-fs", &install).is_none());
    }

    #[test]
    fn sandbox_secret_resolution_prefers_deploy_values_over_tenant_overrides() {
        let vault = SecretsVault::new(&[7u8; 32]);
        vault
            .cache_secret(
                "default",
                "modal_token_id",
                "stale-tenant-token".to_string(),
            )
            .unwrap();
        vault
            .cache_platform_secret("modal_token_id", "fresh-platform-token".to_string())
            .unwrap();

        let resolved = resolve_startup_secret(
            Some(&vault),
            "default",
            "modal_token_id",
            Some("fresh-env-token".to_string()),
        );

        assert_eq!(resolved.as_deref(), Some("fresh-env-token"));
    }

    #[test]
    fn sandbox_secret_resolution_prefers_platform_cache_over_tenant_overrides() {
        let vault = SecretsVault::new(&[8u8; 32]);
        vault
            .cache_secret(
                "default",
                "modal_token_id",
                "stale-tenant-token".to_string(),
            )
            .unwrap();
        vault
            .cache_platform_secret("modal_token_id", "fresh-platform-token".to_string())
            .unwrap();

        let resolved = resolve_startup_secret(Some(&vault), "default", "modal_token_id", None);

        assert_eq!(resolved.as_deref(), Some("fresh-platform-token"));
    }

    #[test]
    fn sandbox_secret_resolution_falls_back_to_tenant_override_without_deploy_values() {
        let vault = SecretsVault::new(&[9u8; 32]);
        vault
            .cache_secret("default", "modal_token_id", "tenant-token".to_string())
            .unwrap();

        let resolved = resolve_startup_secret(Some(&vault), "default", "modal_token_id", None);

        assert_eq!(resolved.as_deref(), Some("tenant-token"));
    }

    #[test]
    fn datadog_configs_use_tenant_aware_entity_queries() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dashboard_path = repo_root.join("dd-dashboards/temperpaw-overview.json");
        let monitor_path = repo_root.join("dd-monitors/temperpaw-monitors.json");

        let dashboard: Value =
            serde_json::from_str(&std::fs::read_to_string(&dashboard_path).unwrap()).unwrap();
        let monitors: Value =
            serde_json::from_str(&std::fs::read_to_string(&monitor_path).unwrap()).unwrap();

        let indexed_entities_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if matches!(
                        definition["title"].as_str()?,
                        "Indexed Entities" | "Indexed Entities (Query Plane)"
                    ) {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Entity count widget query should exist");
        assert_eq!(
            indexed_entities_query,
            "sum:temper_indexed_entities{service:temperpaw,tenant:*}"
        );

        let active_actors_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if matches!(
                        definition["title"].as_str()?,
                        "Active Actors" | "Active Actors (Hydrated)"
                    ) {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Active Actors widget query should exist");
        assert_eq!(
            active_actors_query,
            "avg:temper_active_actors{service:temperpaw}"
        );

        let process_memory_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if matches!(
                        definition["title"].as_str()?,
                        "Process Memory (RSS)" | "TemperPaw Process Memory (RSS)"
                    ) {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Process Memory widget query should exist");
        assert_eq!(
            process_memory_query,
            "avg:process_resident_memory_bytes{service:temperpaw}"
        );

        let indexed_entities_by_host_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "Indexed Entities by Host" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Indexed Entities by Host widget query should exist");
        assert_eq!(
            indexed_entities_by_host_query,
            "sum:temper_indexed_entities{service:temperpaw,tenant:*} by {host}"
        );

        let active_actors_by_host_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "Active Actors by Host" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Active Actors by Host widget query should exist");
        assert_eq!(
            active_actors_by_host_query,
            "avg:temper_active_actors{service:temperpaw} by {host}"
        );

        let process_memory_by_host_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "TemperPaw RSS by Host" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("TemperPaw RSS by Host widget query should exist");
        assert_eq!(
            process_memory_by_host_query,
            "avg:process_resident_memory_bytes{service:temperpaw} by {host}"
        );

        let projected_entities_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if matches!(
                        definition["title"].as_str()?,
                        "Projected Entities" | "Projected Entities (Durable Catalog)"
                    ) {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Projected Entities widget query should exist");
        assert_eq!(
            projected_entities_query,
            "sum:temper_projected_entities{service:temperpaw,tenant:*}"
        );

        let projection_coverage_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "Projection Coverage" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Projection Coverage widget query should exist");
        assert_eq!(
            projection_coverage_query,
            "avg:temper_projection_coverage_ratio{service:temperpaw}"
        );

        let snapshot_miss_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "Projection Snapshot Misses" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Projection Snapshot Misses widget query should exist");
        assert_eq!(
            snapshot_miss_query,
            "default_zero(sum:temper_projection_backfill_snapshot_misses_total{service:temperpaw}.as_count().rollup(sum, 60))"
        );

        let reconcile_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "OS App Reconcile" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("OS App Reconcile widget query should exist");
        assert_eq!(
            reconcile_query,
            "default_zero(sum:temper_os_app_reconcile_total{service:temperpaw} by {app,result}.as_count().rollup(sum, 60))"
        );

        let reconcile_duration_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "OS App Reconcile Duration" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("OS App Reconcile Duration widget query should exist");
        assert_eq!(
            reconcile_duration_query,
            "default_zero(avg:temper_os_app_reconcile_duration_ms{service:temperpaw} by {app,result}.rollup(avg, 60))"
        );

        let startup_restore_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "Startup Live Restore Entities" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Startup Live Restore Entities widget query should exist");
        assert_eq!(
            startup_restore_query,
            "default_zero(sum:temper_startup_live_restore_entities_total{service:temperpaw} by {tenant}.as_count().rollup(sum, 60))"
        );

        let session_context_tokens_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "Session Context Tokens" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Session Context Tokens widget query should exist");
        assert_eq!(
            session_context_tokens_query,
            "avg:temper_session_context_tokens{service:temperpaw} by {provider}.rollup(avg, 60)"
        );

        let session_context_bytes_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "Session Context Bytes" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Session Context Bytes widget query should exist");
        assert_eq!(
            session_context_bytes_query,
            "avg:temper_session_context_bytes{service:temperpaw} by {provider}.rollup(avg, 60)"
        );

        let provider_request_bytes_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "Provider Request Bytes" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Provider Request Bytes widget query should exist");
        assert_eq!(
            provider_request_bytes_query,
            "avg:temper_session_provider_request_bytes{service:temperpaw} by {provider}.rollup(avg, 60)"
        );

        let memory_budget_exceeded_query = dashboard["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|widget| {
                let widgets = widget["definition"]["widgets"].as_array()?;
                widgets.iter().find_map(|inner| {
                    let definition = &inner["definition"];
                    if definition["title"].as_str()? == "Session Memory Budget Exceeded" {
                        definition["requests"][0]["q"].as_str()
                    } else {
                        None
                    }
                })
            })
            .expect("Session Memory Budget Exceeded widget query should exist");
        assert_eq!(
            memory_budget_exceeded_query,
            "default_zero(sum:temper_session_memory_limit_exceeded_total{service:temperpaw}.as_count().rollup(sum, 60))"
        );

        let indexed_entities_drop_query = monitors
            .as_array()
            .unwrap()
            .iter()
            .find_map(|monitor| {
                if monitor["name"].as_str()? == "[Temper] Indexed Entities Drop" {
                    monitor["query"].as_str()
                } else {
                    None
                }
            })
            .expect("Indexed Entities Drop monitor query should exist");
        assert_eq!(
            indexed_entities_drop_query,
            "avg(last_15m):sum:temper_indexed_entities{service:temperpaw,tenant:*} < 1"
        );

        let startup_regression_query = monitors
            .as_array()
            .unwrap()
            .iter()
            .find_map(|monitor| {
                if monitor["name"].as_str()? == "[Temper] Startup Time Regression" {
                    monitor["query"].as_str()
                } else {
                    None
                }
            })
            .expect("Startup Time Regression monitor query should exist");
        assert_eq!(
            startup_regression_query,
            "avg(last_15m):avg:temper_startup_time_to_healthy_ms{service:temperpaw} > 120000"
        );

        let reconcile_regression_query = monitors
            .as_array()
            .unwrap()
            .iter()
            .find_map(|monitor| {
                if monitor["name"].as_str()? == "[Temper] OS App Reconcile Regression" {
                    monitor["query"].as_str()
                } else {
                    None
                }
            })
            .expect("OS App Reconcile Regression monitor query should exist");
        assert_eq!(
            reconcile_regression_query,
            "avg(last_1h):avg:temper_startup_phase_duration_ms{service:temperpaw,phase:phase_6_os_app_reconcile} > 60000"
        );

        let wasm_failure_monitor_query = monitors
            .as_array()
            .unwrap()
            .iter()
            .find_map(|monitor| {
                if monitor["name"].as_str()? == "[Temper] Required WASM Load Failures" {
                    monitor["query"].as_str()
                } else {
                    None
                }
            })
            .expect("Required WASM Load Failures monitor query should exist");
        assert_eq!(
            wasm_failure_monitor_query,
            "sum(last_15m):default_zero(sum:temper_wasm_module_load_failures_total{service:temperpaw}.as_count()) > 0"
        );

        let session_memory_monitor_query = monitors
            .as_array()
            .unwrap()
            .iter()
            .find_map(|monitor| {
                if monitor["name"].as_str()? == "[Temper] Session Memory Budget Exceeded" {
                    monitor["query"].as_str()
                } else {
                    None
                }
            })
            .expect("Session Memory Budget Exceeded monitor query should exist");
        assert_eq!(
            session_memory_monitor_query,
            "sum(last_15m):default_zero(sum:temper_session_memory_limit_exceeded_total{service:temperpaw}.as_count()) > 0"
        );

        let dashboard_json = dashboard.to_string();
        let monitors_json = monitors.to_string();
        assert!(
            monitors_json.contains(
                "sum(last_15m):default_zero(sum:temper_session_phase_budget_exceeded_total{service:temperpaw}.as_count()) >= 1"
            ),
            "Monitors should alert on session phase budget failures."
        );
        assert!(
            monitors_json.contains(
                "sum(last_15m):default_zero(sum:temper_query_projection_update_error_total{service:temperpaw}.as_count()) >= 1"
            ),
            "Monitors should alert on background query projection update errors."
        );
        assert!(
            dashboard_json.contains("avg:temper_up{service:temperpaw}"),
            "Dashboard should include the metrics pipeline canary."
        );
        assert!(
            dashboard_json.contains(
                "sum:temper_cedar_evaluations_total{service:temperpaw}.as_count().rollup(sum, 60)"
            ),
            "Dashboard should include Cedar evaluation volume."
        );
        assert!(
            dashboard_json.contains(
                "avg:temper_turso_query_duration{service:temperpaw} by {operation}.rollup(avg, 60)"
            ),
            "Dashboard should include Turso query duration."
        );
        assert!(
            dashboard_json.contains(
                "avg:temper_query_projection_update_duration_ms{service:temperpaw} by {operation,result}.rollup(avg, 60)"
            ),
            "Dashboard should include background query projection update duration."
        );
        assert!(
            dashboard_json.contains(
                "default_zero(sum:temper_query_projection_update_error_total{service:temperpaw} by {operation}.as_count().rollup(sum, 60))"
            ),
            "Dashboard should include background query projection update errors."
        );
        assert!(
            dashboard_json.contains(
                "sum:temper_wasm_host_http_requests_total{service:temperpaw} by {call_kind,status_code_class}.as_count().rollup(sum, 60)"
            ),
            "Dashboard should include WASM host HTTP request volume."
        );
        assert!(
            dashboard_json.contains(
                "avg:temper_wasm_host_http_duration_ms{service:temperpaw} by {call_kind,status_code_class}.rollup(avg, 60)"
            ),
            "Dashboard should include WASM host HTTP latency."
        );
        assert!(
            dashboard_json.contains(
                "avg:temper_event_replay_duration{service:temperpaw} by {tenant,entity_type}.rollup(avg, 60)"
            ),
            "Dashboard should include event replay duration."
        );
        assert!(
            dashboard_json.contains(
                "avg:temper_session_context_prepare_duration_ms{service:temperpaw}.rollup(avg, 60)"
            ),
            "Dashboard should include session context prepare duration."
        );
        assert!(
            dashboard_json.contains(
                "avg:temper_session_phase_duration_ms{service:temperpaw} by {phase,result}.rollup(avg, 60)"
            ),
            "Dashboard should include session phase duration."
        );
        assert!(
            dashboard_json.contains(
                "default_zero(sum:temper_session_phase_budget_exceeded_total{service:temperpaw} by {phase,last_step}.as_count().rollup(sum, 60))"
            ),
            "Dashboard should include session phase budget failures."
        );
        assert!(
            dashboard_json.contains(
                "avg:temper_session_provider_response_bytes{service:temperpaw} by {provider}.rollup(avg, 60)"
            ),
            "Dashboard should include provider response bytes."
        );
        assert!(
            dashboard_json.contains("temper_startup_phase_duration_ms"),
            "Dashboard should include startup phase duration."
        );
        assert!(
            dashboard_json.contains("temper_startup_time_to_healthy_ms"),
            "Dashboard should include startup time to ready."
        );
        assert!(
            dashboard_json.contains("temper_wasm_module_load_failures_total"),
            "Dashboard should include required WASM load failures."
        );
        assert!(
            !dashboard_json.contains("temper_wasm_module_skipped_total"),
            "Dashboard should not reference the stale WASM skipped metric."
        );
    }

    #[test]
    fn soul_lookup_filters_cover_current_and_legacy_names() {
        assert_eq!(
            soul_lookup_filters("Paw"),
            ["Name eq 'Paw'".to_string(), "name eq 'paw'".to_string()]
        );
        assert_eq!(
            soul_lookup_filters("SRE"),
            ["Name eq 'SRE'".to_string(), "name eq 'sre'".to_string()]
        );
    }

    #[test]
    fn temper_api_key_persists_and_env_overrides() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("api.key");

        let generated = load_or_create_temper_api_key(None, &path).unwrap();
        assert!(!generated.is_empty());
        assert!(path.exists());

        let reloaded = load_or_create_temper_api_key(None, &path).unwrap();
        assert_eq!(reloaded, generated);

        let explicit = load_or_create_temper_api_key(Some("env-token".to_string()), &path).unwrap();
        assert_eq!(explicit, "env-token");
    }

    #[test]
    fn paw_soul_content_personalization_detection_matches_non_default_content() {
        let default_content = crate::setup::default_paw_soul_content().expect("default content");

        assert!(!paw_soul_content_is_personalized(
            &default_content,
            &default_content
        ));
        assert!(paw_soul_content_is_personalized(
            "## Who I Am\nI am tailored for Arni.",
            &default_content
        ));
    }

    #[tokio::test]
    async fn bootstrap_soul_preserves_existing_personalized_paw_content() {
        #[derive(Clone, Default)]
        struct Seen {
            upload_attempted: Arc<Mutex<bool>>,
        }

        async fn handler(State(seen): State<Seen>, request: Request<Body>) -> impl IntoResponse {
            match (
                request.method(),
                request.uri().path(),
                request.uri().query(),
            ) {
                (&Method::GET, "/tdata/Agents('agent-1')", _) => (
                    StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "fields": {
                            "soul_id": "soul-1"
                        }
                    })),
                )
                    .into_response(),
                (&Method::GET, "/tdata/Souls('soul-1')", _) => (
                    StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "fields": {
                            "ContentFileId": "file-1"
                        }
                    })),
                )
                    .into_response(),
                (&Method::GET, "/tdata/Files('file-1')/$value", _) => (
                    StatusCode::OK,
                    "## Who I Am\nI am tailored for Arni.".to_string(),
                )
                    .into_response(),
                (&Method::PUT, "/tdata/Files('file-1')/$value", _) => {
                    *seen.upload_attempted.lock().unwrap() = true;
                    StatusCode::OK.into_response()
                }
                _ => StatusCode::NOT_FOUND.into_response(),
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let soul_path = temp.path().join("SOUL.md");
        std::fs::write(&soul_path, "# Default soul").unwrap();

        let seen = Seen::default();
        let app = Router::new()
            .fallback(any(handler))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let soul_id = bootstrap_soul(
            &reqwest::Client::new(),
            &format!("http://{addr}"),
            "default",
            &None,
            "agent-1",
            "Paw",
            "Paw soul",
            &[soul_path.to_str().unwrap()],
            false,
        )
        .await
        .unwrap();

        assert_eq!(soul_id, "soul-1");
        assert!(!*seen.upload_attempted.lock().unwrap());
    }

    #[tokio::test]
    async fn spawn_runtime_server_accepts_requests_before_transport_boot() {
        use axum::{Router, routing::get};
        use std::time::Duration;

        let app = Router::new().route("/readyz", get(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_handle = spawn_runtime_server(listener, app);
        wait_for_runtime_server(
            format!("http://{addr}/readyz").as_str(),
            Duration::from_secs(2),
        )
        .await
        .expect("runtime server should be reachable before transport boot");

        server_handle.abort();
    }

    #[tokio::test]
    async fn startup_gates_keep_liveness_up_while_readiness_stays_blocked() {
        use axum::{Router, routing::get};

        let readiness = StartupReadiness::default();
        let app = runtime_router_with_startup_gates(
            Router::new()
                .route("/healthz", get(|| async { StatusCode::OK }))
                .route("/probe", get(|| async { StatusCode::OK })),
            readiness.clone(),
            None,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_handle = spawn_runtime_server(listener, app);
        let client = reqwest::Client::new();

        let health = client
            .get(format!("http://{addr}/healthz"))
            .send()
            .await
            .expect("healthz request");
        assert_eq!(health.status(), StatusCode::OK);

        let ready = client
            .get(format!("http://{addr}/readyz"))
            .send()
            .await
            .expect("readyz request");
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);

        let probe = client
            .get(format!("http://{addr}/probe"))
            .send()
            .await
            .expect("probe request");
        assert_eq!(probe.status(), StatusCode::SERVICE_UNAVAILABLE);

        readiness.mark_ready();

        let ready = client
            .get(format!("http://{addr}/readyz"))
            .send()
            .await
            .expect("readyz request after mark_ready");
        assert_eq!(ready.status(), StatusCode::OK);

        let probe = client
            .get(format!("http://{addr}/probe"))
            .send()
            .await
            .expect("probe request after mark_ready");
        assert_eq!(probe.status(), StatusCode::OK);

        server_handle.abort();
    }
}
