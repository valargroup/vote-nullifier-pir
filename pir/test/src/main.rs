//! PIR E2E test harness.
//!
//! Modes:
//!   small         — Synthetic 1000-nullifier tree, full round-trip (~5s)
//!   local         — Full in-process test with real nullifiers (no HTTP, no YPIR crypto)
//!   server        — Test against a running pir-server instance (HTTP + YPIR crypto)
//!   bench-server  — Closed-loop iteration-bounded RTT/bandwidth/server-compute baseline
//!   load          — Drive concurrent PIR traffic for load testing

mod bench_server;
mod load;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use ff::{Field, PrimeField as _};
use pasta_curves::Fp;
use rand::Rng;

use pir_export::build_pir_tree;
use pir_types::{
    TIER1_ITEM_BITS, TIER1_ROWS, TIER1_ROW_BYTES, TIER2_ITEM_BITS, TIER2_ROWS, TIER2_ROW_BYTES,
};

#[derive(Parser)]
#[command(name = "pir-test", about = "PIR system end-to-end testing")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum BenchModeArg {
    /// Issue all K queries concurrently within one iteration.
    Parallel,
    /// Issue K queries one at a time within one iteration.
    Sequential,
    /// Force K=1 (sequential of size 1).
    Single,
    /// Issue K queries one at a time over a single HTTP/1.1 TCP/TLS
    /// connection (no HTTP/2 stream multiplexing). Used to disambiguate
    /// HTTP/2 contention vs per-query upload bandwidth as the bottleneck.
    SingleTls,
}

