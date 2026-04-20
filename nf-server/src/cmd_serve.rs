//! `nf-server serve` — load tier files and start the PIR HTTP server.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use clap::Args as ClapArgs;
use sentry::integrations::tower as sentry_tower;
use tokio::sync::RwLock;
use tower::ServiceBuilder;

use nf_ingest::config;
use nf_ingest::file_store;

use crate::bootstrap;
use crate::metrics;
use crate::serve::handlers;
use crate::serve::rebuild;
use crate::serve::state::{AppState, ServerPhase};

#[derive(ClapArgs)]
pub struct Args {
    /// Listen port.
    #[arg(long, default_value = "3000")]
    port: u16,

    /// Directory containing tier0.bin, tier1.bin, tier2.bin, and pir_root.json.
    #[arg(long, default_value = "./pir-data")]
    pir_data_dir: PathBuf,

    /// Directory containing nullifiers.bin and nullifiers.checkpoint.
    /// Required for snapshot rebuilds via POST /snapshot/prepare.
    #[arg(long, default_value = ".")]
    data_dir: PathBuf,

    /// Lightwalletd endpoint URL(s) for syncing during rebuild.
    /// Can also be set via LWD_URLS env (comma-separated).
    #[arg(long, default_value = "https://zec.rocks:443")]
    lwd_url: String,

    /// Chain SDK URL for checking active rounds before rebuild.
    /// If set, POST /snapshot/prepare will reject rebuilds when a round is active.
    #[arg(long, env = "SVOTE_CHAIN_URL")]
    chain_url: Option<String>,

    /// URL of the published `voting-config.json` whose `snapshot_height`
    /// is treated as the canonical height every PIR replica should
    /// serve. Set to an empty string to disable the startup
    /// self-bootstrap entirely (operator manages snapshots manually).
    #[arg(
        long,
        env = "SVOTE_VOTING_CONFIG_URL",
        default_value = bootstrap::Config::DEFAULT_VOTING_CONFIG_URL
    )]
    voting_config_url: String,

    /// Bucket origin for pre-computed PIR snapshots (matches the
    /// admin UI's `SVOTE_PRECOMPUTED_BASE_URL`). The bootstrap fetches
    /// `<base>/snapshots/<height>/{manifest.json,tier0.bin,...}`.
    /// Trailing slashes are trimmed. Empty disables the download
    /// portion of the bootstrap (operators relying on out-of-band
    /// staging can keep the voting-config height check enabled).
    #[arg(
        long,
        env = "SVOTE_PRECOMPUTED_BASE_URL",
        default_value = bootstrap::Config::DEFAULT_PRECOMPUTED_BASE_URL
    )]
    precomputed_base_url: String,

    /// Per-request timeout for the snapshot bootstrap in seconds.
    /// Defaults to 30 minutes — a slow tier0 fetch from the wrong
    /// region can sit close to that, so we err on the side of
    /// patience rather than spurious failures on a fresh host.
    #[arg(long, env = "SVOTE_BOOTSTRAP_TIMEOUT_SECS", default_value = "1800")]
    bootstrap_timeout_secs: u64,

    /// Sentry DSN for error tracking. When empty, Sentry is disabled.
    #[arg(long, env = "SENTRY_DSN", default_value = "")]
    pub(crate) sentry_dsn: String,
}

