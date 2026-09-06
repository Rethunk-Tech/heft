use std::process::ExitCode;

use clap::Parser;

mod cli;

use cli::Cli;

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