impl From<BenchModeArg> for bench_server::BenchMode {
    fn from(v: BenchModeArg) -> Self {
        match v {
            BenchModeArg::Parallel => bench_server::BenchMode::Parallel,
            BenchModeArg::Sequential => bench_server::BenchMode::Sequential,
            BenchModeArg::Single => bench_server::BenchMode::Single,
            BenchModeArg::SingleTls => bench_server::BenchMode::SingleTls,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Synthetic 1000-nullifier tree, fast round-trip test (~5s).
    Small,

    /// Full in-process test with real or synthetic nullifiers.
    /// Tests tier data extraction and proof construction without YPIR crypto.
    Local {
        /// Path to nullifiers.bin. If omitted, generates 10,000 random nullifiers.
        #[arg(long)]
        nullifiers: Option<PathBuf>,

        /// Number of proofs to generate and verify.
        #[arg(long, default_value = "10")]
        num_proofs: usize,
    },

    /// Test against a running pir-server instance.
    Server {
        /// Server URL (e.g., http://localhost:3000).
        #[arg(long)]
        url: String,

        /// Path to nullifiers.bin (to know which values to query).
        #[arg(long)]
        nullifiers: PathBuf,

        /// Number of proofs to generate and verify.
        #[arg(long, default_value = "5")]
        num_proofs: usize,

        /// If set, fetch all proofs in a single parallel PIR request batch.
        #[arg(long, default_value_t = false)]
        parallel: bool,
    },

    /// Verify YPIR round-trip correctness by comparing decoded rows with originals.
    VerifyYpir,

    /// Benchmark YPIR query/response sizes and timing in-process (no HTTP).
    Bench {
        /// Number of YPIR queries per tier.
        #[arg(long, default_value = "3")]
        num_queries: usize,
    },

    /// Benchmark multiple tier split configurations to compare sizes/timing.
    BenchSplits {
        /// Number of YPIR queries per tier per configuration.
        #[arg(long, default_value = "1")]
        num_queries: usize,

        /// Run only a specific config, e.g. "11-7-7". Omit to run all.
        #[arg(long)]
        config: Option<String>,
    },

    /// Closed-loop, iteration-bounded latency / bandwidth / server-compute
    /// baseline against a running pir-server. Emits a JSON summary intended
    /// to be checked into the repo as a baseline for diff later (e.g.
    /// before/after PIR batch-query rollout).
    BenchServer {
        /// Server URL (e.g., https://pir.valargroup.org).
        #[arg(long)]
        url: String,

        /// Path to nullifiers.bin (used to pick `nf_lo + 1` query values).
        #[arg(long)]
        nullifiers: PathBuf,

        /// Number of measured iterations.
        #[arg(long, default_value = "30")]
        iterations: usize,

        /// Number of warmup iterations whose timings are discarded.
        #[arg(long, default_value = "3")]
        warmup: usize,

        /// Number of PIR proofs to fetch per iteration (K).
        #[arg(long, default_value = "5")]
        batch_size: usize,

        /// How to issue the K queries within an iteration.
        #[arg(long, default_value = "parallel")]
        mode: BenchModeArg,

        /// Deterministic RNG seed for query selection.
        #[arg(long)]
        seed: Option<u64>,

        /// Write JSON summary to this path.
        #[arg(long)]
        json_out: Option<PathBuf>,

        /// Free-form label used in the JSON output and stdout summary.
        #[arg(long)]
        label: Option<String>,
    },

    /// Drive concurrent PIR traffic against a running server for load testing.
    Load {
        /// Server URL (e.g., http://localhost:3000).
        #[arg(long)]
        url: String,

        /// Path to nullifiers.bin (to know which values to query).
        #[arg(long)]
        nullifiers: PathBuf,

        /// Number of concurrent workers (closed-loop mode, default).
        #[arg(long, default_value = "8")]
        concurrency: usize,

        /// Target requests per second (open-loop mode). If omitted, uses closed-loop.
        #[arg(long)]
        rps: Option<f64>,

        /// Max in-flight requests for open-loop mode.
        #[arg(long, default_value = "256")]
        max_inflight: usize,

        /// How long to sustain load.
        #[arg(long, default_value = "60s", value_parser = parse_duration)]
        duration: Duration,

        /// Warmup period before timed measurement begins.
        #[arg(long, default_value = "10s", value_parser = parse_duration)]
        warmup: Duration,

        /// Write JSON summary to this path.
        #[arg(long)]
        json_out: Option<PathBuf>,

        /// Skip proof verification (only measure transport + crypto latency).
        #[arg(long, default_value_t = false)]
        no_verify: bool,

        /// Deterministic RNG seed for query selection.
        #[arg(long)]
        seed: Option<u64>,

        /// Fail if error rate exceeds this fraction (0.0–1.0).
        #[arg(long, default_value = "0.01")]
        max_error_rate: f64,

        /// Fail if end-to-end p99 exceeds this many milliseconds.
        #[arg(long)]
        slo_p99_ms: Option<f64>,
    },
}

fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::Builder::from_default_env()
        .format_timestamp_millis()
        .try_init()
        .ok();

    let args = Args::parse();

    match args.command {
        Command::Small => run_small(),
        Command::Local {
            nullifiers,
            num_proofs,
        } => run_local(nullifiers, num_proofs),
        Command::Server {
            url,
            nullifiers,
            num_proofs,
            parallel,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run_server(url, nullifiers, num_proofs, parallel))
        }
        Command::VerifyYpir => run_verify_ypir(),
        Command::Bench { num_queries } => run_bench(num_queries),
        Command::BenchSplits { num_queries, config } => run_bench_splits(num_queries, config),
        Command::BenchServer {
            url,
            nullifiers,
            iterations,
            warmup,
            batch_size,
            mode,
            seed,
            json_out,
            label,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(bench_server::run(bench_server::BenchConfig {
                url,
                nullifiers_path: nullifiers,
                iterations,
                warmup,
                batch_size: if matches!(mode, BenchModeArg::Single) {
                    1
                } else {
                    batch_size
                },
                mode: mode.into(),
                seed,
                json_out,
                label,
            }))
        }
        Command::Load {
            url,
            nullifiers,
            concurrency,
            rps,
            max_inflight,
            duration,
            warmup,
            json_out,
            no_verify,
            seed,
            max_error_rate,
            slo_p99_ms,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(load::run(load::LoadConfig {
                url,
                nullifiers_path: nullifiers,
                concurrency,
                rps,
                max_inflight,
                duration,
                warmup,
                json_out,
                no_verify,
                seed,
                max_error_rate,
                slo_p99_ms,
            }))
        }
    }
}

fn parse_duration(s: &str) -> Result<Duration, humantime::DurationError> {
    humantime::parse_duration(s)
}

// ── Small mode ───────────────────────────────────────────────────────────────

fn run_small() -> Result<()> {
    eprintln!("=== PIR Test: small (synthetic 1000 nullifiers) ===\n");
    let t_total = Instant::now();

    // Generate 1000 random nullifiers
    let mut rng = rand::thread_rng();
    let nfs: Vec<Fp> = (0..1000).map(|_| Fp::random(&mut rng)).collect();

    run_local_inner(&nfs, 10)?;

    eprintln!(
        "\n=== PASSED in {:.1}s ===",
        t_total.elapsed().as_secs_f64()
    );
    Ok(())
}

// ── Local mode ───────────────────────────────────────────────────────────────

fn run_local(nullifiers_path: Option<PathBuf>, num_proofs: usize) -> Result<()> {
    eprintln!("=== PIR Test: local ===\n");

    let nfs = if let Some(path) = nullifiers_path {
        eprintln!("Loading nullifiers from {:?}...", path);
        load_nullifiers(&path)?
    } else {
        eprintln!("Generating 10,000 random nullifiers...");
        let mut rng = rand::thread_rng();
        (0..10_000).map(|_| Fp::random(&mut rng)).collect()
    };

    run_local_inner(&nfs, num_proofs)?;

    eprintln!("\n=== PASSED ===");
    Ok(())
}

