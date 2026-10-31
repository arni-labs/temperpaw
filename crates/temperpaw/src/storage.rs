//! TemperPaw storage selection and small Paw-specific persistence helpers.

use std::path::Path;

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use temper_runtime::persistence::{SecretPrecondition, SecretWrite};
use temper_server::StorageStack;
use temper_server::platform_store::PlatformStore;
use temper_store_postgres::PostgresEventStore;
use temper_store_postgres::migration::run_migrations;
use temper_store_turso::TursoEventStore;

use crate::config::{Config, StorageBackend};

#[derive(Clone)]
pub(crate) enum PawStorage {
    Turso(TursoEventStore),
    Postgres(PostgresEventStore),
}

impl PawStorage {
    pub(crate) async fn connect(config: &Config, default_db_path: &Path) -> Result<Self> {
        ensure_single_storage_backend(config)?;

        match config.event_store_backend {
            StorageBackend::Turso => {
                let turso_url = config
                    .turso_url
                    .clone()
                    .unwrap_or_else(|| format!("file:{}", default_db_path.display()));
                let store = TursoEventStore::new(&turso_url, config.turso_auth_token.as_deref())
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to connect to Turso/libSQL: {e}"))?;
                tracing::info!("Storage: turso ({turso_url})");
                Ok(Self::Turso(store))
            }
            StorageBackend::Postgres => {
                let database_url = config
                    .database_url
                    .as_deref()
                    .context("DATABASE_URL is required when TEMPER_EVENT_STORE=postgres")?;
                let max_connections = std::env::var("TEMPER_POSTGRES_MAX_CONNECTIONS")
                    .ok()
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(10);
                let pool = PgPoolOptions::new()
                    .max_connections(max_connections)
                    .connect(database_url)
                    .await
                    .with_context(|| "Failed to connect to Postgres via DATABASE_URL")?;
                run_migrations(&pool)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to run Postgres migrations: {e}"))?;
                tracing::info!(
                    max_connections,
                    "Storage: postgres (DATABASE_URL configured)"
                );
                Ok(Self::Postgres(PostgresEventStore::new(pool)))
            }
        }
    }

    pub(crate) fn backend_name(&self) -> &'static str {
        match self {
            Self::Turso(_) => "Turso",
            Self::Postgres(_) => "Postgres",
        }
    }

    pub(crate) fn platform_store(&self) -> &dyn PlatformStore {
        match self {
            Self::Turso(store) => store,
            Self::Postgres(store) => store,
        }
    }

    pub(crate) fn storage_stack(&self) -> StorageStack {
        match self {
            Self::Turso(store) => StorageStack::from_turso(store.clone()),
            Self::Postgres(store) => StorageStack::from_postgres(store.clone()),
        }
    }

    pub(crate) async fn upsert_secret(
        &self,
        tenant: &str,
        key_name: &str,
        ciphertext: &[u8],
        nonce: &[u8],
    ) -> Result<(), String> {
        // Each write gives the row a fresh version id, the way the server's own secret writes do
        // (temper #114), so a reader of version ids can never test guesses of the value against them.
        let version_id = temper_runtime::scheduler::sim_uuid().simple().to_string();
        let write = SecretWrite {
            tenant,
            key_name,
            ciphertext,
            nonce,
            version_id: &version_id,
            precondition: SecretPrecondition::Any,
        };
        let written = match self {
            Self::Turso(store) => store.write_secret(&write).await,
            Self::Postgres(store) => store.write_secret(&write).await,
        };
        written.map(|_| ()).map_err(|e| e.to_string())
    }

    pub(crate) async fn delete_secret(&self, tenant: &str, key_name: &str) -> Result<bool, String> {
        match self {
            Self::Turso(store) => store
                .delete_secret(tenant, key_name)
                .await
                .map_err(|e| e.to_string()),
            Self::Postgres(store) => store
                .delete_secret(tenant, key_name)
                .await
                .map_err(|e| e.to_string()),
        }
    }

    pub(crate) async fn load_secrets_for_tenant(
        &self,
        tenant: &str,
    ) -> Result<Vec<(String, Vec<u8>, Vec<u8>)>, String> {
        match self {
            Self::Turso(store) => store
                .load_secrets_for_tenant(tenant)
                .await
                .map_err(|e| e.to_string()),
            Self::Postgres(store) => store
                .load_secrets_for_tenant(tenant)
                .await
                .map_err(|e| e.to_string()),
        }
    }
}

fn ensure_single_storage_backend(config: &Config) -> Result<()> {
    for (env_name, backend) in [
        ("TEMPER_PLATFORM_STORE", config.platform_store_backend),
        (
            "TEMPER_QUERY_PROJECTION_STORE",
            config.query_projection_store_backend,
        ),
    ] {
        if backend != config.event_store_backend {
            anyhow::bail!(
                "{env_name}={} is not supported by temperpaw yet; it must match TEMPER_EVENT_STORE={}",
                backend.as_str(),
                config.event_store_backend.as_str()
            );
        }
    }
    Ok(())
}

impl From<TursoEventStore> for PawStorage {
    fn from(store: TursoEventStore) -> Self {
        Self::Turso(store)
    }
}

impl From<PostgresEventStore> for PawStorage {
    fn from(store: PostgresEventStore) -> Self {
        Self::Postgres(store)
    }
}
