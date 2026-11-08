//! Opening the vault's stored secrets at boot, and the previous master key a
//! key rotation opens old rows with (temper ADR-0075).
//!
//! A rotation boots with the new key in `TEMPER_VAULT_KEY` and the old one in
//! `TEMPER_VAULT_KEY_PREVIOUS`. Temper's own load path opens each row with
//! either key and seals rows still under the old key again under the new one,
//! so once a boot has re-sealed them the old key can be dropped.

use std::collections::BTreeSet;

use anyhow::{Result, anyhow, bail};
use temper_platform::PlatformState;
use temper_server::state::SecretLoadReport;

/// The variable naming the master key a rotation replaces.
pub(crate) const PREVIOUS_KEY_ENV: &str = "TEMPER_VAULT_KEY_PREVIOUS";

/// The previous master key, when a rotation names one.
///
/// Rows it opens are sealed again under the current key, so the current key
/// must come from a valid `TEMPER_VAULT_KEY`: a generated or file key would
/// strand those rows at the next boot without it. Either problem stops the
/// boot, as does a malformed previous key, which would leave old rows unread.
pub(crate) fn previous_vault_key(
    current: Option<&str>,
    previous: Option<&str>,
) -> Result<Option<[u8; 32]>> {
    let Some(previous) = previous else {
        return Ok(None);
    };
    let Some(current) = current else {
        bail!(
            "{PREVIOUS_KEY_ENV} needs TEMPER_VAULT_KEY: rows would be re-sealed under a throwaway key"
        );
    };
    decode_vault_key("TEMPER_VAULT_KEY", current)?;
    decode_vault_key(PREVIOUS_KEY_ENV, previous).map(Some)
}

fn decode_vault_key(name: &str, key_b64: &str) -> Result<[u8; 32]> {
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(key_b64)
        .map_err(|_| anyhow!("{name} must be valid base64"))?;
    decoded
        .try_into()
        .map_err(|decoded: Vec<u8>| anyhow!("{name} must be 32 bytes, got {}", decoded.len()))
}

/// What loading every tenant's stored secrets found.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StoredSecrets {
    /// Every loaded tenant's rows, by outcome.
    pub(crate) rows: SecretLoadReport,
    /// Tenants that hold secrets, or whose database could not be read, and
    /// were not loaded: their rows may still need the previous key.
    pub(crate) unchecked: usize,
    /// The tenants holding secrets could not be listed at all.
    pub(crate) unlisted: bool,
}

impl StoredSecrets {
    /// The boot's one line on stored secrets, worded as `temper serve` words it.
    pub(crate) fn line(&self) -> String {
        let rows = &self.rows;
        let mut line = format!(
            "Secrets from database (every tenant): {} kept, {} re-sealed, {} deferred, {} unreadable, {} tenants unchecked.",
            rows.kept, rows.resealed, rows.deferred, rows.unreadable, self.unchecked
        );
        if rows.unreadable > 0 {
            line.push_str(" Unreadable rows open with neither vault key; see the errors above.");
        }
        if self.unchecked > 0 {
            line.push_str(
                " Unchecked tenants' secrets were not loaded; their rows may still need the previous key.",
            );
        }
        if self.unlisted {
            line.push_str(" Tenants holding secrets could not be listed; none were loaded.");
        }
        line
    }

    fn log(&self) {
        let line = self.line();
        let rows = &self.rows;
        if rows.unreadable > 0 || self.unchecked > 0 || self.unlisted {
            tracing::warn!(
                kept = rows.kept,
                resealed = rows.resealed,
                deferred = rows.deferred,
                unreadable = rows.unreadable,
                unchecked = self.unchecked,
                "{line}"
            );
        } else {
            tracing::info!(
                kept = rows.kept,
                resealed = rows.resealed,
                deferred = rows.deferred,
                unreadable = rows.unreadable,
                unchecked = self.unchecked,
                "{line}"
            );
        }
    }
}