fn run_local_inner(raw_nfs: &[Fp], num_proofs: usize) -> Result<()> {
    let t0 = Instant::now();

    let ranges = pir_export::prepare_nullifiers(raw_nfs.to_vec());
    eprintln!(
        "  {} ranges from {} nullifiers in {:.1}s",
        ranges.len(),
        raw_nfs.len(),
        t0.elapsed().as_secs_f64()
    );

    // Build PIR tree
    let t1 = Instant::now();
    let tree = build_pir_tree(ranges.clone())?;
    eprintln!(
        "  PIR tree built in {:.1}s (root25={}, root29={})",
        t1.elapsed().as_secs_f64(),
        &hex::encode(tree.root25.to_repr())[..16],
        &hex::encode(tree.root29.to_repr())[..16],
    );

    let t2 = Instant::now();
    let (tier0_data, tier1_data, tier2_data) = export_tiers(&tree)?;
    eprintln!("  Exported in {:.1}s", t2.elapsed().as_secs_f64());

    // Pick random values from populated ranges to query
    let mut rng = rand::thread_rng();
    let test_values: Vec<Fp> = (0..num_proofs)
        .map(|_| {
            let idx = rng.gen_range(0..ranges.len());
            let [nf_lo, _nf_mid, nf_hi] = ranges[idx];
            // Pick a random value strictly between nf_lo and nf_hi (truncated to u64).
            let span_u64 = u64::from_le_bytes((nf_hi - nf_lo).to_repr()[..8].try_into().unwrap());
            let offset_val = if span_u64 > 2 {
                rng.gen_range(1..span_u64.min(u64::MAX - 1))
            } else {
                1
            };
            nf_lo + Fp::from(offset_val)
        })
        .collect();

    // Test each proof
    let mut passed = 0;
    let mut failed = 0;

    for (i, &value) in test_values.iter().enumerate() {
        let t_proof = Instant::now();

        let result = pir_client::fetch_proof_local(
            &tier0_data,
            &tier1_data,
            &tier2_data,
            ranges.len(),
            value,
            &tree.empty_hashes,
            tree.root29,
        );

        match result {
            Ok(proof) => {
                if proof.verify(value) {
                    passed += 1;
                    eprintln!(
                        "  Proof {}/{}: PASS ({:.1}ms) leaf_pos={}",
                        i + 1,
                        num_proofs,
                        t_proof.elapsed().as_secs_f64() * 1000.0,
                        proof.leaf_pos,
                    );
                } else {
                    failed += 1;
                    eprintln!(
                        "  Proof {}/{}: FAIL (verify returned false) leaf_pos={}",
                        i + 1,
                        num_proofs,
                        proof.leaf_pos,
                    );
                }
            }
            Err(e) => {
                failed += 1;
                eprintln!("  Proof {}/{}: ERROR: {}", i + 1, num_proofs, e);
            }
        }
    }

    eprintln!("\n  Summary: {} passed, {} failed", passed, failed);
    if failed > 0 {
        anyhow::bail!("{} proofs failed", failed);
    }

    Ok(())
}

// ── Server mode ──────────────────────────────────────────────────────────────

async fn run_server(
    url: String,
    nullifiers_path: PathBuf,
    num_proofs: usize,
    parallel: bool,
) -> Result<()> {
    eprintln!("=== PIR Test: server ({}) ===\n", url);

    let nfs = load_nullifiers(&nullifiers_path)?;
    eprintln!("  Loaded {} nullifiers", nfs.len());

    // Connect to server
    let client = pir_client::PirClient::connect(&url).await?;
    eprintln!("  Connected to PIR server");

    // Pick random values
    let ranges = pir_export::prepare_nullifiers(nfs);

    let mut rng = rand::thread_rng();
    let test_values: Vec<Fp> = (0..num_proofs)
        .map(|_| {
            let idx = rng.gen_range(0..ranges.len());
            let [nf_lo, _, _] = ranges[idx];
            nf_lo + Fp::one() // nf_lo + 1 is always in the punctured range
        })
        .collect();

    let mut passed = 0usize;
    let mut failed = 0usize;

    if parallel {
        let t0 = Instant::now();
        let proofs = client.fetch_proofs(&test_values).await?;
        anyhow::ensure!(
            proofs.len() == test_values.len(),
            "parallel fetch returned {} proofs for {} queries",
            proofs.len(),
            test_values.len()
        );
        for (i, (&value, proof)) in test_values.iter().zip(proofs.iter()).enumerate() {
            if proof.verify(value) {
                passed += 1;
            } else {
                failed += 1;
                eprintln!("  Proof {}/{}: FAIL (verify false)", i + 1, num_proofs);
            }
        }
        eprintln!(
            "  Parallel batch: {}/{} valid ({:.1}ms total)",
            passed,
            num_proofs,
            t0.elapsed().as_secs_f64() * 1000.0
        );
    } else {
        for (i, &value) in test_values.iter().enumerate() {
            let t0 = Instant::now();
            match client.fetch_proof(value).await {
                Ok(proof) => {
                    if proof.verify(value) {
                        passed += 1;
                        eprintln!(
                            "  Proof {}/{}: PASS ({:.1}ms)",
                            i + 1,
                            num_proofs,
                            t0.elapsed().as_secs_f64() * 1000.0,
                        );
                    } else {
                        failed += 1;
                        eprintln!("  Proof {}/{}: FAIL (verify false)", i + 1, num_proofs);
                    }
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("  Proof {}/{}: ERROR: {}", i + 1, num_proofs, e);
                }
            }
        }
    }

    eprintln!("\n  Summary: {} passed, {} failed", passed, failed);
    if failed > 0 {
        anyhow::bail!("{} proofs failed", failed);
    }

    eprintln!("\n=== PASSED ===");
    Ok(())
}

