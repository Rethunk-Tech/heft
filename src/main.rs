use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, error::ErrorKind};

mod cli;

use cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Before sampling, not after: the TUI cannot open a terminal it has not
    // got, and surfacing that at the end of a `/proc` walk would burn a whole
    // `--interval` first. Non-zero, and no quiet fall back to `--once`, which
    // would surprise anyone piping heft expecting a TUI.
    if !cli.once && !cli.json && !std::io::stdout().is_terminal() {
        eprintln!(
            "heft: the TUI needs a terminal on stdout. Use --once for one table, or --json for one JSON document."
        );
        return ExitCode::FAILURE;
    }
    let (interval, pss_interval) = heft::proc::clamp_intervals(cli.interval, cli.pss_interval);
    // A saved view is a human's TUI preference. `--json` is a documented
    // contract, so only an explicit flag reshapes it.
    let mut view = if cli.json {
        heft::config::View::default()
    } else {
        heft::config::load_view()
    };
    if let Some(label) = cli.sort {
        view.sort = check_sort(&label).to_string();
    }
    if let Some(filter) = cli.filter {
        view.filter = filter;
    }
    let result = if cli.json {
        heft::once::print_json(interval, &view)
    } else if cli.once {
        heft::once::print_table(interval, &view)
    } else {
        heft::ui::run(interval, pss_interval, view)
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

/// A typo on the command line is told to the user, where `Sort::from_label`
/// silently falls back for a saved view: a stale `view.json` must not stop the
/// monitor, but an argument just typed can still be corrected.
fn check_sort(label: &str) -> &str {
    let labels = heft::once::sort_labels();
    if labels.contains(&label) {
        return label;
    }
    Cli::command()
        .error(
            ErrorKind::InvalidValue,
            format!(
                "invalid value '{label}' for '--sort <COLUMN>'\n  [possible values: {}]",
                labels.join(", ")
            ),
        )
        .exit()
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
