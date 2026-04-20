//! Self-bootstrap of `pir-data/` from the published snapshot CDN.
//!
//! Runs once at `serve` startup, before [`pir_server::load_serving_state`].
//! Resolves the canonical snapshot height from the published voting-config
//! and, if local state is missing or at the wrong height, downloads the
//! pre-computed tier files from the bucket configured by
//! `--precomputed-base-url` and verifies them against the manifest's
//! sha256s before swapping them into `--pir-data-dir`.
//!
//! ## URL layout (matches `.github/workflows/publish-snapshot.yml`)
//!
//! ```text
//! <precomputed_base_url>/snapshots/<height>/manifest.json
//! <precomputed_base_url>/snapshots/<height>/tier0.bin
//! <precomputed_base_url>/snapshots/<height>/tier1.bin
//! <precomputed_base_url>/snapshots/<height>/tier2.bin
//! <precomputed_base_url>/snapshots/<height>/pir_root.json
//! ```
//!
//! ## Atomicity
//!
//! Files are written into `<pir_data_dir>/.bootstrap-staging/` and verified
//! before being moved into place. Within a single filesystem each `rename`
//! is atomic; we move the tier blobs first and `pir_root.json` last so
//! that the next startup will treat a half-applied bootstrap (if the
//! process is killed mid-rename) as "no/old metadata" and re-attempt.
//!
//! ## Failure policy
//!
//! Each network step (voting-config fetch, manifest fetch, blob fetch) is
//! treated as a soft failure: we log loudly and fall through to whatever
//! is on disk. The hard failure is `load_serving_state` later — if local
//! state is unusable AND we couldn't bootstrap, the daemon refuses to
//! start, which is the correct end-state for an empty PIR host. This
//! preserves the existing operator workflow on offline dev machines
//! (set `--voting-config-url ""` to skip entirely).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::metrics;

/// Subpath under `precomputed_base_url` where PIR snapshots live.
///
/// Matches the constant of the same name in the admin UI
/// (`vote-sdk/ui/src/api/chain.ts`) and the prefix written by
/// `publish-snapshot.yml`.
pub const PIR_SNAPSHOTS_PATH: &str = "/snapshots";

/// Files that make up a complete published snapshot, in the order they
/// must be moved into place. `pir_root.json` is intentionally last so
/// that its presence at the canonical height implies the tier blobs are
/// already in place.
const SNAPSHOT_FILES: &[&str] = &["tier0.bin", "tier1.bin", "tier2.bin", "pir_root.json"];

/// Subset of `voting-config.json` that nf-server needs at startup.
#[derive(Debug, Deserialize)]
struct VotingConfig {
    /// Canonical Orchard nullifier-tree snapshot height for the current
    /// voting round. Optional in the schema for back-compat with older
    /// configs; an absent value means "operator manages snapshots out of
    /// band, don't touch local state".
    #[serde(default)]
    snapshot_height: Option<u64>,
}

/// Subset of `pir_root.json` that the bootstrap reads to decide whether
/// the local snapshot is already at the right height. We deliberately
/// don't import the full `PirMetadata` struct from `pir-types` — the
/// only field we care about here is `height`, and being lenient about
/// the rest avoids tying the bootstrap path to schema changes elsewhere.
#[derive(Debug, Deserialize)]
struct PirRootHeader {
    #[serde(default)]
    height: Option<u64>,
}

/// Per-file integrity entry in `manifest.json`.
#[derive(Debug, Deserialize)]
struct ManifestFile {
    size: u64,
    sha256: String,
}

/// Wire format of `manifest.json` published by `publish-snapshot.yml`.
#[derive(Debug, Deserialize)]
struct PublishedManifest {
    /// `1` today; bumped if the file layout changes.
    schema_version: u32,
    /// Block height the snapshot was built at; must equal the height
    /// embedded in the URL and in `pir_root.json`.
    height: u64,
    files: std::collections::BTreeMap<String, ManifestFile>,
}

/// Configuration for [`run`] resolved from CLI flags / env.
///
/// All URL fields are pre-trimmed of trailing slashes so the bootstrap
/// composes paths without doubled slashes.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where to fetch `voting-config.json` from. Empty string disables
    /// the entire bootstrap (operator manages snapshots manually).
    pub voting_config_url: String,
    /// Bucket origin for pre-computed snapshots. Empty disables download
    /// even if the voting-config height differs from local state — we
    /// surface a warning so the operator notices.
    pub precomputed_base_url: String,
    /// Where the live snapshot lives. Bootstrap writes here.
    pub pir_data_dir: PathBuf,
    /// Cap on each individual HTTP request. Tier files are large
    /// (multi-GB); 30 minutes is generous for slow links and lines up
    /// with how long the publisher CI itself takes.
    pub http_timeout: Duration,
}