// ── Verify YPIR mode ─────────────────────────────────────────────────────────

fn run_verify_ypir() -> Result<()> {
    use pir_server::OwnedTierState;
    use ypir::client::YPIRClient;

    eprintln!("=== YPIR Round-Trip Verification ===\n");

    let mut rng = rand::thread_rng();
    let raw_nfs: Vec<Fp> = (0..1000).map(|_| Fp::random(&mut rng)).collect();
    let ranges = pir_export::prepare_nullifiers(raw_nfs);
    let tree = build_pir_tree(ranges)?;
    let (_, tier1_data, tier2_data) = export_tiers(&tree)?;

    let tier1_scenario = pir_server::tier1_scenario();
    let tier2_scenario = pir_server::tier2_scenario();

    eprintln!("Initializing Tier 1 YPIR server...");
    let tier1_server = OwnedTierState::new(&tier1_data, tier1_scenario.clone());

    for row_idx in [0usize, 1, 100, TIER1_ROWS - 1] {
        let ypir_client = YPIRClient::from_db_sz(
            tier1_scenario.num_items as u64,
            tier1_scenario.item_size_bits as u64,
            true,
        );
        let (query, seed) = ypir_client.generate_query_simplepir(row_idx);
        let payload = pir_types::serialize_ypir_query(query.0.as_slice(), query.1.as_slice());
        let answer = tier1_server.server().answer_query(&payload)?;
        let decoded = ypir_client.decode_response_simplepir(seed, &answer.response);

        let original = &tier1_data[row_idx * TIER1_ROW_BYTES..(row_idx + 1) * TIER1_ROW_BYTES];
        let decoded_row = &decoded[..TIER1_ROW_BYTES];

        if original == decoded_row {
            eprintln!("  Tier 1 row {}: MATCH", row_idx);
        } else {
            let mismatches: Vec<usize> = original
                .iter()
                .zip(decoded_row.iter())
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, _)| i)
                .collect();
            eprintln!(
                "  Tier 1 row {}: MISMATCH at {} byte positions (first: {})",
                row_idx,
                mismatches.len(),
                mismatches.first().unwrap_or(&0)
            );
            if let Some(&first) = mismatches.first() {
                eprintln!(
                    "    original[{}..{}]: {:02x?}",
                    first,
                    (first + 16).min(TIER1_ROW_BYTES),
                    &original[first..(first + 16).min(TIER1_ROW_BYTES)]
                );
                eprintln!(
                    "    decoded [{}..{}]: {:02x?}",
                    first,
                    (first + 16).min(TIER1_ROW_BYTES),
                    &decoded_row[first..(first + 16).min(TIER1_ROW_BYTES)]
                );
            }
        }
    }

    drop(tier1_server);

    eprintln!("\nInitializing Tier 2 YPIR server...");
    let tier2_server = OwnedTierState::new(&tier2_data, tier2_scenario.clone());

    for row_idx in [0usize, 1, 100] {
        let ypir_client = YPIRClient::from_db_sz(
            tier2_scenario.num_items as u64,
            tier2_scenario.item_size_bits as u64,
            true,
        );
        let (query, seed) = ypir_client.generate_query_simplepir(row_idx);
        let payload = pir_types::serialize_ypir_query(query.0.as_slice(), query.1.as_slice());
        let answer = tier2_server.server().answer_query(&payload)?;
        let decoded = ypir_client.decode_response_simplepir(seed, &answer.response);

        let original = &tier2_data[row_idx * TIER2_ROW_BYTES..(row_idx + 1) * TIER2_ROW_BYTES];
        let decoded_row = &decoded[..TIER2_ROW_BYTES];

        if original == decoded_row {
            eprintln!("  Tier 2 row {}: MATCH", row_idx);
        } else {
            let mismatches: Vec<usize> = original
                .iter()
                .zip(decoded_row.iter())
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, _)| i)
                .collect();
            eprintln!(
                "  Tier 2 row {}: MISMATCH at {} byte positions (first: {})",
                row_idx,
                mismatches.len(),
                mismatches.first().unwrap_or(&0)
            );
            if let Some(&first) = mismatches.first() {
                eprintln!(
                    "    original[{}..{}]: {:02x?}",
                    first,
                    (first + 16).min(TIER2_ROW_BYTES),
                    &original[first..(first + 16).min(TIER2_ROW_BYTES)]
                );
                eprintln!(
                    "    decoded [{}..{}]: {:02x?}",
                    first,
                    (first + 16).min(TIER2_ROW_BYTES),
                    &decoded_row[first..(first + 16).min(TIER2_ROW_BYTES)]
                );
            }
        }
    }

    drop(tier2_server);

    eprintln!("\n=== Done ===");
    Ok(())
}

