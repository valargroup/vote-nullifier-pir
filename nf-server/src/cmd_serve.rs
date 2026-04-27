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
use crate::serve::watchdog;

#[derive(ClapArgs)]
pub struct Args {
    /// Listen port.
    #[arg(long, default_value = "3000", env = "SVOTE_PIR_PORT")]
    port: u16,

    /// Directory for all on-disk state: nullifiers.bin, nullifiers.checkpoint,
    /// nullifiers.index, nullifiers.tree, tier0.bin, tier1.bin, tier2.bin, and
    /// pir_root.json. Required for snapshot rebuilds via POST /snapshot/prepare.
    #[arg(long, default_value = "./pir-data", env = "SVOTE_PIR_DATA_DIR")]
    pir_data_dir: PathBuf,

    /// Lightwalletd endpoint URL(s) for syncing during rebuild.
    /// Can also be set via LWD_URLS env (comma-separated).
    #[arg(
        long,
        default_value = "https://zec.rocks:443",
        env = "SVOTE_PIR_MAINNET_RPC_URL"
    )]
    lwd_url: String,

    /// Chain SDK URL for checking active rounds before rebuild.
    /// If set, POST /snapshot/prepare will reject rebuilds when a round is active.
    #[arg(long, env = "SVOTE_PIR_VOTE_CHAIN_URL")]
    chain_url: Option<String>,

    /// URL of the published `voting-config.json` whose `snapshot_height`
    /// is treated as the canonical height every PIR replica should serve.
    /// Defaults to the production GitHub Pages URL; leave unset so operators
    /// pick up the baked-in default, or set `SVOTE_PIR_VOTING_CONFIG_URL=` (empty)
    /// to disable startup self-bootstrap and serve only pre-staged files under
    /// `pir_data_dir`.
    #[arg(
        long,
        env = "SVOTE_PIR_VOTING_CONFIG_URL",
        default_value = bootstrap::Config::DEFAULT_VOTING_CONFIG_URL
    )]
    voting_config_url: String,

    /// Bucket origin for pre-computed PIR snapshots (matches the
    /// admin UI). The bootstrap fetches
    /// `<base>/snapshots/<height>/{manifest.json,tier0.bin,...}`.
    /// Trailing slashes are trimmed. Empty disables the download
    /// portion of the bootstrap (operators relying on out-of-band
    /// staging can keep the voting-config height check enabled).
    #[arg(
        long,
        env = "SVOTE_PIR_PRECOMPUTED_BASE_URL",
        default_value = bootstrap::Config::DEFAULT_PRECOMPUTED_BASE_URL
    )]
    precomputed_base_url: String,

    /// Per-request timeout for the snapshot bootstrap in seconds.
    /// Defaults to 30 minutes — a slow tier0 fetch from the wrong
    /// region can sit close to that, so we err on the side of
    /// patience rather than spurious failures on a fresh host.
    #[arg(long, env = "SVOTE_PIR_BOOTSTRAP_TIMEOUT_SECS", default_value = "1800")]
    bootstrap_timeout_secs: u64,

    /// Download matching published YPIR precompute caches during snapshot
    /// bootstrap when the manifest provides them. Non-production targets skip
    /// this automatically. Set false to force local cache generation.
    #[arg(long, env = "SVOTE_PIR_PRECOMPUTE_BOOTSTRAP", default_value_t = true)]
    precompute_bootstrap: bool,

    /// How long the host must continuously serve a snapshot older
    /// than the canonical voting-config height before the watchdog
    /// emits a Sentry error event (which Sentry's Slack integration
    /// then routes to the on-call channel). Default 30 minutes.
    /// Set to 0 to disable the watchdog entirely.
    #[arg(long, env = "SVOTE_PIR_STALE_THRESHOLD_SECS", default_value = "1800")]
    stale_threshold_secs: u64,

    /// How often the watchdog checks `served` vs `expected`. The tick
    /// interval also bounds the precision of the `stale_seconds` gauge
    /// and the `stale_threshold_secs` deadline, so the default of 60s
    /// gives ±1m precision on a 30m threshold. Capped below the
    /// threshold at runtime.
    #[arg(long, env = "SVOTE_PIR_WATCHDOG_TICK_SECS", default_value = "60")]
    watchdog_tick_secs: u64,

    /// Sentry DSN for error tracking. When empty, Sentry is disabled.
    #[arg(long, env = "SENTRY_DSN", default_value = "")]
    pub(crate) sentry_dsn: String,
}