/// Load every tenant's stored secrets through temper's load path, which
/// re-seals rows only the previous key opens, and log the line.
///
/// Each tenant's values land in its tenant cache, except the `shared`
/// tenants': their rows are served as shared platform copies, which values
/// set later in the boot (deployment variables) replace, so a tenant copy
/// would shadow those values. Their rows are still loaded, for the re-seal.
pub(crate) async fn load_stored_secrets(state: &PlatformState, shared: &[&str]) -> StoredSecrets {
    let mut loaded = StoredSecrets::default();
    if state.server.secrets_vault.is_none() || state.server.storage_stack.is_none() {
        return loaded;
    }
    let found = match state.server.secret_tenants().await {
        Ok(found) => found,
        Err(e) => {
            tracing::error!(error = %e, "tenants holding secrets could not be listed");
            loaded.unlisted = true;
            loaded.log();
            return loaded;
        }
    };
    let due: BTreeSet<&String> = found.holding.iter().chain(&found.unopened).collect();
    let mut done = 0usize;
    for tenant in &found.holding {
        if let Some(reason) = state.tenant_unavailable_reason(tenant) {
            tracing::error!(
                tenant,
                reason,
                "tenant unavailable; its secrets are unchecked"
            );
            continue;
        }
        match state.server.load_tenant_secrets(tenant).await {
            Ok(report) => {
                loaded.rows.add(report);
                done += 1;
            }
            Err(e) => tracing::error!(tenant, error = %e, "failed to load secrets"),
        }
        if shared.contains(&tenant.as_str())
            && let Some(vault) = state.server.secrets_vault.as_ref()
        {
            for key in vault.list_keys(tenant) {
                vault.remove_secret(tenant, &key);
            }
        }
    }
    loaded.unchecked = due.len() - done;
    loaded.log();
    loaded
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use base64::Engine as _;
    use temper_runtime::persistence::SecretPrecondition;
    use temper_server::secrets::vault::SecretsVault;

    use super::*;

    const KEY_A: [u8; 32] = [0xA1; 32];
    const KEY_B: [u8; 32] = [0xB2; 32];

    fn b64(key: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(key)
    }

    #[test]
    fn previous_key_needs_a_valid_current_key() {
        let a = b64(&KEY_A);
        let b = b64(&KEY_B);
        assert_eq!(previous_vault_key(None, None).unwrap(), None);
        assert_eq!(previous_vault_key(Some(&b), None).unwrap(), None);
        assert_eq!(previous_vault_key(Some(&b), Some(&a)).unwrap(), Some(KEY_A));

        let without_current = previous_vault_key(None, Some(&a)).unwrap_err();
        assert!(
            without_current
                .to_string()
                .contains("needs TEMPER_VAULT_KEY")
        );
        let short_current = previous_vault_key(Some(&b64(&[1; 16])), Some(&a)).unwrap_err();
        assert!(
            short_current
                .to_string()
                .contains("TEMPER_VAULT_KEY must be 32 bytes")
        );
        let bad_previous = previous_vault_key(Some(&b), Some("not base64!")).unwrap_err();
        assert!(
            bad_previous
                .to_string()
                .contains("TEMPER_VAULT_KEY_PREVIOUS must be valid base64")
        );
    }

    /// One boot's server on the store at `events`, its vault holding `current`
    /// and, during a rotation, `previous`.
    async fn boot(events: &str, current: [u8; 32], previous: Option<[u8; 32]>) -> PlatformState {
        let mut state = PlatformState::with_registry(temper_server::SpecRegistry::new(), None);
        let store = temper_store_turso::TursoEventStore::new(events, None)
            .await
            .unwrap();
        state
            .server
            .set_storage_stack(temper_server::StorageStack::from_turso(store));
        let mut vault = SecretsVault::new(&current);
        if let Some(previous) = previous {
            vault = vault.with_previous_key(&previous);
        }
        state.server.secrets_vault = Some(Arc::new(vault));
        state
    }

    fn report(kept: usize, resealed: usize) -> StoredSecrets {
        StoredSecrets {
            rows: SecretLoadReport {
                kept,
                resealed,
                ..SecretLoadReport::default()
            },
            ..StoredSecrets::default()
        }
    }

    #[tokio::test]
    async fn a_rotation_reseals_old_rows_once_and_the_new_key_alone_reads_them() {
        let dir = tempfile::tempdir().unwrap();
        let events = format!("file:{}", dir.path().join("events.db").display());

        let before = boot(&events, KEY_A, None).await;
        let vault = before.server.secrets_vault.clone().unwrap();
        for (tenant, value) in [("default", "one"), ("other", "two")] {
            let (ciphertext, nonce) = vault.encrypt(value.as_bytes()).unwrap();
            before
                .server
                .write_secret(
                    tenant,
                    "api_key",
                    &ciphertext,
                    &nonce,
                    SecretPrecondition::Any,
                )
                .await
                .unwrap();
        }
        drop(before);

        let rotating = boot(&events, KEY_B, Some(KEY_A)).await;
        assert_eq!(
            load_stored_secrets(&rotating, &["default"]).await,
            report(0, 2)
        );
        let vault = rotating.server.secrets_vault.clone().unwrap();
        assert_eq!(vault.get_secret("other", "api_key").as_deref(), Some("two"));
        drop(rotating);

        let again = boot(&events, KEY_B, Some(KEY_A)).await;
        assert_eq!(
            load_stored_secrets(&again, &["default"]).await,
            report(2, 0)
        );
        drop(again);

        let rotated = boot(&events, KEY_B, None).await;
        let loaded = load_stored_secrets(&rotated, &["default"]).await;
        assert_eq!(loaded, report(2, 0));
        assert!(loaded.line().starts_with(
            "Secrets from database (every tenant): 2 kept, 0 re-sealed, 0 deferred, 0 unreadable, 0 tenants unchecked."
        ));
        let vault = rotated.server.secrets_vault.clone().unwrap();
        assert_eq!(vault.get_secret("other", "api_key").as_deref(), Some("two"));
    }

    /// The shared tenants' rows are served as shared copies, which values set
    /// later in the boot replace, so none of them stays in the tenant cache to
    /// shadow those values.
    #[tokio::test]
    async fn a_shared_tenants_rows_leave_no_tenant_copy_behind() {
        let dir = tempfile::tempdir().unwrap();
        let events = format!("file:{}", dir.path().join("events.db").display());
        let state = boot(&events, KEY_B, Some(KEY_A)).await;
        let old = SecretsVault::new(&KEY_A);
        for tenant in ["default", "other"] {
            let (ciphertext, nonce) = old.encrypt(b"stored").unwrap();
            state
                .server
                .write_secret(
                    tenant,
                    "api_key",
                    &ciphertext,
                    &nonce,
                    SecretPrecondition::Any,
                )
                .await
                .unwrap();
        }

        assert_eq!(
            load_stored_secrets(&state, &["default"]).await,
            report(0, 2)
        );
        let vault = state.server.secrets_vault.clone().unwrap();
        vault.cache_platform_secret("api_key", "set-later-in-boot".to_string());
        assert_eq!(
            vault.get_secret("default", "api_key").as_deref(),
            Some("set-later-in-boot")
        );
        assert_eq!(
            vault.get_secret("other", "api_key").as_deref(),
            Some("stored")
        );
    }
}
