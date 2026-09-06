use clap::Parser;

#[derive(Parser)]
#[command(
    name = "heft",
    version,
    about = "Read-only Linux application-weight process monitor"
)]
pub struct Cli {
    /// Print one table and exit
    #[arg(long)]
    pub once: bool,
    /// Print one JSON document and exit
    #[arg(long)]
    pub json: bool,
    /// Seconds between catch-all samples (TUI tick and `--once` / `--json` gap)
    #[arg(long, default_value_t = 1.0)]
    pub interval: f64,
    /// TUI seconds between PSS (`smaps_rollup`) reads (≥ `--interval`). `--once` / `--json` always read PSS
    #[arg(long, default_value_t = 5.0)]
    pub pss_interval: f64,
}