impl Config {
    /// Production-default endpoints. Matches the admin UI's defaults
    /// (`SVOTE_VOTING_CONFIG_URL` is implicit there) and svoted's
    /// `SVOTE_PRECOMPUTED_BASE_URL` default.
    pub const DEFAULT_VOTING_CONFIG_URL: &'static str =
        "https://valargroup.github.io/token-holder-voting-config/voting-config.json";
    pub const DEFAULT_PRECOMPUTED_BASE_URL: &'static str =
        "https://vote.fra1.digitaloceanspaces.com";
}

/// Outcome of [`run`], used for logging/metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Bootstrap was disabled by an empty `voting_config_url`.
    Disabled,
    /// Local snapshot is already at the expected height; nothing fetched.
    AlreadyAtHeight(u64),
    /// Local snapshot was missing or stale; CDN fetch succeeded.
    BootstrappedTo(u64),
    /// Local snapshot is stale and CDN fetch failed (or
    /// `precomputed_base_url` was empty); falling through to local.
    FellThrough { reason: String },
}

/// Run the bootstrap. See module docs for the algorithm.
///
/// Returns the [`Outcome`] for logging; never returns Err for soft
/// failures (network, missing remote snapshot at the requested height).
/// Hard errors (e.g. unable to write into `pir_data_dir`) propagate up
/// because they indicate a misconfigured host, not a transient issue.
pub async fn run(cfg: &Config) -> Result<Outcome> {
    let started = Instant::now();
    metrics::bootstrap_attempts_inc();

    if cfg.voting_config_url.is_empty() {
        info!("snapshot bootstrap disabled (voting-config-url is empty)");
        metrics::bootstrap_outcome_inc("disabled");
        return Ok(Outcome::Disabled);
    }

    let local_height = read_local_height(&cfg.pir_data_dir);
    if let Some(h) = local_height {
        metrics::served_height_set(h);
    }

    let expected_height = match fetch_voting_config_height(&cfg.voting_config_url, cfg.http_timeout)
        .await
    {
        Ok(Some(h)) => {
            info!(height = h, "voting-config snapshot_height resolved");
            metrics::expected_height_set(h);
            h
        }
        Ok(None) => {
            warn!(
                url = %cfg.voting_config_url,
                "voting-config has no snapshot_height; falling through to local state"
            );
            metrics::bootstrap_outcome_inc("fell_through");
            return Ok(Outcome::FellThrough {
                reason: "voting-config has no snapshot_height".to_string(),
            });
        }
        Err(e) => {
            warn!(
                url = %cfg.voting_config_url,
                error = %e,
                "voting-config fetch failed; falling through to local state"
            );
            metrics::bootstrap_outcome_inc("fell_through");
            return Ok(Outcome::FellThrough {
                reason: format!("voting-config fetch failed: {e}"),
            });
        }
    };

    if local_height == Some(expected_height) {
        info!(
            height = expected_height,
            "local snapshot already at expected height"
        );
        metrics::bootstrap_duration_observe(started.elapsed());
        metrics::bootstrap_outcome_inc("already_at_height");
        return Ok(Outcome::AlreadyAtHeight(expected_height));
    }

    if cfg.precomputed_base_url.is_empty() {
        warn!(
            local = ?local_height,
            expected = expected_height,
            "local snapshot does not match voting-config but precomputed-base-url is empty; falling through"
        );
        metrics::bootstrap_outcome_inc("fell_through");
        return Ok(Outcome::FellThrough {
            reason: "precomputed-base-url is empty".to_string(),
        });
    }

    info!(
        local = ?local_height,
        expected = expected_height,
        base = %cfg.precomputed_base_url,
        "bootstrapping snapshot from CDN"
    );

    match fetch_and_install(cfg, expected_height).await {
        Ok(bytes) => {
            metrics::bootstrap_bytes_inc(bytes);
            metrics::bootstrap_duration_observe(started.elapsed());
            metrics::served_height_set(expected_height);
            metrics::bootstrap_outcome_inc("bootstrapped");
            info!(
                height = expected_height,
                bytes,
                elapsed_s = format!("{:.1}", started.elapsed().as_secs_f64()),
                "snapshot bootstrap complete"
            );
            Ok(Outcome::BootstrappedTo(expected_height))
        }
        Err(e) => {
            warn!(
                error = %e,
                expected = expected_height,
                "snapshot bootstrap failed; falling through to local state"
            );
            metrics::bootstrap_outcome_inc("fell_through");
            Ok(Outcome::FellThrough {
                reason: format!("CDN fetch failed: {e}"),
            })
        }
    }
}

