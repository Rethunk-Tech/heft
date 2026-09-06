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
        // Rust ignores SIGPIPE, so a closed reader surfaces as EPIPE rather
        // than killing us. `heft --once | head` is the normal case, not a
        // failure, so exit quietly instead of reporting it.
        Err(e) if is_broken_pipe(e.as_ref()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn is_broken_pipe(e: &(dyn std::error::Error + 'static)) -> bool {
    std::iter::successors(Some(e), |e| e.source())
        .filter_map(|e| e.downcast_ref::<std::io::Error>())
        .any(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
}

#[cfg(test)]
mod tests {
    use super::is_broken_pipe;
    use std::io::{Error, ErrorKind};

    #[test]
    fn only_a_closed_pipe_exits_quietly() {
        let pipe: heft::types::Error = Error::from(ErrorKind::BrokenPipe).into();
        assert!(is_broken_pipe(pipe.as_ref()));

        // A full disk is a real failure and must still be reported: writing
        // to /dev/full is the case that proves this branch is not swallowing
        // every io error.
        let full: heft::types::Error = Error::from(ErrorKind::StorageFull).into();
        assert!(!is_broken_pipe(full.as_ref()));

        let other: heft::types::Error = "docker HTTP error".into();
        assert!(!is_broken_pipe(other.as_ref()));
    }
}
