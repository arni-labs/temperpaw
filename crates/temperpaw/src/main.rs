//! Temper Paw — Agent daemon built on Temper platform.
//!
//! Boots an embedded Temper platform, installs Paw OS apps,
//! seeds agent souls, and starts the Discord transport.

/// jemalloc, as Temper's own binary uses, with the purge settings in
/// [`allocator`]: glibc's malloc kept each read burst's high-water resident.
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod allocator;
mod auth;
mod config;
mod discord_app;
mod setup;
mod setup_api;
mod setup_llm;
mod startup;
mod storage;
mod transport_manager;

use clap::{Parser, Subcommand};
use std::io::IsTerminal;

const TOKIO_WORKER_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;

#[derive(Parser)]
#[command(name = "temperpaw-server", about = "Temper Paw — agent server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Configure and run TemperPaw locally — API keys, messaging, soul personalization
    Run,
    /// Diagnose configuration and show what's working
    Doctor,
}

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(TOKIO_WORKER_THREAD_STACK_BYTES)
        .build()?
        .block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut config = config::Config::from_env()?;

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let data_dir = std::path::Path::new(&home).join(".local/share/temperpaw");
    std::fs::create_dir_all(&data_dir)?;

    let force_soul_setup = match cli.command {
        Some(Command::Run) => {
            // Collect API key + messaging config, then boot the server
            let result = setup::run_setup_config(&config).await?;
            setup::merge_setup_into_config(&mut config, result);
            true // force soul personalization after boot
        }
        Some(Command::Doctor) => {
            setup::run_doctor(&data_dir, &config);
            return Ok(());
        }
        None => {
            // No subcommand: just boot the server.
            if std::io::stdin().is_terminal() && setup::needs_setup(&data_dir, &config) {
                eprintln!();
                eprintln!("  Temper Paw is not configured yet.");
                eprintln!();
                eprintln!("  Run \x1b[1mtemperpaw run\x1b[0m to get started locally,");
                eprintln!("  or  \x1b[1mtemperpaw deploy\x1b[0m to deploy to the cloud.");
                eprintln!();
                std::process::exit(1);
            }
            false
        }
    };

    ensure_dd_env();

    // In a terminal, suppress noisy logs — only show warnings and errors.
    // Full debug logs when RUST_LOG is explicitly set or not in a terminal.
    let is_terminal = std::io::stderr().is_terminal();
    if std::env::var_os("RUST_LOG").is_none() {
        let level = if is_terminal {
            "warn"
        } else {
            "info,temperpaw=debug"
        };
        unsafe {
            std::env::set_var("RUST_LOG", level);
        }
    }
    if config.otel_enabled {
        let has_explicit_endpoint = std::env::var_os("OTLP_ENDPOINT").is_some()
            || std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_some();
        if !has_explicit_endpoint {
            unsafe {
                std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", &config.otel_endpoint);
            }
        }
    } else {
        unsafe {
            std::env::remove_var("OTLP_ENDPOINT");
            std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        }
    }

    let otel_guard = temper_observe::otel::init_observability("temperpaw");

    // Print a clean banner in the terminal
    if is_terminal {
        let port = config.port;
        eprintln!();
        eprintln!("  \x1b[1mTemper Paw\x1b[0m is starting...");
        eprintln!();
        eprintln!("  Dashboard → \x1b[36mhttp://localhost:{port}/dashboard\x1b[0m");
        eprintln!("  API       → \x1b[36mhttp://localhost:{port}\x1b[0m");
        eprintln!();
        eprintln!("  Logs are suppressed. Set \x1b[1mRUST_LOG=info\x1b[0m for verbose output.");
        eprintln!();
    } else {
        tracing::info!("Temper Paw starting...");
    }

    let result = startup::run(config, force_soul_setup).await;

    if let Some(guard) = otel_guard {
        guard.shutdown();
    }

    result
}

fn ensure_dd_env() {
    if std::env::var_os("DD_ENV").is_none() {
        unsafe {
            std::env::set_var("DD_ENV", "local");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ensure_dd_env;

    #[test]
    fn ensure_dd_env_defaults_to_local_without_override() {
        unsafe {
            std::env::remove_var("DD_ENV");
        }

        ensure_dd_env();
        assert_eq!(std::env::var("DD_ENV").ok().as_deref(), Some("local"));
    }

    #[test]
    fn ensure_dd_env_preserves_existing_value() {
        unsafe {
            std::env::set_var("DD_ENV", "prod");
        }

        ensure_dd_env();
        assert_eq!(std::env::var("DD_ENV").ok().as_deref(), Some("prod"));
    }
}
