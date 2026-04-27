//! `pir-test bench-server`: closed-loop, iteration-bounded latency / bandwidth
//! / server-compute baseline driver against a running pir-server.
//!
//! For each iteration, we issue `batch_size` PIR proof fetches according to the
//! selected `mode` and capture the per-tier `TierTiming` records. Iterations are
//! aggregated into hdrhistograms and emitted as a JSON summary suitable for
//! checking into the repo as a baseline.
//!
//! Compared to `pir-test load`, this mode is meant to characterise *one*
//! request shape (K=batch_size, sequential or parallel) over a small,
//! reproducible number of iterations rather than to drive sustained traffic.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use hdrhistogram::Histogram;
use pasta_curves::Fp;
use rand::{Rng, SeedableRng};
use serde::Serialize;

use pir_client::{NoteTiming, PirClient, TierTiming};

// ── Config ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub enum BenchMode {
    /// Issue all K queries concurrently with `futures::join_all` (mirrors what
    /// `PirClient::fetch_proofs` does today; `try_join_all` is replaced with
    /// `join_all` so a single per-note error doesn't drop timings for the
    /// remaining queries).
    Parallel,
    /// Issue K queries one at a time inside a single iteration.
    Sequential,
    /// `batch_size` is forced to 1 — same flow as `Sequential` with K=1.
    Single,
    /// Issue K queries one at a time, all riding the same single
    /// HTTP/1.1 TCP/TLS connection (no HTTP/2 multiplexing, no concurrent
    /// streams). Useful for isolating per-query upload bandwidth from
    /// HTTP/2 stream contention: this mode pays `K * upload_bytes`
    /// serialized over one TCP connection, so its upload p50 reflects
    /// link bandwidth alone rather than concurrent-stream behavior.
    SingleTls,
}

impl BenchMode {
    fn as_str(self) -> &'static str {
        match self {
            BenchMode::Parallel => "parallel",
            BenchMode::Sequential => "sequential",
            BenchMode::Single => "single",
            BenchMode::SingleTls => "single-tls",
        }
    }
}

pub struct BenchConfig {
    pub url: String,
    pub nullifiers_path: PathBuf,
    pub iterations: usize,
    pub warmup: usize,
    pub batch_size: usize,
    pub mode: BenchMode,
    pub seed: Option<u64>,
    pub json_out: Option<PathBuf>,
    pub label: Option<String>,
}

// ── JSON output schema ───────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct BenchSummary {
    pub label: String,
    pub url: String,
    pub mode: String,
    pub batch_size: usize,
    pub iterations: usize,
    pub warmup: usize,
    pub started_at: String,
    pub finished_at: String,
    pub observer_host: String,
    pub wall_clock_ms: HistogramSummary,
    pub tier1: TierSummary,
    pub tier2: TierSummary,
    pub success_count: u64,
    pub error_count: u64,
    pub error_classes: Vec<ErrorClassCount>,
}

#[derive(Serialize)]
pub struct TierSummary {
    pub rtt_ms: HistogramSummary,
    pub server_total_ms: HistogramSummary,
    pub server_compute_ms: HistogramSummary,
    pub server_validate_ms: HistogramSummary,
    pub server_decode_copy_ms: HistogramSummary,
    pub net_queue_ms: HistogramSummary,
    pub upload_to_server_ms: HistogramSummary,
    pub download_from_server_ms: HistogramSummary,
    pub client_gen_ms: HistogramSummary,
    pub client_decode_ms: HistogramSummary,
    pub upload_bytes: BytesSummary,
    /// Per-query bytes attributable to the SimplePIR query vector
    /// itself (`q.0` / `pqr`).
    pub upload_q_bytes: BytesSummary,
    /// Per-query bytes attributable to `pack_pub_params`. Identical
    /// across queries that share a YPIR `client_seed`.
    pub upload_pp_bytes: BytesSummary,
    pub download_bytes: BytesSummary,
}

#[derive(Serialize)]
pub struct HistogramSummary {
    pub n: u64,
    pub min: f64,
    pub p50: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
}