// ── Bench mode ───────────────────────────────────────────────────────────────

fn run_bench(num_queries: usize) -> Result<()> {
    use pir_server::OwnedTierState;

    let tier1_scenario = pir_server::tier1_scenario();
    let tier2_scenario = pir_server::tier2_scenario();

    eprintln!("=== PIR Benchmark: in-process YPIR ({} queries per tier) ===\n", num_queries);
    eprintln!(
        "  Config: TIER1_LAYERS={}, TIER2_LAYERS={}",
        pir_types::TIER1_LAYERS,
        pir_types::TIER2_LAYERS
    );
    eprintln!(
        "  Tier 1: {} rows × {} bytes/row ({} bits/item), instances={}",
        TIER1_ROWS,
        TIER1_ROW_BYTES,
        TIER1_ITEM_BITS,
        (TIER1_ITEM_BITS as f64 / (2048.0 * 14.0)).ceil() as usize,
    );
    eprintln!(
        "  Tier 2: {} rows × {} bytes/row ({} bits/item), instances={}",
        TIER2_ROWS,
        TIER2_ROW_BYTES,
        TIER2_ITEM_BITS,
        (TIER2_ITEM_BITS as f64 / (2048.0 * 14.0)).ceil() as usize,
    );

    // Build a small tree to get valid tier data
    eprintln!("\nBuilding synthetic tree (1000 nullifiers)...");
    let mut rng = rand::thread_rng();
    let raw_nfs: Vec<Fp> = (0..1000).map(|_| Fp::random(&mut rng)).collect();
    let ranges = pir_export::prepare_nullifiers(raw_nfs);
    let tree = build_pir_tree(ranges)?;

    let (_, tier1_data, tier2_data) = export_tiers(&tree)?;

    // Initialize YPIR servers
    eprintln!("\nInitializing YPIR servers...");
    let t0 = Instant::now();
    let tier1_server = OwnedTierState::new(&tier1_data, tier1_scenario.clone());
    eprintln!("  Tier 1 YPIR server ready in {:.1}s", t0.elapsed().as_secs_f64());
    drop(tier1_data);

    let t0 = Instant::now();
    let tier2_server = OwnedTierState::new(&tier2_data, tier2_scenario.clone());
    eprintln!("  Tier 2 YPIR server ready in {:.1}s", t0.elapsed().as_secs_f64());
    drop(tier2_data);

    // Run tier 1 benchmarks
    eprintln!("\n── Tier 1 YPIR Benchmark ──────────────────────────────────");
    let tier1_results = bench_tier(
        "tier1",
        tier1_scenario.num_items,
        tier1_scenario.item_size_bits,
        tier1_server.server(),
        num_queries,
    )?;

    // Run tier 2 benchmarks
    eprintln!("\n── Tier 2 YPIR Benchmark ──────────────────────────────────");
    let tier2_results = bench_tier(
        "tier2",
        tier2_scenario.num_items,
        tier2_scenario.item_size_bits,
        tier2_server.server(),
        num_queries,
    )?;

    // Summary table
    eprintln!("\n══════════════════════════════════════════════════════════════");
    eprintln!("  SUMMARY (averages over {} queries)", num_queries);
    eprintln!("══════════════════════════════════════════════════════════════");
    eprintln!(
        "  {:>10} {:>12} {:>12} {:>10} {:>10} {:>10}",
        "", "Query(up)", "Response(dn)", "ClientGen", "ServerComp", "ClientDec"
    );
    eprintln!(
        "  {:>10} {:>12} {:>12} {:>10} {:>10} {:>10}",
        "Tier 1",
        format_bytes(tier1_results.avg_query_bytes),
        format_bytes(tier1_results.avg_response_bytes),
        format_ms(tier1_results.avg_gen_ms),
        format_ms(tier1_results.avg_server_ms),
        format_ms(tier1_results.avg_decode_ms),
    );
    eprintln!(
        "  {:>10} {:>12} {:>12} {:>10} {:>10} {:>10}",
        "Tier 2",
        format_bytes(tier2_results.avg_query_bytes),
        format_bytes(tier2_results.avg_response_bytes),
        format_ms(tier2_results.avg_gen_ms),
        format_ms(tier2_results.avg_server_ms),
        format_ms(tier2_results.avg_decode_ms),
    );
    eprintln!(
        "  {:>10} {:>12} {:>12}",
        "TOTAL",
        format_bytes(tier1_results.avg_query_bytes + tier2_results.avg_query_bytes),
        format_bytes(tier1_results.avg_response_bytes + tier2_results.avg_response_bytes),
    );
    eprintln!("══════════════════════════════════════════════════════════════");

    Ok(())
}

