//! `nf-server snapshot` — call the admin listener on a running `nf-server serve`.

use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, Subcommand};
use hyper::StatusCode;
use serde_json::Value;

use crate::admin_client;
use crate::admin_listen::{parse_admin_endpoint, AdminBind};

#[derive(Subcommand)]
pub enum SnapshotCmd {
    /// Trigger an in-process rebuild on a running nf-server.
    Prepare {
        /// Target block height (multiple of 10, at or above NU5 activation).
        #[arg(long)]
        height: u64,
    },
    /// Print snapshot phase + height as JSON (one-shot).
    Status,
}

#[derive(ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: SnapshotCmd,

    /// Admin endpoint (`unix:///path` or `tcp://HOST:PORT`). Must match `serve --admin-listen`.
    #[arg(
        long,
        env = "SVOTE_PIR_ADMIN_ENDPOINT",
        default_value = "unix:///run/nf-server/admin.sock",
        global = true
    )]
    pub endpoint: String,
}

/// Run the snapshot CLI. Returns a process exit code (`0` = success).
pub async fn run(args: Args) -> i32 {
    let bind = match parse_admin_endpoint(&args.endpoint) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e:#}");
            return 2;
        }
    };

    match args.cmd {
        SnapshotCmd::Status => match status_once(&bind).await {
            Ok(v) => {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                0
            }
            Err(e) => {
                eprintln!("{e:#}");
                2
            }
        },
        SnapshotCmd::Prepare { height } => prepare_and_wait(&bind, height).await,
    }
}

async fn status_once(endpoint: &AdminBind) -> Result<Value> {
    let (status, bytes) = admin_client::get_bytes(endpoint, "/snapshot/status")
        .await
        .context("GET /snapshot/status")?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes);
        anyhow::bail!("GET /snapshot/status -> {status}: {body}");
    }
    let v: Value = serde_json::from_slice(&bytes).context("parse /snapshot/status JSON")?;
    Ok(v)
}

async fn prepare_and_wait(endpoint: &AdminBind, height: u64) -> i32 {
    let body = serde_json::json!({ "height": height });
    let (status, resp_bytes) = match admin_client::post_json(endpoint, "/snapshot/prepare", &body).await {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e:#}");
            return 2;
        }
    };

    if !status.is_success() {
        let msg = String::from_utf8_lossy(&resp_bytes);
        eprintln!("POST /snapshot/prepare -> {status}: {msg}");
        return 3;
    }
    if status != StatusCode::ACCEPTED {
        let msg = String::from_utf8_lossy(&resp_bytes);
        eprintln!("POST /snapshot/prepare -> unexpected {status}: {msg}");
        return 3;
    }
    let msg = String::from_utf8_lossy(&resp_bytes);
    eprintln!("{msg}");

    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let snap = match status_once(endpoint).await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e:#}");
                return 2;
            }
        };

        let phase = snap
            .get("phase")
            .and_then(|p| p.as_str())
            .unwrap_or_default();

        if let Some(pct) = snap.get("progress_pct").and_then(|p| p.as_u64()) {
            if let Some(prog) = snap.get("progress").and_then(|p| p.as_str()) {
                eprintln!("rebuild: {prog} ({pct}%)");
            }
        }

        if phase == "error" {
            let msg = snap
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("rebuild failed");
            eprintln!("{msg}");
            return 1;
        }

        if phase == "serving" {
            let served = snap.get("height").and_then(|h| h.as_u64()).unwrap_or(0);
            if served >= height {
                eprintln!("rebuild complete: serving height {served} (target {height})");
                return 0;
            }
            // Rare: keep polling if served height is still behind target.
            continue;
        }

        if phase != "rebuilding" {
            // starting / unknown — keep polling until terminal state.
            continue;
        }
    }
}
