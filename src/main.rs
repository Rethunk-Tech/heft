use std::process::ExitCode;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "heft",
    version,
    about = "Read-only Linux application-weight process monitor"
)]
struct Cli {
    /// Print one table and exit
    #[arg(long)]
    once: bool,
    /// Print one JSON document and exit
    #[arg(long)]
    json: bool,
    /// Seconds between catch-all samples (TUI tick and `--once` / `--json` gap)
    #[arg(long, default_value_t = 1.0)]
    interval: f64,
    /// TUI seconds between PSS (`smaps_rollup`) reads (≥ `--interval`). `--once` / `--json` always read PSS
    #[arg(long, default_value_t = 5.0)]
    pss_interval: f64,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (interval, pss_interval) = heft::proc::clamp_intervals(cli.interval, cli.pss_interval);
    let result = if cli.json {
        heft::once::print_json(interval)
    } else if cli.once {
        heft::once::print_table(interval)
    } else {
        heft::ui::run(interval, pss_interval)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