// ── Bench-splits mode ────────────────────────────────────────────────────────

struct SplitConfig {
    t0: usize,
    t1: usize,
    t2: usize,
}

impl SplitConfig {
    fn tier1_logical_rows(&self) -> usize { 1 << self.t0 }
    /// YPIR requires at least poly_len=2048 rows; pad up if t0 < 11.
    fn tier1_rows(&self) -> usize { (1usize << self.t0).max(2048) }
    fn tier2_rows(&self) -> usize { 1 << (self.t0 + self.t1) }
    fn tier1_leaves(&self) -> usize { 1 << self.t1 }
    fn tier2_leaves(&self) -> usize { 1 << self.t2 }
    fn tier1_row_bytes(&self) -> usize { self.tier1_leaves() * 64 }
    fn tier2_row_bytes(&self) -> usize { self.tier2_leaves() * 96 }
    /// YPIR requires item_size_bits >= 2048*14 = 28672 (one SimplePIR column).
    fn tier1_item_bits(&self) -> usize { (self.tier1_row_bytes() * 8).max(28672) }
    fn tier2_item_bits(&self) -> usize { (self.tier2_row_bytes() * 8).max(28672) }
    fn tier1_db_bytes(&self) -> usize { self.tier1_rows() * (self.tier1_item_bits() / 8) }
    fn tier2_db_bytes(&self) -> usize { self.tier2_rows() * (self.tier2_item_bits() / 8) }

    fn tier0_bytes(&self) -> usize {
        let internal_nodes = (1usize << self.t0) - 1;
        internal_nodes * 32 + self.tier1_rows() * 64
    }

    fn label(&self) -> String {
        format!("{}-{}-{}", self.t0, self.t1, self.t2)
    }
}

struct SplitResults {
    config: SplitConfig,
    tier1: BenchResults,
    tier2: BenchResults,
    tier1_init_s: f64,
    tier2_init_s: f64,
}

