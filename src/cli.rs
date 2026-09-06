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
    /// Sort by this column label: the labels `c` cycles and `view.json` saves; a bad one lists them all
    #[arg(long, value_name = "COLUMN")]
    pub sort: Option<String>,
    /// Keep only rows whose name matches this regex, and their parents
    #[arg(long, value_name = "REGEX", conflicts_with = "json")]
    pub filter: Option<String>,
    /// Show only this user's branch; repeat for several. Takes a name or a uid
    #[arg(long = "user", value_name = "NAME|UID")]
    pub user: Vec<String>,
    /// Sort high to low, whatever the saved view holds
    #[arg(long, conflicts_with = "asc")]
    pub desc: bool,
    /// Sort low to high, whatever the saved view holds
    #[arg(long)]
    pub asc: bool,
}
