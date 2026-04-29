//! `nf-server precompute-cache` — generate YPIR precompute cache files.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args as ClapArgs;

#[derive(ClapArgs)]
pub struct Args {
    /// Directory containing tier1.bin and tier2.bin. Writes
    /// tier1.precompute and tier2.precompute beside them.
    #[arg(long, default_value = "./pir-data", env = "SVOTE_PIR_DATA_DIR")]
    pir_data_dir: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    pir_server::generate_precompute_caches(&args.pir_data_dir)
}