fn run_bench_splits(num_queries: usize, filter: Option<String>) -> Result<()> {
    use pir_server::OwnedTierState;
    use pir_types::YpirScenario;

    let all_configs = vec![
        SplitConfig { t0: 11, t1: 7, t2: 7 },
        SplitConfig { t0: 10, t1: 6, t2: 9 },
        SplitConfig { t0: 9, t1: 6, t2: 10 },
        SplitConfig { t0: 9, t1: 7, t2: 9 },
        SplitConfig { t0: 8, t1: 6, t2: 11 },
        SplitConfig { t0: 8, t1: 7, t2: 10 },
        SplitConfig { t0: 10, t1: 5, t2: 10 },
        SplitConfig { t0: 10, t1: 4, t2: 11 },
    ];

    let configs: Vec<SplitConfig> = if let Some(ref f) = filter {
        all_configs.into_iter().filter(|c| c.label() == *f).collect()
    } else {
        all_configs
    };

    if configs.is_empty() {
        anyhow::bail!("no config matched filter {:?}", filter);
    }

    eprintln!("=== PIR Split Comparison ({} queries per tier per config) ===\n", num_queries);

    let mut results = Vec::new();

    for cfg in &configs {
        assert_eq!(cfg.t0 + cfg.t1 + cfg.t2, 25, "splits must sum to PIR_DEPTH=25");

        let t1_scenario = YpirScenario {
            num_items: cfg.tier1_rows(),
            item_size_bits: cfg.tier1_item_bits(),
        };
        let t2_scenario = YpirScenario {
            num_items: cfg.tier2_rows(),
            item_size_bits: cfg.tier2_item_bits(),
        };

        eprintln!("── Config {} ──────────────────────────────────────────", cfg.label());
        eprintln!(
            "  Tier 0: {} (plaintext download)",
            format_bytes(cfg.tier0_bytes()),
        );
        let pad_note = if cfg.tier1_rows() > cfg.tier1_logical_rows() {
            format!(" (padded from {} logical rows)", cfg.tier1_logical_rows())
        } else {
            String::new()
        };
        eprintln!(
            "  Tier 1: {} rows{} × {} B/row = {}, item_bits={}",
            cfg.tier1_rows(), pad_note, cfg.tier1_row_bytes(),
            format_bytes(cfg.tier1_db_bytes()), cfg.tier1_item_bits(),
        );
        eprintln!(
            "  Tier 2: {} rows × {} B/row = {}, item_bits={}",
            cfg.tier2_rows(), cfg.tier2_row_bytes(),
            format_bytes(cfg.tier2_db_bytes()), cfg.tier2_item_bits(),
        );

        // Tier 1: create zeroed dummy data of the right size
        eprintln!("  Initializing Tier 1 YPIR server...");
        let t1_data = vec![0u8; cfg.tier1_db_bytes()];
        let t0 = Instant::now();
        let t1_server = OwnedTierState::new(&t1_data, t1_scenario.clone());
        let tier1_init_s = t0.elapsed().as_secs_f64();
        eprintln!("  Tier 1 ready in {:.1}s", tier1_init_s);
        drop(t1_data);

        let tier1_results = bench_tier(
            "tier1",
            t1_scenario.num_items,
            t1_scenario.item_size_bits,
            t1_server.server(),
            num_queries,
        )?;

        drop(t1_server);

        // Tier 2: create zeroed dummy data of the right size
        eprintln!("  Initializing Tier 2 YPIR server...");
        let t2_data = vec![0u8; cfg.tier2_db_bytes()];
        let t0 = Instant::now();
        let t2_server = OwnedTierState::new(&t2_data, t2_scenario.clone());
        let tier2_init_s = t0.elapsed().as_secs_f64();
        eprintln!("  Tier 2 ready in {:.1}s", tier2_init_s);
        drop(t2_data);

        let tier2_results = bench_tier(
            "tier2",
            t2_scenario.num_items,
            t2_scenario.item_size_bits,
            t2_server.server(),
            num_queries,
        )?;

        drop(t2_server);

        results.push(SplitResults {
            config: SplitConfig { t0: cfg.t0, t1: cfg.t1, t2: cfg.t2 },
            tier1: tier1_results,
            tier2: tier2_results,
            tier1_init_s,
            tier2_init_s,
        });

        eprintln!();
    }

    // Print comparison table
    eprintln!("══════════════════════════════════════════════════════════════════════════════════════════════════════");
    eprintln!("  COMPARISON TABLE");
    eprintln!("══════════════════════════════════════════════════════════════════════════════════════════════════════");
    eprintln!(
        "  {:>7} {:>10} {:>10} {:>12} {:>12} {:>12} {:>10} {:>10} {:>10}",
        "Split", "Tier0(dn)", "T1 DB", "T2 DB", "Query(up)", "Resp(dn)", "T1 Srvr", "T2 Srvr", "T2 Init"
    );
    eprintln!("  {}", "-".repeat(100));
    for r in &results {
        eprintln!(
            "  {:>7} {:>10} {:>10} {:>12} {:>12} {:>12} {:>10} {:>10} {:>10}",
            r.config.label(),
            format_bytes(r.config.tier0_bytes()),
            format_bytes(r.config.tier1_db_bytes()),
            format_bytes(r.config.tier2_db_bytes()),
            format_bytes(r.tier1.avg_query_bytes + r.tier2.avg_query_bytes),
            format_bytes(r.tier1.avg_response_bytes + r.tier2.avg_response_bytes),
            format_ms(r.tier1.avg_server_ms),
            format_ms(r.tier2.avg_server_ms),
            format_ms(r.tier2_init_s * 1000.0),
        );
    }
    eprintln!("══════════════════════════════════════════════════════════════════════════════════════════════════════");

    // Per-tier breakdown
    eprintln!("\n  PER-TIER DETAIL");
    eprintln!("  {}", "-".repeat(100));
    eprintln!(
        "  {:>7} {:>12} {:>12} {:>12} {:>12} {:>10} {:>10} {:>10} {:>10}",
        "Split", "T1 Q(up)", "T1 R(dn)", "T2 Q(up)", "T2 R(dn)", "T1 Gen", "T2 Gen", "T1 Dec", "T2 Dec"
    );
    eprintln!("  {}", "-".repeat(100));
    for r in &results {
        eprintln!(
            "  {:>7} {:>12} {:>12} {:>12} {:>12} {:>10} {:>10} {:>10} {:>10}",
            r.config.label(),
            format_bytes(r.tier1.avg_query_bytes),
            format_bytes(r.tier1.avg_response_bytes),
            format_bytes(r.tier2.avg_query_bytes),
            format_bytes(r.tier2.avg_response_bytes),
            format_ms(r.tier1.avg_gen_ms),
            format_ms(r.tier2.avg_gen_ms),
            format_ms(r.tier1.avg_decode_ms),
            format_ms(r.tier2.avg_decode_ms),
        );
    }
    eprintln!("══════════════════════════════════════════════════════════════════════════════════════════════════════");

    Ok(())
}

