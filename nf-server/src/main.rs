//! Unified CLI binary for the nullifier PIR pipeline.
//!
//! Provides:
//!   - `doctor` — Pre-flight host checks vs runbook hardware guidance.
//!   - `dataset-info` — Print the supported nullifier dataset identity.
//!   - `sync` — Resumable ingest, `nullifiers.tree` checkpoint, PIR tier export.
//!   - `serve` — Start the PIR HTTP server (feature-gated behind `serve`).

#[cfg(feature = "serve")]
mod bootstrap;
mod cmd_doctor;
#[cfg(feature = "serve")]
mod cmd_serve;
mod cmd_sync;
#[cfg(feature = "serve")]
mod metrics;
#[cfg(feature = "serve")]
mod pir_config;
#[cfg(feature = "serve")]
mod serve;
mod sync_pipeline;
mod voting_config;

#[cfg(feature = "serve")]
use std::borrow::Cow;

use clap::{Parser, Subcommand};

/// Top-level CLI parser.
#[derive(Parser)]
#[command(
    name = "nf-server",
    version = env!("CARGO_PKG_VERSION"),
    about = "Nullifier PIR pipeline: inspect, sync, and serve"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Available subcommands.
#[derive(Subcommand)]
enum Command {
    /// Check host resources vs runbook recommendations (advisory warnings only)
    Doctor(cmd_doctor::Args),
    /// Print the supported nullifier dataset identity as JSON
    DatasetInfo,
    /// Ingest nullifiers, build tree checkpoint, export PIR tiers (resumable)
    Sync(cmd_sync::Args),
    /// Start the PIR HTTP server (requires --features serve)
    #[cfg(feature = "serve")]
    Serve(cmd_serve::Args),
}

#[cfg(feature = "serve")]
fn init_sentry(command: &Command) -> sentry::ClientInitGuard {
    let dsn = match command {
        Command::Serve(args) => args.sentry_dsn.as_str(),
        _ => "",
    };
    let environment =
        std::env::var("SENTRY_ENVIRONMENT").unwrap_or_else(|_| "production".to_string());
    let release = std::env::var("SENTRY_RELEASE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(Cow::Owned)
        .or_else(|| sentry::release_name!());
    sentry::init((
        dsn,
        sentry::ClientOptions {
            release,
            environment: Some(Cow::Owned(environment)),
            sample_rate: 1.0,
            // Only trace known API routes. SentryHttpLayer names transactions as
            // "METHOD /path" (raw URI) at sampling time, so unmatched paths such
            // as GET /favicon.ico are visible here and can be dropped.
            traces_sampler: Some(std::sync::Arc::new(|ctx: &sentry::TransactionContext| {
                let name = ctx.name();
                // Allow the startup trace and all registered API routes.
                // /metrics is intentionally excluded: Prometheus scrapes
                // would otherwise dominate Sentry transactions.
                let known: &[&str] = &[
                    "server-startup",
                    "GET /tier0",
                    "GET /params/tier1",
                    "GET /params/tier2",
                    "POST /tier1/query",
                    "POST /tier2/query",
                    "GET /tier1/row/",
                    "GET /tier2/row/",
                    "GET /root",
                    "GET /health",
                    "GET /ready",
                    "POST /snapshot/prepare",
                    "GET /snapshot/status",
                ];
                if known.iter().any(|&r| name.starts_with(r)) {
                    1.0
                } else {
                    0.0
                }
            })),
            attach_stacktrace: true,
            ..Default::default()
        },
    ))
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let _ = tracing_subscriber::fmt().try_init();

    #[cfg(feature = "serve")]
    let _sentry_guard = init_sentry(&cli.command);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            match cli.command {
                Command::Doctor(args) => Ok(cmd_doctor::run(args)?),
                Command::DatasetInfo => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "zcash_networks": ["main", "test"],
                            "nullifier_pool": pir_types::NULLIFIER_POOL,
                            "dataset_version": pir_types::DATASET_VERSION,
                        })
                    );
                    Ok(())
                }
                Command::Sync(args) => cmd_sync::run(args).await,
                #[cfg(feature = "serve")]
                Command::Serve(args) => cmd_serve::run(args).await,
            }
        })
}