impl HistogramSummary {
    fn from_histogram(h: &Histogram<u64>) -> Self {
        if h.is_empty() {
            return HistogramSummary {
                n: 0,
                min: 0.0,
                p50: 0.0,
                p90: 0.0,
                p95: 0.0,
                p99: 0.0,
                max: 0.0,
                mean: 0.0,
            };
        }
        HistogramSummary {
            n: h.len(),
            min: h.min() as f64 / 1000.0,
            p50: h.value_at_quantile(0.50) as f64 / 1000.0,
            p90: h.value_at_quantile(0.90) as f64 / 1000.0,
            p95: h.value_at_quantile(0.95) as f64 / 1000.0,
            p99: h.value_at_quantile(0.99) as f64 / 1000.0,
            max: h.max() as f64 / 1000.0,
            mean: h.mean() / 1000.0,
        }
    }
}

#[derive(Serialize)]
pub struct BytesSummary {
    pub n: u64,
    pub min: u64,
    pub max: u64,
    pub mean: f64,
}

#[derive(Serialize)]
pub struct ErrorClassCount {
    pub class: String,
    pub count: u64,
}

// ── Histograms ───────────────────────────────────────────────────────────────

/// Histogram tracking values in microseconds with 3 sig figs of precision,
/// up to 600s. Stored as u64 microseconds to preserve sub-millisecond detail
/// for fast LAN environments.
fn new_us_histogram() -> Histogram<u64> {
    Histogram::new_with_max(600 * 1_000_000, 3).expect("hdrhistogram bounds")
}

fn record_ms(h: &mut Histogram<u64>, ms: f64) {
    let us = (ms.max(0.0) * 1000.0) as u64;
    let _ = h.record(us);
}

#[derive(Default)]
struct BytesAggregator {
    n: u64,
    min: u64,
    max: u64,
    sum: u128,
}

impl BytesAggregator {
    fn record(&mut self, b: usize) {
        let b = b as u64;
        if self.n == 0 {
            self.min = b;
            self.max = b;
        } else {
            self.min = self.min.min(b);
            self.max = self.max.max(b);
        }
        self.n += 1;
        self.sum += b as u128;
    }
    fn into_summary(self) -> BytesSummary {
        let mean = if self.n > 0 {
            self.sum as f64 / self.n as f64
        } else {
            0.0
        };
        BytesSummary {
            n: self.n,
            min: self.min,
            max: self.max,
            mean,
        }
    }
}

#[derive(Default)]
struct TierAggregator {
    rtt: Option<Histogram<u64>>,
    server_total: Option<Histogram<u64>>,
    server_compute: Option<Histogram<u64>>,
    server_validate: Option<Histogram<u64>>,
    server_decode_copy: Option<Histogram<u64>>,
    net_queue: Option<Histogram<u64>>,
    upload_to_server: Option<Histogram<u64>>,
    download_from_server: Option<Histogram<u64>>,
    client_gen: Option<Histogram<u64>>,
    client_decode: Option<Histogram<u64>>,
    upload_bytes: BytesAggregator,
    upload_q_bytes: BytesAggregator,
    upload_pp_bytes: BytesAggregator,
    download_bytes: BytesAggregator,
}

impl TierAggregator {
    fn new() -> Self {
        Self {
            rtt: Some(new_us_histogram()),
            server_total: Some(new_us_histogram()),
            server_compute: Some(new_us_histogram()),
            server_validate: Some(new_us_histogram()),
            server_decode_copy: Some(new_us_histogram()),
            net_queue: Some(new_us_histogram()),
            upload_to_server: Some(new_us_histogram()),
            download_from_server: Some(new_us_histogram()),
            client_gen: Some(new_us_histogram()),
            client_decode: Some(new_us_histogram()),
            upload_bytes: BytesAggregator::default(),
            upload_q_bytes: BytesAggregator::default(),
            upload_pp_bytes: BytesAggregator::default(),
            download_bytes: BytesAggregator::default(),
        }
    }

