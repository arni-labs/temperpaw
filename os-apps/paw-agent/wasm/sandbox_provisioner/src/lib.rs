//! Sandbox Provisioner — WASM module for provisioning sandboxes via provider abstraction.
//!
//! Used by the Resume flow to provision a sandbox from a known URL.
//! For new sessions, sandbox provisioning is lazy (ADR-0022) — handled
//! by monty_repl/dispatch.rs on first sandbox tool call.
//!
//! Priority order:
//! 1. sandbox_url from entity state (set via Configure — testing override)
//! 2. sandbox_url from integration config (explicit override)
//! 3. Provider-based creation (Tensorlake, Modal, etc.)
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;
use wasm_helpers::resolve_temper_api_url;
use wasm_helpers::sandbox::{self, SandboxConfig, SandboxHandle};

/// Entry point — used by the Resume flow to provision a sandbox for a restored session.
/// For new sessions, sandbox provisioning is lazy (ADR-0022).
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        // Tear down the session's sandbox on terminal transitions so compute
        // is never leaked (sessions provision lazily and previously nothing
        // ever deleted the sandbox). Decided by SESSION STATUS, not trigger
        // config — trigger config resolution proved unreliable across
        // actions, and provisioning at a terminal state is never correct.
        // Best-effort — a destroy failure must not fail the transition.
        let session_status = ctx
            .entity_state
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if matches!(session_status, "Completed" | "Failed" | "Cancelled")
            || ctx.config.get("mode").map(String::as_str) == Some("release")
        {
            release_sandbox(&ctx);
            return Ok(());
        }

        ctx.log("info", "sandbox_provisioner: starting (Resume flow)");

        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

        // Provision sandbox or schedule a later readiness check if the
        // sandbox still needs time to boot.
        let status = provision_sandbox(&ctx, &fields)?;
        let handle = match status {
            ProvisionStatus::Pending(handle) => {
                set_success_result(
                    "ProvisionPending",
                    &json!({
                        "sandbox_url": handle.sandbox_url,
                        "sandbox_id": handle.sandbox_id,
                        "sandbox_provider": handle.provider,
                    }),
                );
                return Ok(());
            }
            ProvisionStatus::Ready(handle) => handle,
        };
        ctx.log(
            "info",
            &format!(
                "sandbox_provisioner: sandbox ready at {} (provider: {})",
                handle.sandbox_url, handle.provider
            ),
        );

        // Run post-provisioning setup (gh CLI etc.) — non-fatal
        sandbox::sandbox_setup(&ctx, &handle);

        // Return sandbox details to the state machine.
        set_success_result(
            "SandboxReady",
            &json!({
                "sandbox_url": handle.sandbox_url,
                "sandbox_id": handle.sandbox_id,
                "sandbox_provider": handle.provider,
            }),
        );

        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

enum ProvisionStatus {
    Pending(SandboxHandle),
    Ready(SandboxHandle),
}