/// Best-effort read of the height baked into a local `pir_root.json`.
///
/// Returns `None` if the file is missing, unreadable, malformed, or
/// has no height field. Any of those means "you need to bootstrap".
fn read_local_height(pir_data_dir: &Path) -> Option<u64> {
    let path = pir_data_dir.join("pir_root.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let meta: PirRootHeader = serde_json::from_str(&raw).ok()?;
    meta.height
}

/// Fetch the published voting-config and return its `snapshot_height`.
///
/// Returns `Ok(None)` when the field is absent (legitimate config
/// without a declared snapshot, e.g. before the first round is set up);
/// returns `Err` for network/decoding failures.
async fn fetch_voting_config_height(url: &str, timeout: Duration) -> Result<Option<u64>> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("build reqwest client")?;
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url} (non-2xx)"))?;
    let cfg: VotingConfig = resp
        .json()
        .await
        .with_context(|| format!("decode {url} as voting-config"))?;
    Ok(cfg.snapshot_height)
}

/// Download manifest + tier files for `height`, verify sha256s, and
/// install them into `pir_data_dir`. Returns the total bytes fetched.
async fn fetch_and_install(cfg: &Config, height: u64) -> Result<u64> {
    let client = reqwest::Client::builder()
        .timeout(cfg.http_timeout)
        .build()
        .context("build reqwest client")?;

    let snapshot_dir = format!(
        "{}{}/{}",
        cfg.precomputed_base_url, PIR_SNAPSHOTS_PATH, height
    );
    let manifest_url = format!("{snapshot_dir}/manifest.json");

    let manifest: PublishedManifest = client
        .get(&manifest_url)
        .send()
        .await
        .with_context(|| format!("GET {manifest_url}"))?
        .error_for_status()
        .with_context(|| format!("GET {manifest_url} (non-2xx)"))?
        .json()
        .await
        .with_context(|| format!("decode {manifest_url}"))?;

    if manifest.schema_version != 1 {
        bail!(
            "manifest schema_version = {} (only 1 is supported); upgrade nf-server",
            manifest.schema_version
        );
    }
    if manifest.height != height {
        bail!(
            "manifest height = {} but URL says {}; refusing to install mismatched snapshot",
            manifest.height,
            height
        );
    }
    for f in SNAPSHOT_FILES {
        if !manifest.files.contains_key(*f) {
            bail!("manifest is missing required file {f}");
        }
    }

    let staging = cfg.pir_data_dir.join(".bootstrap-staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("clean staging dir {}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("create staging dir {}", staging.display()))?;

    let mut total_bytes: u64 = 0;
    for name in SNAPSHOT_FILES {
        let entry = &manifest.files[*name];
        let url = format!("{snapshot_dir}/{name}");
        let dest = staging.join(name);
        let written = download_and_verify(&client, &url, &dest, &entry.sha256, entry.size).await?;
        total_bytes = total_bytes.saturating_add(written);
    }

    install_from_staging(&staging, &cfg.pir_data_dir)?;

    if let Err(e) = std::fs::remove_dir_all(&staging) {
        warn!(error = %e, dir = %staging.display(), "failed to clean staging dir");
    }

    Ok(total_bytes)
}

/// Stream `url` to `dest`, hashing as we go, and `bail!` if the resulting
/// sha256 or byte length disagrees with the manifest. On any error the
/// partial file is removed so a retry starts from a clean slate.
///
/// Uses `Response::chunk` rather than `bytes_stream` to avoid pulling in
/// the `futures-util` crate just to call `.next()`.
async fn download_and_verify(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    expected_size: u64,
) -> Result<u64> {
    use tokio::io::AsyncWriteExt;

    info!(url = %url, expected_size, "downloading snapshot file");
    let started = Instant::now();
    let mut resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url} (non-2xx)"))?;

    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("create {}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .with_context(|| format!("read body chunk from {url}"))?
    {
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .with_context(|| format!("write to {}", dest.display()))?;
        written = written.saturating_add(chunk.len() as u64);
    }
    file.flush()
        .await
        .with_context(|| format!("flush {}", dest.display()))?;
    drop(file);

    if written != expected_size {
        let _ = std::fs::remove_file(dest);
        bail!("{url}: downloaded {written} bytes but manifest expected {expected_size}");
    }

    let actual_sha = hex::encode(hasher.finalize());
    if !actual_sha.eq_ignore_ascii_case(expected_sha256) {
        let _ = std::fs::remove_file(dest);
        bail!(
            "{url}: sha256 mismatch (expected {expected_sha256}, got {actual_sha})"
        );
    }

    info!(
        url = %url,
        bytes = written,
        elapsed_s = format!("{:.1}", started.elapsed().as_secs_f64()),
        "snapshot file verified"
    );
    Ok(written)
}