    fn record(&mut self, t: &TierTiming) {
        record_ms(self.rtt.as_mut().unwrap(), t.rtt_ms);
        record_ms(self.client_gen.as_mut().unwrap(), t.gen_ms);
        record_ms(self.client_decode.as_mut().unwrap(), t.decode_ms);
        record_ms(
            self.download_from_server.as_mut().unwrap(),
            t.download_from_server_ms,
        );
        if let Some(v) = t.server_total_ms {
            record_ms(self.server_total.as_mut().unwrap(), v);
        }
        if let Some(v) = t.server_compute_ms {
            record_ms(self.server_compute.as_mut().unwrap(), v);
        }
        if let Some(v) = t.server_validate_ms {
            record_ms(self.server_validate.as_mut().unwrap(), v);
        }
        if let Some(v) = t.server_decode_copy_ms {
            record_ms(self.server_decode_copy.as_mut().unwrap(), v);
        }
        if let Some(v) = t.net_queue_ms {
            record_ms(self.net_queue.as_mut().unwrap(), v);
        }
        if let Some(v) = t.upload_to_server_ms {
            record_ms(self.upload_to_server.as_mut().unwrap(), v);
        }
        self.upload_bytes.record(t.upload_bytes);
        self.upload_q_bytes.record(t.upload_q_bytes);
        self.upload_pp_bytes.record(t.upload_pp_bytes);
        self.download_bytes.record(t.download_bytes);
    }

    fn into_summary(self) -> TierSummary {
        TierSummary {
            rtt_ms: HistogramSummary::from_histogram(self.rtt.as_ref().unwrap()),
            server_total_ms: HistogramSummary::from_histogram(self.server_total.as_ref().unwrap()),
            server_compute_ms: HistogramSummary::from_histogram(
                self.server_compute.as_ref().unwrap(),
            ),
            server_validate_ms: HistogramSummary::from_histogram(
                self.server_validate.as_ref().unwrap(),
            ),
            server_decode_copy_ms: HistogramSummary::from_histogram(
                self.server_decode_copy.as_ref().unwrap(),
            ),
            net_queue_ms: HistogramSummary::from_histogram(self.net_queue.as_ref().unwrap()),
            upload_to_server_ms: HistogramSummary::from_histogram(
                self.upload_to_server.as_ref().unwrap(),
            ),
            download_from_server_ms: HistogramSummary::from_histogram(
                self.download_from_server.as_ref().unwrap(),
            ),
            client_gen_ms: HistogramSummary::from_histogram(self.client_gen.as_ref().unwrap()),
            client_decode_ms: HistogramSummary::from_histogram(
                self.client_decode.as_ref().unwrap(),
            ),
            upload_bytes: self.upload_bytes.into_summary(),
            upload_q_bytes: self.upload_q_bytes.into_summary(),
            upload_pp_bytes: self.upload_pp_bytes.into_summary(),
            download_bytes: self.download_bytes.into_summary(),
        }
    }
}

// ── Driver ───────────────────────────────────────────────────────────────────