/// Release the session's sandbox on a terminal transition. Reads the sandbox
/// handle from entity fields; a session that never provisioned one (or used a
/// static URL) is a no-op. Never propagates errors — logs and reports status.
fn release_sandbox(ctx: &Context) {
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let sandbox_id = fields
        .get("sandbox_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let provider = fields
        .get("sandbox_provider")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if sandbox_id.is_empty() || sandbox_id == "static-sandbox" || provider == "static" {
        set_success_result("", &json!({"status": "noop", "reason": "no provisioned sandbox"}));
        return;
    }
    let handle = SandboxHandle {
        sandbox_url: fields
            .get("sandbox_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        sandbox_id: sandbox_id.to_string(),
        provider: if provider.is_empty() {
            "tensorlake".to_string()
        } else {
            provider.to_string()
        },
    };
    match sandbox::sandbox_destroy(ctx, &handle) {
        Ok(()) => {
            ctx.log(
                "info",
                &format!(
                    "sandbox_provisioner: released sandbox {} ({})",
                    handle.sandbox_id, handle.provider
                ),
            );
            set_success_result("", &json!({"status": "released", "sandbox_id": handle.sandbox_id}));
        }
        Err(error) => {
            ctx.log(
                "warn",
                &format!(
                    "sandbox_provisioner: failed to release sandbox {} ({}): {error}",
                    handle.sandbox_id, handle.provider
                ),
            );
            set_success_result("", &json!({"status": "release_failed", "sandbox_id": handle.sandbox_id, "error": error}));
        }
    }
}

/// Provision a sandbox. Priority order:
/// 1. sandbox_url from entity state (set via Configure action) or integration config
/// 2. Provider-based creation (Tensorlake, Modal, etc.)
/// 3. Fail with setup guidance
fn provision_sandbox(ctx: &Context, fields: &Value) -> Result<ProvisionStatus, String> {
    // Retry path: sandbox was already created on a previous invocation;
    // only readiness checking remains.
    let existing_sandbox_url = fields
        .get("sandbox_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let existing_sandbox_id = fields
        .get("sandbox_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let existing_provider = fields
        .get("sandbox_provider")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    if let (Some(sandbox_url), Some(sandbox_id)) =
        (existing_sandbox_url, existing_sandbox_id.clone())
    {
        let provider = existing_provider.clone().unwrap_or_else(|| {
            sandbox::resolve_sandbox_provider(ctx, fields)
                .unwrap_or_else(|_| "tensorlake".to_string())
        });
        let handle = SandboxHandle {
            sandbox_url,
            sandbox_id,
            provider,
        };
        return check_sandbox_ready(ctx, fields, handle);
    }

    // Priority 1: sandbox_url from Configure-time state or integration config.
    let static_url = fields
        .get("sandbox_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .map(|s| s.to_string())
        .or_else(|| {
            ctx.config
                .get("sandbox_url")
                .filter(|s| !s.is_empty() && !s.contains("{secret:"))
                .cloned()
        })
        .or_else(|| {
            ctx.trigger_params
                .get("sandbox_url")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty() && !s.contains("{secret:"))
                .map(|s| s.to_string())
        });
    if let Some(url) = static_url {
        ctx.log(
            "info",
            &format!("sandbox_provisioner: using static sandbox_url: {url}"),
        );
        return Ok(ProvisionStatus::Ready(SandboxHandle {
            sandbox_url: url,
            sandbox_id: "static-sandbox".to_string(),
            provider: "static".to_string(),
        }));
    }

    // Priority 2: Provider-based creation.
    let provider = sandbox::resolve_sandbox_provider(ctx, fields)?;
    ctx.log(
        "info",
        &format!("sandbox_provisioner: provisioning via {provider} provider"),
    );
    let config = sandbox::sandbox_config_from_fields(fields);
    let handle = sandbox::sandbox_create(ctx, &provider, &config)?;
    check_sandbox_ready(ctx, fields, handle)
}

fn check_sandbox_ready(
    ctx: &Context,
    fields: &Value,
    handle: SandboxHandle,
) -> Result<ProvisionStatus, String> {
    match sandbox::sandbox_health_check(ctx, &handle) {
        Ok(true) => {
            ctx.log(
                "info",
                &format!(
                    "sandbox_provisioner: sandbox ready: id={}, url={}, provider={}",
                    handle.sandbox_id, handle.sandbox_url, handle.provider
                ),
            );
            Ok(ProvisionStatus::Ready(handle))
        }
        Ok(false) => {
            let check_count = fields
                .get("provision_check_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let max_checks = fields
                .get("max_provision_checks")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(24)
                .max(1);
            if check_count >= max_checks {
                Err(format!(
                    "sandbox {} did not become ready within {} retries",
                    handle.sandbox_id, max_checks
                ))
            } else {
                let temper_api_url = resolve_temper_api_url(ctx, fields);
                ctx.log(
                    "info",
                    &format!(
                        "sandbox_provisioner: sandbox {} not ready yet, scheduling check {}/{} via {}",
                        handle.sandbox_id,
                        check_count + 1,
                        max_checks,
                        temper_api_url
                    ),
                );
                Ok(ProvisionStatus::Pending(handle))
            }
        }
        Err(e) => {
            let check_count = fields
                .get("provision_check_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let max_checks = fields
                .get("max_provision_checks")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(24)
                .max(1);
            if check_count >= max_checks {
                Err(format!(
                    "sandbox {} did not become ready within {} retries: {}",
                    handle.sandbox_id, max_checks, e
                ))
            } else {
                ctx.log(
                    "info",
                    &format!(
                        "sandbox_provisioner: readiness check failed ({}), scheduling retry {}/{}",
                        e,
                        check_count + 1,
                        max_checks
                    ),
                );
                Ok(ProvisionStatus::Pending(handle))
            }
        }
    }
}
