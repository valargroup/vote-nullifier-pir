use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use hdrhistogram::Histogram;
use pasta_curves::Fp;
use serde::Serialize;
use tokio::sync::{mpsc, Semaphore};

use pir_client::PirClient;

// ── Config ───────────────────────────────────────────────────────────────────

pub struct LoadConfig {
    pub url: String,
    pub nullifiers_path: PathBuf,
    pub concurrency: usize,
    pub rps: Option<f64>,
    pub max_inflight: usize,
    pub duration: Duration,
    pub warmup: Duration,
    pub json_out: Option<PathBuf>,
    pub no_verify: bool,
    pub seed: Option<u64>,
    pub max_error_rate: f64,
    pub slo_p99_ms: Option<f64>,
}

// ── Sample ───────────────────────────────────────────────────────────────────

struct Sample {
    total_ms: f64,
    tier1_rtt_ms: f64,
    tier2_rtt_ms: f64,
    tier1_server_ms: Option<f64>,
    tier2_server_ms: Option<f64>,
    success: bool,
    error_class: Option<ErrorClass>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ErrorClass {
    Timeout,
    Http503,
    VerifyFail,
    Other,
}

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorClass::Timeout => write!(f, "timeout"),
            ErrorClass::Http503 => write!(f, "http_503"),
            ErrorClass::VerifyFail => write!(f, "verify_fail"),
            ErrorClass::Other => write!(f, "other"),
        }
    }
}

fn classify_error(e: &anyhow::Error) -> ErrorClass {
    let msg = format!("{:#}", e);
    if msg.contains("timed out") || msg.contains("timeout") {
        ErrorClass::Timeout
    } else if msg.contains("HTTP 503") {
        ErrorClass::Http503
    } else {
        ErrorClass::Other
    }
}

// ── JSON output ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct LoadSummary {
    pub url: String,
    pub duration_s: f64,
    pub concurrency: usize,
    pub completed: u64,
    pub errors: u64,
    pub error_rate: f64,
    pub stages: Vec<StageSummary>,
    pub error_classes: Vec<ErrorClassCount>,
}

#[derive(Serialize)]
pub struct StageSummary {
    pub name: String,
    pub n: u64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub p999_ms: f64,
    pub max_ms: f64,
}

#[derive(Serialize)]
pub struct ErrorClassCount {
    pub class: String,
    pub count: u64,
}

// ── StatsCollector ───────────────────────────────────────────────────────────

struct StatsCollector {
    end_to_end: Histogram<u64>,
    tier1_rtt: Histogram<u64>,
    tier2_rtt: Histogram<u64>,
    tier1_server: Histogram<u64>,
    tier2_server: Histogram<u64>,
    ok_count: u64,
    err_count: u64,
    error_counts: std::collections::HashMap<ErrorClass, u64>,
}

impl StatsCollector {
    fn new() -> Self {
        Self {
            end_to_end: Histogram::new_with_max(300_000, 3).unwrap(),
            tier1_rtt: Histogram::new_with_max(300_000, 3).unwrap(),
            tier2_rtt: Histogram::new_with_max(300_000, 3).unwrap(),
            tier1_server: Histogram::new_with_max(300_000, 3).unwrap(),
            tier2_server: Histogram::new_with_max(300_000, 3).unwrap(),
            ok_count: 0,
            err_count: 0,
            error_counts: std::collections::HashMap::new(),
        }
    }

    fn record(&mut self, sample: Sample) {
        let _ = self.end_to_end.record(sample.total_ms as u64);
        if sample.success {
            self.ok_count += 1;
            let _ = self.tier1_rtt.record(sample.tier1_rtt_ms as u64);
            let _ = self.tier2_rtt.record(sample.tier2_rtt_ms as u64);
            if let Some(ms) = sample.tier1_server_ms {
                let _ = self.tier1_server.record(ms as u64);
            }
            if let Some(ms) = sample.tier2_server_ms {
                let _ = self.tier2_server.record(ms as u64);
            }
        } else {
            self.err_count += 1;
            if let Some(class) = sample.error_class {
                *self.error_counts.entry(class).or_insert(0) += 1;
            }
        }
    }