pub async fn run(cfg: BenchConfig) -> Result<()> {
    let started_at = chrono::Utc::now();

    let label = cfg.label.clone().unwrap_or_else(|| {
        format!(
            "{}-k{}-{}",
            short_url(&cfg.url),
            cfg.batch_size,
            cfg.mode.as_str()
        )
    });

    eprintln!("=== pir-test bench-server ===");
    eprintln!("  label:      {}", label);
    eprintln!("  url:        {}", cfg.url);
    eprintln!("  mode:       {}", cfg.mode.as_str());
    eprintln!("  batch_size: {}", cfg.batch_size);
    eprintln!("  iterations: {} (after {} warmup)", cfg.iterations, cfg.warmup);
    if let Some(s) = cfg.seed {
        eprintln!("  seed:       {}", s);
    }
    eprintln!();

    let nf_data = std::fs::read(&cfg.nullifiers_path)
        .with_context(|| format!("reading {:?}", cfg.nullifiers_path))?;
    let nfs = nf_ingest::file_store::parse_nullifier_bytes(&nf_data)?;
    eprintln!("  Loaded {} nullifiers from {:?}", nfs.len(), cfg.nullifiers_path);
    anyhow::ensure!(!nfs.is_empty(), "nullifiers file is empty");

    let ranges = pir_export::prepare_nullifiers(nfs);
    eprintln!("  Prepared {} ranges", ranges.len());

    eprintln!("  Connecting to PIR server...");
    let connect_start = Instant::now();
    let client = Arc::new(connect_client(cfg.mode, &cfg.url).await?);
    eprintln!(
        "  Connected in {:.2}s\n",
        connect_start.elapsed().as_secs_f64()
    );

    let mut rng: Box<dyn rand::RngCore> = match cfg.seed {
        Some(s) => Box::new(rand::rngs::StdRng::seed_from_u64(s)),
        None => Box::new(rand::thread_rng()),
    };

    let total_iters = cfg.warmup + cfg.iterations;
    let mut wall_clock = new_us_histogram();
    let mut tier1 = TierAggregator::new();
    let mut tier2 = TierAggregator::new();
    let mut success_count = 0u64;
    let mut error_count = 0u64;
    let mut error_classes: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();

    for iter in 0..total_iters {
        let is_warmup = iter < cfg.warmup;
        let test_values = pick_values(&ranges, cfg.batch_size, &mut rng);

        let iter_start = Instant::now();
        let outcomes = run_iteration(client.clone(), cfg.mode, &test_values).await;
        let wall_ms = iter_start.elapsed().as_secs_f64() * 1000.0;

        let mut iter_ok = 0usize;
        let mut iter_err = 0usize;
        for outcome in &outcomes {
            match outcome {
                Ok(_) => iter_ok += 1,
                Err(_) => iter_err += 1,
            }
        }

        eprintln!(
            "  iter {:>3}/{:<3} {:8} ok={} err={} wall={:.0}ms",
            iter + 1,
            total_iters,
            if is_warmup { "[warmup]" } else { "[measure]" },
            iter_ok,
            iter_err,
            wall_ms,
        );

        if is_warmup {
            continue;
        }

        record_ms(&mut wall_clock, wall_ms);
        for outcome in outcomes {
            match outcome {
                Ok(t) => {
                    tier1.record(&t.tier1);
                    tier2.record(&t.tier2);
                    success_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    *error_classes.entry(classify(&e)).or_insert(0) += 1;
                }
            }
        }
    }

    let finished_at = chrono::Utc::now();

    let mut error_class_vec: Vec<ErrorClassCount> = error_classes
        .into_iter()
        .map(|(class, count)| ErrorClassCount { class, count })
        .collect();
    error_class_vec.sort_by(|a, b| b.count.cmp(&a.count));

    let summary = BenchSummary {
        label,
        url: cfg.url.clone(),
        mode: cfg.mode.as_str().to_string(),
        batch_size: cfg.batch_size,
        iterations: cfg.iterations,
        warmup: cfg.warmup,
        started_at: started_at.to_rfc3339(),
        finished_at: finished_at.to_rfc3339(),
        observer_host: hostname(),
        wall_clock_ms: HistogramSummary::from_histogram(&wall_clock),
        tier1: tier1.into_summary(),
        tier2: tier2.into_summary(),
        success_count,
        error_count,
        error_classes: error_class_vec,
    };

    print_summary(&summary);

    if let Some(path) = cfg.json_out.as_ref() {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        let json = serde_json::to_string_pretty(&summary)?;
        std::fs::write(path, &json)
            .with_context(|| format!("writing JSON summary to {:?}", path))?;
        eprintln!("\nJSON summary written to {:?}", path);
    }

    Ok(())
}

async fn run_iteration(
    client: Arc<PirClient>,
    mode: BenchMode,
    test_values: &[Fp],
) -> Vec<Result<NoteTiming>> {
    match mode {
        BenchMode::Parallel => {
            let futs = test_values.iter().map(|&v| {
                let c = client.clone();
                async move {
                    let (_proof, timing) = c.fetch_proof_with_timing(v).await?;
                    Ok::<_, anyhow::Error>(timing)
                }
            });
            futures::future::join_all(futs).await
        }
        BenchMode::Sequential | BenchMode::Single | BenchMode::SingleTls => {
            let mut out = Vec::with_capacity(test_values.len());
            for &v in test_values {
                let res = client
                    .fetch_proof_with_timing(v)
                    .await
                    .map(|(_, timing)| timing);
                out.push(res);
            }
            out
        }
    }
}

/// Build a [`PirClient`] for `mode`. For everything except
/// [`BenchMode::SingleTls`] this uses the default reqwest client (HTTP/2,
/// connection pooling). [`BenchMode::SingleTls`] forces HTTP/1.1 with at
/// most one idle connection per host so each query rides the same TCP/TLS
/// session sequentially.
async fn connect_client(mode: BenchMode, url: &str) -> Result<PirClient> {
    match mode {
        BenchMode::SingleTls => {
            let http = reqwest::Client::builder()
                .http1_only()
                .pool_max_idle_per_host(1)
                .build()
                .context("building http1-only reqwest client")?;
            PirClient::connect_with_http(url, http).await
        }
        _ => PirClient::connect(url).await,
    }
}