struct BenchResults {
    avg_query_bytes: usize,
    avg_response_bytes: usize,
    avg_gen_ms: f64,
    avg_server_ms: f64,
    avg_decode_ms: f64,
}

fn bench_tier(
    name: &str,
    num_items: usize,
    item_size_bits: usize,
    server: &pir_server::TierServer<'static>,
    num_queries: usize,
) -> Result<BenchResults> {
    use ypir::client::YPIRClient;

    let ypir_client = YPIRClient::from_db_sz(num_items as u64, item_size_bits as u64, true);

    let mut total_query_bytes = 0usize;
    let mut total_response_bytes = 0usize;
    let mut total_gen_ms = 0.0f64;
    let mut total_server_ms = 0.0f64;
    let mut total_decode_ms = 0.0f64;

    for i in 0..num_queries {
        let row_idx = i % num_items;

        // Client: generate query
        let t_gen = Instant::now();
        let (query, seed) = ypir_client.generate_query_simplepir(row_idx);
        let gen_ms = t_gen.elapsed().as_secs_f64() * 1000.0;

        let payload = pir_types::serialize_ypir_query(query.0.as_slice(), query.1.as_slice());
        let query_bytes = payload.len();

        // Server: answer query
        let t_server = Instant::now();
        let answer = server.answer_query(&payload)?;
        let server_ms = t_server.elapsed().as_secs_f64() * 1000.0;
        let response_bytes = answer.response.len();

        // Client: decode response
        let t_decode = Instant::now();
        let _decoded = ypir_client.decode_response_simplepir(seed, &answer.response);
        let decode_ms = t_decode.elapsed().as_secs_f64() * 1000.0;

        eprintln!(
            "  {} query {}/{}: up={} dn={} gen={:.0}ms server={:.0}ms decode={:.0}ms",
            name,
            i + 1,
            num_queries,
            format_bytes(query_bytes),
            format_bytes(response_bytes),
            gen_ms,
            server_ms,
            decode_ms,
        );

        total_query_bytes += query_bytes;
        total_response_bytes += response_bytes;
        total_gen_ms += gen_ms;
        total_server_ms += server_ms;
        total_decode_ms += decode_ms;
    }

    let n = num_queries as f64;
    Ok(BenchResults {
        avg_query_bytes: total_query_bytes / num_queries,
        avg_response_bytes: total_response_bytes / num_queries,
        avg_gen_ms: total_gen_ms / n,
        avg_server_ms: total_server_ms / n,
        avg_decode_ms: total_decode_ms / n,
    })
}

fn format_bytes(b: usize) -> String {
    if b >= 1_048_576 {
        format!("{:.2} MB", b as f64 / 1_048_576.0)
    } else if b >= 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{} B", b)
    }
}

fn format_ms(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.1}s", ms / 1000.0)
    } else {
        format!("{:.0}ms", ms)
    }
}

// ── Utilities ────────────────────────────────────────────────────────────────

/// Export all three tier data blobs from a built PIR tree.
fn export_tiers(tree: &pir_export::PirTree) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let tier0_data =
        pir_export::tier0::export(&tree.root25, &tree.levels, &tree.ranges, &tree.empty_hashes);
    eprintln!("  Tier 0: {} bytes", tier0_data.len());

    let mut tier1_data = Vec::new();
    pir_export::tier1::export(
        &tree.levels,
        &tree.ranges,
        &tree.empty_hashes,
        &mut tier1_data,
    )?;
    eprintln!("  Tier 1: {} bytes", tier1_data.len());

    let mut tier2_data = Vec::new();
    pir_export::tier2::export(&tree.ranges, &mut tier2_data)?;
    eprintln!("  Tier 2: {} bytes", tier2_data.len());

    Ok((tier0_data, tier1_data, tier2_data))
}

fn load_nullifiers(path: &std::path::Path) -> Result<Vec<Fp>> {
    let data = std::fs::read(path)?;
    nf_ingest::file_store::parse_nullifier_bytes(&data)
}