    fn stage_summary(name: &str, h: &Histogram<u64>) -> StageSummary {
        StageSummary {
            name: name.to_string(),
            n: h.len(),
            p50_ms: h.value_at_quantile(0.50) as f64,
            p90_ms: h.value_at_quantile(0.90) as f64,
            p95_ms: h.value_at_quantile(0.95) as f64,
            p99_ms: h.value_at_quantile(0.99) as f64,
            p999_ms: h.value_at_quantile(0.999) as f64,
            max_ms: h.max() as f64,
        }
    }

    fn into_summary(self, url: &str, duration_s: f64, concurrency: usize) -> LoadSummary {
        let completed = self.ok_count + self.err_count;
        let error_rate = if completed > 0 {
            self.err_count as f64 / completed as f64
        } else {
            0.0
        };

        let mut stages = vec![
            Self::stage_summary("end-to-end", &self.end_to_end),
            Self::stage_summary("tier1_rtt", &self.tier1_rtt),
            Self::stage_summary("tier2_rtt", &self.tier2_rtt),
        ];
        if self.tier1_server.len() > 0 {
            stages.push(Self::stage_summary("tier1_srvr", &self.tier1_server));
        }
        if self.tier2_server.len() > 0 {
            stages.push(Self::stage_summary("tier2_srvr", &self.tier2_server));
        }

        let mut error_classes: Vec<ErrorClassCount> = self
            .error_counts
            .iter()
            .map(|(class, &count)| ErrorClassCount {
                class: class.to_string(),
                count,
            })
            .collect();
        error_classes.sort_by(|a, b| b.count.cmp(&a.count));

        LoadSummary {
            url: url.to_string(),
            duration_s,
            concurrency,
            completed,
            errors: self.err_count,
            error_rate,
            stages,
            error_classes,
        }
    }
}

// ── Driver ───────────────────────────────────────────────────────────────────

pub async fn run(cfg: LoadConfig) -> Result<()> {
    eprintln!("=== pir-test load ===\n");
    eprintln!("  url:         {}", cfg.url);
    eprintln!("  concurrency: {}", cfg.concurrency);
    if let Some(rps) = cfg.rps {
        eprintln!("  rps:         {:.1}", rps);
        eprintln!("  max_inflight:{}", cfg.max_inflight);
    }
    eprintln!(
        "  duration:    {}s",
        cfg.duration.as_secs_f64()
    );
    eprintln!(
        "  warmup:      {}s",
        cfg.warmup.as_secs_f64()
    );
    eprintln!();

    // Load nullifiers
    let nf_data = std::fs::read(&cfg.nullifiers_path)
        .with_context(|| format!("reading {:?}", cfg.nullifiers_path))?;
    let nfs = nf_ingest::file_store::parse_nullifier_bytes(&nf_data)?;
    eprintln!("  Loaded {} nullifiers", nfs.len());
    anyhow::ensure!(!nfs.is_empty(), "nullifiers file is empty");

    let ranges = pir_export::prepare_nullifiers(nfs);
    eprintln!("  Prepared {} ranges", ranges.len());

    // Build a pool of query values
    let pool = build_query_pool(&ranges, 1024, cfg.seed);
    let pool = Arc::new(pool);

    // Connect
    eprintln!("  Connecting to PIR server...");
    let client = Arc::new(PirClient::connect(&cfg.url).await?);
    eprintln!("  Connected.\n");

    // Warmup: a single proof to verify connectivity
    {
        eprintln!("  Warmup: single proof...");
        let proof = client.fetch_proof(pool[0]).await?;
        if !cfg.no_verify {
            anyhow::ensure!(proof.verify(pool[0]), "warmup proof verification failed");
        }
        eprintln!("  Warmup: OK\n");
    }

    // Extended warmup period
    if !cfg.warmup.is_zero() {
        eprintln!("  Extended warmup ({:.0}s)...", cfg.warmup.as_secs_f64());
        run_phase(
            &client,
            &pool,
            &cfg,
            cfg.warmup,
            false,
        )
        .await?;
        eprintln!("  Warmup complete.\n");
    }

    // Timed phase
    eprintln!("  Starting load phase ({:.0}s)...\n", cfg.duration.as_secs_f64());
    let summary = run_phase(
        &client,
        &pool,
        &cfg,
        cfg.duration,
        true,
    )
    .await?;

    // Print summary
    print_summary(&summary);

    // Write JSON
    if let Some(ref path) = cfg.json_out {
        let json = serde_json::to_string_pretty(&summary)?;
        std::fs::write(path, &json)
            .with_context(|| format!("writing JSON summary to {:?}", path))?;
        eprintln!("\nJSON summary written to {:?}", path);
    }

    // SLO checks
    let mut failed = false;
    if summary.error_rate > cfg.max_error_rate {
        eprintln!(
            "\nFAIL: error rate {:.2}% > threshold {:.2}%",
            summary.error_rate * 100.0,
            cfg.max_error_rate * 100.0,
        );
        failed = true;
    }
    if let Some(slo) = cfg.slo_p99_ms {
        if let Some(e2e) = summary.stages.first() {
            if e2e.p99_ms > slo {
                eprintln!(
                    "\nFAIL: p99 {:.0}ms > SLO {:.0}ms",
                    e2e.p99_ms, slo
                );
                failed = true;
            }
        }
    }
    if failed {
        anyhow::bail!("SLO check failed");
    }

    Ok(())
}

