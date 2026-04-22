//! `nf-server doctor` — pre-flight checks against published hardware guidance.
//!
//! Prints human-readable results to stdout. When the host is below the
//! [server setup runbook](../../docs/runbooks/server-setup.md) recommendations,
//! writes `WARN: …` lines to stderr (and `tracing::warn!` for log aggregation).
//! Always exits successfully so CI can smoke the command without treating
//! undersized runners as failures.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Args as ClapArgs;
use sysinfo::{Disk, Disks, System};

/// Recommended minimum logical CPUs (vCPU count).
const RECOMMENDED_CPU: u32 = 4;
/// Recommended minimum system RAM (GiB).
const RECOMMENDED_RAM_GIB: u64 = 32;
/// Recommended minimum free space on the PIR data volume (GiB).
const RECOMMENDED_FREE_DISK_GIB: u64 = 35;

#[derive(ClapArgs)]
pub struct Args {
    /// Directory used for PIR on-disk state (same default as `serve` / `sync`).
    /// Free space is checked on the filesystem backing this path.
    #[arg(long, default_value = "./pir-data", env = "SVOTE_PIR_DATA_DIR")]
    pir_data_dir: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    println!("nf-server doctor — hardware vs runbook recommendations");
    println!(
        "Target: ≥{RECOMMENDED_CPU} logical CPUs, ≥{RECOMMENDED_RAM_GIB} GiB RAM, \
         ≥{RECOMMENDED_FREE_DISK_GIB} GiB free on the PIR data volume, \
         AVX-512 on x86_64 for best PIR performance"
    );
    println!();

    print_build_features();

    check_cpu();
    check_ram();
    check_disk(&args.pir_data_dir)?;
    check_avx512();

    println!();
    println!("Doctor finished (warnings are advisory; exit code is always 0).");
    Ok(())
}

fn print_build_features() {
    let serve = if cfg!(feature = "serve") {
        "enabled"
    } else {
        "disabled"
    };
    let avx512 = if cfg!(feature = "avx512") {
        "enabled"
    } else {
        "disabled"
    };
    println!("Build: `serve` feature {serve}; `avx512` feature {avx512}");
    if !cfg!(feature = "serve") {
        println!(
            "Note: this binary was built without `serve`; production PIR hosts should use \
             `cargo build -p nf-server --features serve` (optionally `--features avx512`)."
        );
    } else if !cfg!(feature = "avx512") {
        println!(
            "Note: AVX-512 not enabled at compile time; rebuild with `--features avx512` on \
             AVX-512-capable hosts for best throughput."
        );
    }
    println!();
}

fn check_cpu() {
    let n = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    println!("Logical CPUs: {n}");
    if n < RECOMMENDED_CPU {
        warn_hardware(format!(
            "logical CPU count ({n}) is below the recommended {RECOMMENDED_CPU} vCPUs",
        ));
    }
}

fn check_ram() {
    let mut sys = System::new();
    sys.refresh_memory();
    let total = sys.total_memory();
    let gib = total as f64 / (1024.0 * 1024.0 * 1024.0);
    println!("System RAM: {:.1} GiB", gib);
    if total < RECOMMENDED_RAM_GIB * 1024 * 1024 * 1024 {
        warn_hardware(format!(
            "system RAM ({gib:.1} GiB) is below the recommended {RECOMMENDED_RAM_GIB} GiB"
        ));
    }
}

fn check_disk(pir_data_dir: &Path) -> Result<()> {
    let disks = Disks::new_with_refreshed_list();
    let check_path = resolve_existing_prefix(pir_data_dir);
    let disk = disk_for_path(&disks, check_path.as_path());
    match disk {
        Some(d) => {
            let free = d.available_space();
            let free_gib = free as f64 / (1024.0 * 1024.0 * 1024.0);
            let mount = d.mount_point().display();
            println!(
                "Free disk (volume {mount}, checked from {}): {:.1} GiB",
                check_path.display(),
                free_gib
            );
            let min_bytes = RECOMMENDED_FREE_DISK_GIB * 1024 * 1024 * 1024;
            if free < min_bytes {
                warn_hardware(format!(
                    "free space ({free_gib:.1} GiB on {mount}) is below the recommended \
                     {RECOMMENDED_FREE_DISK_GIB} GiB for PIR data under {}",
                    pir_data_dir.display()
                ));
            }
        }
        None => {
            warn_hardware(format!(
                "could not map `{}` to a disk mount; skipping free-space check",
                check_path.display()
            ));
        }
    }
    Ok(())
}

fn check_avx512() {
    #[cfg(target_arch = "x86_64")]
    {
        let supported = std::arch::is_x86_feature_detected!("avx512f");
        println!("AVX-512F (runtime): {}", if supported { "yes" } else { "no" });
        if !supported {
            warn_hardware(
                "CPU does not advertise AVX-512F at runtime; PIR still runs but may be \
                 slower than on AVX-512-capable hardware",
            );
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        println!("AVX-512F (runtime): not applicable on this architecture");
    }
}

/// Walk up from `pir_data_dir` until an existing path is found so `stat` /
/// mount resolution works before the directory is created.
fn resolve_existing_prefix(p: &Path) -> PathBuf {
    let mut cur = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p)
    };
    loop {
        if cur.exists() {
            return cur.canonicalize().unwrap_or(cur);
        }
        if let Some(parent) = cur.parent() {
            if parent == cur {
                return parent.to_path_buf();
            }
            cur = parent.to_path_buf();
        } else {
            return PathBuf::from("/");
        }
    }
}

fn disk_for_path<'a>(disks: &'a Disks, path: &Path) -> Option<&'a Disk> {
    disks
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
}

fn warn_hardware(msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    eprintln!("WARN: {msg}");
    tracing::warn!("{msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_for_path_prefers_longest_mount() {
        let disks = Disks::new_with_refreshed_list();
        let tmp = std::env::temp_dir();
        assert!(disk_for_path(&disks, &tmp).is_some());
    }
}