pub async fn run(args: Args) -> Result<()> {
    let chain_url = args
        .chain_url
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let lwd_urls = config::resolve_lwd_urls(&args.lwd_url);
    let state = Arc::new(AppState {
        phase: RwLock::new(ServerPhase::Starting {
            progress: "initializing".to_string(),
        }),
        serving: RwLock::new(None),
        rebuild_lock: Arc::new(tokio::sync::Mutex::new(())),
        pir_data_dir: args.pir_data_dir.clone(),
        lwd_urls,
        chain_url,
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
    let warm_pir_dir = args.pir_data_dir.clone();
    let warm_stale_threshold_secs = args.stale_threshold_secs;
    let warm_watchdog_tick_secs = args.watchdog_tick_secs;
    let warm_sentry_dsn = args.sentry_dsn.clone();
    // Self-bootstrap from the published snapshot CDN before we try to
    // load tier files. On a fresh host this populates `pir_data_dir/`
    // from scratch; on an existing host this is a no-op when the local
    // pir_root.json already matches voting-config.snapshot_height.
    let warm_bootstrap_cfg = bootstrap::Config {
        voting_config_url: args.voting_config_url.trim_end_matches('/').to_string(),
        precomputed_base_url: args.precomputed_base_url.trim_end_matches('/').to_string(),
        pir_data_dir: args.pir_data_dir.clone(),
        http_timeout: Duration::from_secs(args.bootstrap_timeout_secs),
        precompute_bootstrap: args.precompute_bootstrap,
        #[cfg(test)]
        precompute_cache_target_override: None,
    };
    // Hold `rebuild_lock` for the entire duration of the startup
    // pipeline (index rebuild → bootstrap → load). This serialises
    // startup against `POST /snapshot/prepare`, which also acquires
    // this lock before touching `pir_data_dir`. Without
    // it, a prepare call arriving in the Starting window would race
    // with `rebuild_index`, `bootstrap::run`, and `load_serving_state`
    // on the same on-disk stores and clobber `phase` / `serving`.
    //
    // The prepare handler separately refuses to run unless the phase
    // is `Serving` or `Rebuilding`, so during Starting it returns 503
    // with a clear error instead of spinning on lock contention.
    let startup_guard = Arc::clone(&warm_state.rebuild_lock).lock_owned().await;
    tokio::spawn(async move {
        let _startup_guard = startup_guard;
        let tx =
            sentry::start_transaction(sentry::TransactionContext::new("server-startup", "startup"));
        sentry::configure_scope(|scope| scope.set_span(Some(tx.clone().into())));

        {
            let mut phase = warm_state.phase.write().await;
            *phase = ServerPhase::Starting {
                progress: "rebuilding nullifier index".to_string(),
            };
        }
        let pir_dir_for_index = warm_pir_dir.clone();
        let index_result =
            tokio::task::spawn_blocking(move || file_store::rebuild_index(&pir_dir_for_index))
                .await;
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
        let pir_dir_for_load = warm_pir_dir.clone();
        let load =
            tokio::task::spawn_blocking(move || pir_server::load_serving_state(&pir_dir_for_load))
                .await;
        match load {
            Ok(Ok(serving)) => {
                if let Some(h) = serving.metadata.height {
                    metrics::served_height_set(h);
                }
                *warm_state.serving.write().await = Some(serving);
                *warm_state.phase.write().await = ServerPhase::Serving;
                // Spawn the snapshot-stale watchdog only after `served_height` is
                // populated; otherwise the first tick would always observe
                // `served=0` and start a false stale episode.
                //
                // We gate on BOTH a non-zero threshold (kill-switch for ops) AND a
                // configured SENTRY_DSN. Without a DSN, `sentry::capture_message`
                // is a no-op, so ticking forever would burn CPU and update the
                // `nf_snapshot_stale_seconds` gauge without ever paging anyone.
                let dsn_configured = !warm_sentry_dsn.trim().is_empty();
                if warm_stale_threshold_secs > 0 && dsn_configured {
                    let threshold = Duration::from_secs(warm_stale_threshold_secs);
                    // Tick faster than the threshold so we don't overshoot by a
                    // full tick. Floor at 1s for sanity.
                    let raw_tick = Duration::from_secs(warm_watchdog_tick_secs.max(1));
                    let tick = raw_tick.min(threshold);
                    eprintln!(
                        "snapshot watchdog: tick={}s threshold={}s",
                        tick.as_secs(),
                        threshold.as_secs(),
                    );
                    watchdog::spawn(tick, threshold);
                } else if warm_stale_threshold_secs == 0 {
                    eprintln!("snapshot watchdog: disabled (stale_threshold_secs=0)");
                } else {
                    eprintln!(
                        "snapshot watchdog: disabled (no SENTRY_DSN configured; \
                         set SENTRY_DSN to enable alerting)"
                    );
                }
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