async fn run_phase(
    client: &Arc<PirClient>,
    pool: &Arc<Vec<Fp>>,
    cfg: &LoadConfig,
    duration: Duration,
    collect_stats: bool,
) -> Result<LoadSummary> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Sample>();
    let inflight = Arc::new(AtomicU64::new(0));
    let request_idx = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + duration;

    let no_verify = cfg.no_verify;

    // Progress printer (shared by both modes)
    let inflight_progress = Arc::clone(&inflight);
    let request_idx_progress = Arc::clone(&request_idx);
    let tx_progress = tx.clone();
    let progress_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.tick().await; // skip first immediate tick
        let start = Instant::now();
        while Instant::now() < deadline {
            interval.tick().await;
            if Instant::now() >= deadline || tx_progress.is_closed() {
                break;
            }
            let elapsed = start.elapsed().as_secs();
            let in_flight = inflight_progress.load(Ordering::Relaxed);
            let total = request_idx_progress.load(Ordering::Relaxed);
            eprintln!(
                "  elapsed={}s  reqs={}  in_flight={}",
                elapsed, total, in_flight,
            );
        }
    });

    if let Some(rps) = cfg.rps {
        // Open-loop: spawn tasks at a fixed rate, bounded by semaphore
        let sem = Arc::new(Semaphore::new(cfg.max_inflight));
        let interval_us = (1_000_000.0 / rps) as u64;
        let mut ticker = tokio::time::interval(Duration::from_micros(interval_us));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);

        let mut handles = Vec::new();
        while Instant::now() < deadline {
            ticker.tick().await;
            if Instant::now() >= deadline {
                break;
            }
            let permit = sem.clone().acquire_owned().await?;
            let client = Arc::clone(client);
            let pool = Arc::clone(pool);
            let tx = tx.clone();
            let inflight = Arc::clone(&inflight);
            let idx = request_idx.fetch_add(1, Ordering::Relaxed);
            handles.push(tokio::spawn(async move {
                inflight.fetch_add(1, Ordering::Relaxed);
                let sample = do_request(&client, &pool, idx, no_verify).await;
                inflight.fetch_sub(1, Ordering::Relaxed);
                let _ = tx.send(sample);
                drop(permit);
            }));
        }
        // Wait for all in-flight tasks to finish
        for h in handles {
            let _ = h.await;
        }
    } else {
        // Closed-loop: N persistent workers
        let mut handles = Vec::with_capacity(cfg.concurrency);
        for _ in 0..cfg.concurrency {
            let client = Arc::clone(client);
            let pool = Arc::clone(pool);
            let tx = tx.clone();
            let inflight = Arc::clone(&inflight);
            let request_idx = Arc::clone(&request_idx);
            handles.push(tokio::spawn(async move {
                while Instant::now() < deadline {
                    let idx = request_idx.fetch_add(1, Ordering::Relaxed);
                    inflight.fetch_add(1, Ordering::Relaxed);
                    let sample = do_request(&client, &pool, idx, no_verify).await;
                    inflight.fetch_sub(1, Ordering::Relaxed);
                    if tx.send(sample).is_err() {
                        break;
                    }
                }
            }));
        }

        // Wait for all workers
        for h in handles {
            let _ = h.await;
        }
    }

    progress_handle.abort();
    let _ = progress_handle.await;

    // Drop the original sender so the collector can finish
    drop(tx);

    // Collect stats
    let mut collector = StatsCollector::new();
    if collect_stats {
        while let Some(sample) = rx.recv().await {
            collector.record(sample);
        }
    } else {
        // Drain channel
        while rx.recv().await.is_some() {}
    }

    Ok(collector.into_summary(&cfg.url, duration.as_secs_f64(), cfg.concurrency))
}

