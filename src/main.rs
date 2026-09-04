use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "heft",
    about = "Read-only Linux application-weight process monitor"
)]
struct Cli {
    /// Print one table and exit
    #[arg(long)]
    once: bool,
    /// Print one JSON document and exit
    #[arg(long)]
    json: bool,
    /// Seconds between the two samples (also the TUI tick)
    #[arg(long, default_value_t = 1.0)]
    interval: f64,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let interval = Duration::from_secs_f64(cli.interval.max(0.05));
    let result = if cli.json {
        heft::once::print_json(interval)
    } else if cli.once {
        heft::once::print_table(interval)
    } else {
        heft::ui::run(interval)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