pub async fn run(args: Args) -> Result<()> {
    tracing_subscriber::fmt::init();

    let lwd_urls = config::resolve_lwd_urls(&args.lwd_url);

    let state = Arc::new(AppState {
        phase: RwLock::new(ServerPhase::Starting {
            progress: "initializing".to_string(),
        }),
        serving: RwLock::new(None),
        rebuild_lock: Arc::new(tokio::sync::Mutex::new(())),
        data_dir: args.data_dir.clone(),
        pir_data_dir: args.pir_data_dir.clone(),
        lwd_urls,
        chain_url: args.chain_url.clone(),
        next_req_id: AtomicU64::new(0),
        inflight_requests: AtomicUsize::new(0),
    });

    let cors = tower_http::cors::CorsLayer::permissive();

    let app = Router::new()
        .route("/tier0", get(handlers::get_tier0))
        .route("/params/tier1", get(handlers::get_params_tier1))
        .route("/params/tier2", get(handlers::get_params_tier2))
        .route("/tier1/query", post(handlers::post_tier1_query))
        .route("/tier2/query", post(handlers::post_tier2_query))
        .route("/tier1/row/:idx", get(handlers::get_tier1_row))
        .route("/tier2/row/:idx", get(handlers::get_tier2_row))
        .route("/root", get(handlers::get_root))
        .route("/snapshot/prepare", post(rebuild::post_snapshot_prepare))
        .route("/snapshot/status", get(rebuild::get_snapshot_status))
        .route("/metrics", get(metrics::handle_metrics))
        .route("/health", get(handlers::get_health))
        .route("/ready", get(handlers::get_ready))
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .layer(cors)
        .layer(
            ServiceBuilder::new()
                .layer(sentry_tower::NewSentryLayer::new_from_top())
                .layer(sentry_tower::SentryHttpLayer::with_transaction()),
        )
        .with_state(Arc::clone(&state));

    let addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("Listening on {addr}");

    let warm_state = Arc::clone(&state);
    let warm_data_dir = args.data_dir.clone();
    let warm_pir_dir = args.pir_data_dir.clone();
    // Self-bootstrap from the published snapshot CDN before we try to
    // load tier files. On a fresh host this populates `pir_data_dir/`
    // from scratch; on an existing host this is a no-op when the local
    // pir_root.json already matches voting-config.snapshot_height.
    let warm_bootstrap_cfg = bootstrap::Config {
        voting_config_url: args.voting_config_url.trim_end_matches('/').to_string(),
        precomputed_base_url: args.precomputed_base_url.trim_end_matches('/').to_string(),
        pir_data_dir: args.pir_data_dir.clone(),
        http_timeout: Duration::from_secs(args.bootstrap_timeout_secs),
    };
    tokio::spawn(async move {
        let tx =
            sentry::start_transaction(sentry::TransactionContext::new("server-startup", "startup"));
        sentry::configure_scope(|scope| scope.set_span(Some(tx.clone().into())));

        {
            let mut phase = warm_state.phase.write().await;
            *phase = ServerPhase::Starting {
                progress: "rebuilding nullifier index".to_string(),
            };
        }
        let index_result =
            tokio::task::spawn_blocking(move || file_store::rebuild_index(&warm_data_dir)).await;
        match index_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let msg = format!("startup index rebuild failed: {e:#}");
                *warm_state.phase.write().await = ServerPhase::Error {
                    message: msg.clone(),
                };
                sentry::capture_message(&msg, sentry::Level::Error);
                tx.finish();
                return;
            }
            Err(e) => {
                let msg = format!("startup index rebuild task failed: {e}");
                *warm_state.phase.write().await = ServerPhase::Error {
                    message: msg.clone(),
                };
                sentry::capture_message(&msg, sentry::Level::Error);
                tx.finish();
                return;
            }
        }

        {
            let mut phase = warm_state.phase.write().await;
            *phase = ServerPhase::Starting {
                progress: "snapshot bootstrap".to_string(),
            };
        }
        match bootstrap::run(&warm_bootstrap_cfg).await {
            Ok(outcome) => eprintln!("snapshot bootstrap: {outcome:?}"),
            Err(e) => {
                let msg = format!("snapshot bootstrap hard error: {e:#}");
                *warm_state.phase.write().await = ServerPhase::Error {
                    message: msg.clone(),
                };
                sentry::capture_message(&msg, sentry::Level::Error);
                tx.finish();
                return;
            }
        }

        {
            let mut phase = warm_state.phase.write().await;
            *phase = ServerPhase::Starting {
                progress: "loading tier files".to_string(),
            };
        }
        let load =
            tokio::task::spawn_blocking(move || pir_server::load_serving_state(&warm_pir_dir))
                .await;
        match load {
            Ok(Ok(serving)) => {
                if let Some(h) = serving.metadata.height {
                    metrics::served_height_set(h);
                }
                *warm_state.serving.write().await = Some(serving);
                *warm_state.phase.write().await = ServerPhase::Serving;
                tx.finish();
                sentry::capture_message("nf-server ready", sentry::Level::Info);
            }
            Ok(Err(e)) => {
                let msg = format!("initial load failed: {e:#}");
                *warm_state.phase.write().await = ServerPhase::Error {
                    message: msg.clone(),
                };
                sentry::capture_message(&msg, sentry::Level::Error);
                tx.finish();
            }
            Err(e) => {
                let msg = format!("initial load task failed: {e}");
                *warm_state.phase.write().await = ServerPhase::Error {
                    message: msg.clone(),
                };
                sentry::capture_message(&msg, sentry::Level::Error);
                tx.finish();
            }
        }
    });

    axum::serve(listener, app).await?;

    Ok(())
}