async fn do_request(client: &PirClient, pool: &[Fp], idx: u64, no_verify: bool) -> Sample {
    let value = pool[(idx as usize) % pool.len()];
    let t0 = Instant::now();

    match client.fetch_proof_with_timing(value).await {
        Ok((proof, timing)) => {
            let verified = no_verify || proof.verify(value);
            if !verified {
                return Sample {
                    total_ms: t0.elapsed().as_secs_f64() * 1000.0,
                    tier1_rtt_ms: timing.tier1.rtt_ms,
                    tier2_rtt_ms: timing.tier2.rtt_ms,
                    tier1_server_ms: timing.tier1.server_total_ms,
                    tier2_server_ms: timing.tier2.server_total_ms,
                    success: false,
                    error_class: Some(ErrorClass::VerifyFail),
                };
            }
            Sample {
                total_ms: timing.total_ms,
                tier1_rtt_ms: timing.tier1.rtt_ms,
                tier2_rtt_ms: timing.tier2.rtt_ms,
                tier1_server_ms: timing.tier1.server_total_ms,
                tier2_server_ms: timing.tier2.server_total_ms,
                success: true,
                error_class: None,
            }
        }
        Err(e) => {
            let class = classify_error(&e);
            Sample {
                total_ms: t0.elapsed().as_secs_f64() * 1000.0,
                tier1_rtt_ms: 0.0,
                tier2_rtt_ms: 0.0,
                tier1_server_ms: None,
                tier2_server_ms: None,
                success: false,
                error_class: Some(class),
            }
        }
    }
}

fn build_query_pool(ranges: &[[Fp; 3]], size: usize, seed: Option<u64>) -> Vec<Fp> {
    use rand::SeedableRng;

    let mut rng: Box<dyn rand::RngCore> = if let Some(s) = seed {
        Box::new(rand::rngs::StdRng::seed_from_u64(s))
    } else {
        Box::new(rand::thread_rng())
    };

    (0..size)
        .map(|_| {
            use rand::Rng;
            let idx = rng.gen_range(0..ranges.len());
            let [nf_lo, _, _] = ranges[idx];
            nf_lo + Fp::one()
        })
        .collect()
}

fn print_summary(s: &LoadSummary) {
    eprintln!("\n=== pir-test load summary ===");
    eprintln!(
        "url={}   duration={:.0}s   concurrency={}   completed={}   errors={} ({:.2}%)",
        s.url,
        s.duration_s,
        s.concurrency,
        s.completed,
        s.errors,
        s.error_rate * 100.0,
    );

    eprintln!(
        "  {:>14} {:>10} {:>10} {:>10} {:>10} {:>10} {:>8}",
        "", "p50", "p90", "p95", "p99", "max", "n"
    );
    for stage in &s.stages {
        eprintln!(
            "  {:>14} {:>10} {:>10} {:>10} {:>10} {:>10} {:>8}",
            stage.name,
            format_ms(stage.p50_ms),
            format_ms(stage.p90_ms),
            format_ms(stage.p95_ms),
            format_ms(stage.p99_ms),
            format_ms(stage.max_ms),
            stage.n,
        );
    }

    if !s.error_classes.is_empty() {
        let parts: Vec<String> = s
            .error_classes
            .iter()
            .map(|ec| format!("{}={}", ec.class, ec.count))
            .collect();
        eprintln!("errors by class: {}", parts.join(" "));
    }
}

fn format_ms(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2}s", ms / 1000.0)
    } else {
        format!("{:.0}ms", ms)
    }
}