/// Move staged files into `pir_data_dir` in the order defined by
/// [`SNAPSHOT_FILES`] (tier blobs first, `pir_root.json` last) so that
/// a half-applied install is idempotent — the absent or stale
/// `pir_root.json` will simply trigger another bootstrap on the next
/// startup.
fn install_from_staging(staging: &Path, pir_data_dir: &Path) -> Result<()> {
    if !pir_data_dir.exists() {
        std::fs::create_dir_all(pir_data_dir)
            .with_context(|| format!("create {}", pir_data_dir.display()))?;
    }
    for name in SNAPSHOT_FILES {
        let from = staging.join(name);
        let to = pir_data_dir.join(name);
        std::fs::rename(&from, &to).map_err(|e| {
            anyhow!(
                "rename {} -> {} failed: {e}. Bootstrap left partial state behind; re-run \
                 the daemon to retry.",
                from.display(),
                to.display(),
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    fn write_pir_root(dir: &Path, height: Option<u64>) {
        // Mirrors the relevant fields of `pir_types::PirMetadata` without
        // depending on it: the bootstrap reads only `height`, but the
        // file shape on disk is fixed by `pir-export`.
        let mut m = serde_json::json!({
            "root25": "00",
            "root29": "00",
            "num_ranges": 1,
            "pir_depth": 1,
            "tier0_bytes": 0,
            "tier1_rows": 0,
            "tier1_row_bytes": 0,
            "tier2_rows": 0,
            "tier2_row_bytes": 0,
        });
        if let Some(h) = height {
            m["height"] = serde_json::Value::from(h);
        }
        std::fs::write(
            dir.join("pir_root.json"),
            serde_json::to_string(&m).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn read_local_height_returns_none_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(read_local_height(tmp.path()), None);
    }

    #[test]
    fn read_local_height_returns_none_for_malformed_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("pir_root.json"), b"not json").unwrap();
        assert_eq!(read_local_height(tmp.path()), None);
    }

    #[test]
    fn read_local_height_extracts_height() {
        let tmp = TempDir::new().unwrap();
        write_pir_root(tmp.path(), Some(42));
        assert_eq!(read_local_height(tmp.path()), Some(42));
    }

    #[test]
    fn read_local_height_returns_none_when_height_field_absent() {
        let tmp = TempDir::new().unwrap();
        write_pir_root(tmp.path(), None);
        assert_eq!(read_local_height(tmp.path()), None);
    }

    #[test]
    fn install_moves_files_in_canonical_order() {
        let tmp = TempDir::new().unwrap();
        let staging = tmp.path().join(".bootstrap-staging");
        let dest = tmp.path().join("pir-data");
        std::fs::create_dir_all(&staging).unwrap();
        for name in SNAPSHOT_FILES {
            std::fs::write(staging.join(name), name.as_bytes()).unwrap();
        }
        install_from_staging(&staging, &dest).unwrap();
        for name in SNAPSHOT_FILES {
            assert!(dest.join(name).exists(), "{name} should be installed");
            assert!(!staging.join(name).exists(), "{name} should be moved");
        }
        assert_eq!(SNAPSHOT_FILES.last(), Some(&"pir_root.json"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_disabled_when_voting_config_url_empty() {
        let tmp = TempDir::new().unwrap();
        let cfg = Config {
            voting_config_url: String::new(),
            precomputed_base_url: "ignored".into(),
            pir_data_dir: tmp.path().to_path_buf(),
            http_timeout: Duration::from_secs(1),
        };
        let outcome = run(&cfg).await.unwrap();
        assert_eq!(outcome, Outcome::Disabled);
    }

    #[test]
    fn manifest_decodes_canonical_payload() {
        let raw = serde_json::json!({
            "schema_version": 1,
            "height": 100,
            "created_at": "2026-01-01T00:00:00Z",
            "nf_server_sha256": "deadbeef",
            "publisher": { "git_ref": "main", "git_sha": "abc" },
            "files": {
                "tier0.bin":     { "size": 1, "sha256": "00" },
                "tier1.bin":     { "size": 2, "sha256": "11" },
                "tier2.bin":     { "size": 3, "sha256": "22" },
                "pir_root.json": { "size": 4, "sha256": "33" }
            }
        });
        let m: PublishedManifest = serde_json::from_value(raw).unwrap();
        assert_eq!(m.schema_version, 1);
        assert_eq!(m.height, 100);
        let mut keys: Vec<&str> = m.files.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["pir_root.json", "tier0.bin", "tier1.bin", "tier2.bin"]);
    }

}