fn pick_values<R: Rng + ?Sized>(ranges: &[[Fp; 3]], k: usize, rng: &mut R) -> Vec<Fp> {
    (0..k)
        .map(|_| {
            let idx = rng.gen_range(0..ranges.len());
            let [nf_lo, _, _] = ranges[idx];
            nf_lo + Fp::one()
        })
        .collect()
}

fn classify(e: &anyhow::Error) -> String {
    let msg = format!("{:#}", e);
    if msg.contains("timed out") || msg.contains("timeout") {
        "timeout".to_string()
    } else if msg.contains("HTTP 503") {
        "http_503".to_string()
    } else if msg.contains("HTTP 4") {
        "http_4xx".to_string()
    } else if msg.contains("HTTP 5") {
        "http_5xx".to_string()
    } else if msg.contains("decryption panicked") || msg.contains("decoded response too short") {
        "decode_fail".to_string()
    } else if msg.contains("not found in") {
        "tier_lookup_miss".to_string()
    } else {
        "other".to_string()
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        })
}

fn short_url(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url)
        .replace('.', "-")
}

fn fmt_ms(v: f64) -> String {
    if v >= 1000.0 {
        format!("{:.2}s", v / 1000.0)
    } else if v >= 1.0 {
        format!("{:.0}ms", v)
    } else {
        format!("{:.1}ms", v)
    }
}

fn fmt_bytes(b: f64) -> String {
    if b >= 1_048_576.0 {
        format!("{:.2}MB", b / 1_048_576.0)
    } else if b >= 1024.0 {
        format!("{:.1}KB", b / 1024.0)
    } else {
        format!("{}B", b as u64)
    }
}

fn print_summary(s: &BenchSummary) {
    eprintln!("\n=== bench-server summary ({}) ===", s.label);
    eprintln!(
        "  url={}  mode={}  K={}  iterations={}  ok={} err={}",
        s.url, s.mode, s.batch_size, s.iterations, s.success_count, s.error_count,
    );
    eprintln!();

    let h = &s.wall_clock_ms;
    eprintln!(
        "  wall-clock  p50={} p90={} p95={} p99={} max={}  (n={})",
        fmt_ms(h.p50),
        fmt_ms(h.p90),
        fmt_ms(h.p95),
        fmt_ms(h.p99),
        fmt_ms(h.max),
        h.n,
    );
    eprintln!();

    print_tier("tier1", &s.tier1);
    print_tier("tier2", &s.tier2);

    if !s.error_classes.is_empty() {
        let parts: Vec<String> = s
            .error_classes
            .iter()
            .map(|ec| format!("{}={}", ec.class, ec.count))
            .collect();
        eprintln!("\nerrors by class: {}", parts.join(" "));
    }
}

fn print_tier(name: &str, t: &TierSummary) {
    eprintln!(
        "  {:>5} rtt        p50={} p95={} p99={} max={}  (n={})",
        name,
        fmt_ms(t.rtt_ms.p50),
        fmt_ms(t.rtt_ms.p95),
        fmt_ms(t.rtt_ms.p99),
        fmt_ms(t.rtt_ms.max),
        t.rtt_ms.n,
    );
    eprintln!(
        "  {:>5} server     total p50={} p99={}  compute p50={} p99={}",
        name,
        fmt_ms(t.server_total_ms.p50),
        fmt_ms(t.server_total_ms.p99),
        fmt_ms(t.server_compute_ms.p50),
        fmt_ms(t.server_compute_ms.p99),
    );
    eprintln!(
        "  {:>5} net+queue  p50={} p95={} p99={}",
        name,
        fmt_ms(t.net_queue_ms.p50),
        fmt_ms(t.net_queue_ms.p95),
        fmt_ms(t.net_queue_ms.p99),
    );
    eprintln!(
        "  {:>5} client     gen p50={}  decode p50={}",
        name,
        fmt_ms(t.client_gen_ms.p50),
        fmt_ms(t.client_decode_ms.p50),
    );
    eprintln!(
        "  {:>5} bytes      up={} (pp={} + q={})  dn={}  (per query, mean over n={})",
        name,
        fmt_bytes(t.upload_bytes.mean),
        fmt_bytes(t.upload_pp_bytes.mean),
        fmt_bytes(t.upload_q_bytes.mean),
        fmt_bytes(t.download_bytes.mean),
        t.upload_bytes.n,
    );
    eprintln!();
}
