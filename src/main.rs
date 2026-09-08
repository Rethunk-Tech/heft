use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, error::ErrorKind};

mod cli;

use cli::{Cli, Glyphs};

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Ahead of the terminal check: a flag combination that cannot mean
    // anything is a usage error wherever stdout happens to point, and telling
    // someone about their terminal when the problem is their command line
    // sends them looking in the wrong place. The TUI is already a follow.
    if cli.follow && !cli.once && !cli.json {
        Cli::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "--follow needs --once or --json; the TUI already samples continuously",
            )
            .exit()
    }
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
    // Before anything renders, and once: the answer cannot change while heft
    // runs, so no render site has to carry it.
    heft::glyph::init(match cli.glyphs {
        Glyphs::Auto => None,
        Glyphs::Unicode => Some(heft::glyph::Set::Unicode),
        Glyphs::Ascii => Some(heft::glyph::Set::Ascii),
    });
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
        check_filter(&filter);
        view.filter = filter;
    }
    view.top = cli.top.map(|n| n as usize);
    // `--sort age` alone meant whichever direction the saved view happened to
    // hold, so the same command printed differently on two machines. Only an
    // explicit flag moves it; absent both, the view still decides.
    if cli.asc {
        view.desc = false;
    } else if cli.desc {
        view.desc = true;
    }
    if !cli.hide.is_empty() {
        view.hide_columns = cli
            .hide
            .iter()
            .map(|label| check_hide(label).to_string())
            .collect();
    }
    if !cli.order.is_empty() {
        let mut seen = std::collections::HashSet::new();
        view.column_order = cli
            .order
            .iter()
            .map(|label| {
                let label = check_order(label);
                if !seen.insert(label) {
                    Cli::command()
                        .error(
                            ErrorKind::InvalidValue,
                            format!("invalid value '{label}' for '--order <COLUMN>': listed more than once"),
                        )
                        .exit()
                }
                label.to_string()
            })
            .collect();
    }
    // Unlike `--filter`, this one applies to `--json` too: it prunes User
    // nodes, which the JSON tree has, where "keep the ancestors" had nothing
    // to mean there.
    view.users = cli.user.iter().map(|who| check_user(who)).collect();
    let result = match (cli.json, cli.once, cli.follow) {
        (true, _, false) => heft::once::print_json(interval, &view),
        (true, _, true) => heft::once::follow_json(interval, pss_interval, &view),
        (_, true, false) => heft::once::print_table(interval, &view),
        (_, true, true) => heft::once::follow_table(interval, pss_interval, &view),
        _ => heft::ui::run(interval, pss_interval, view),
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
fn check_column<'a>(flag: &str, label: &'a str) -> &'a str {
    let labels = heft::once::sort_labels();
    if labels.contains(&label) {
        return label;
    }
    Cli::command()
        .error(
            ErrorKind::InvalidValue,
            format!(
                "invalid value '{label}' for '--{flag} <COLUMN>'\n  [possible values: {}]",
                labels.join(", ")
            ),
        )
        .exit()
}

fn check_sort(label: &str) -> &str {
    check_column("sort", label)
}

fn check_order(label: &str) -> &str {
    check_column("order", label)
}

/// Same split as `--sort`: a typo on the command line is a usage error, a
/// stale `view.json` entry only warns. `name` is refused rather than listed
/// among the possible values, because hiding it would leave a table of
/// numbers with no labels.
fn check_hide(label: &str) -> &str {
    if label == "name" {
        Cli::command()
            .error(
                ErrorKind::InvalidValue,
                "invalid value 'name' for '--hide <COLUMN>': the name column cannot be hidden",
            )
            .exit()
    }
    let labels = heft::once::hideable_labels();
    if labels.contains(&label) {
        return label;
    }
    Cli::command()
        .error(
            ErrorKind::InvalidValue,
            format!(
                "invalid value '{label}' for '--hide <COLUMN>'\n  [possible values: {}]",
                labels.join(", ")
            ),
        )
        .exit()
}

/// A pattern that does not compile is reported the way a bad `--sort` is: a
/// saved view must not stop the monitor, but an argument just typed can still
/// be corrected. The message is the engine's own, which names the offset.
fn check_filter(pattern: &str) {
    if heft::once::Filter::new(pattern).is_some() {
        return;
    }
    Cli::command()
        .error(
            ErrorKind::InvalidValue,
            format!("invalid value '{pattern}' for '--filter <REGEX>': not a valid regex"),
        )
        .exit()
}

/// A name nothing on this machine answers to is a typo worth reporting, the
/// same as a bad `--sort`. A bare number is never rejected: uids without a
/// passwd entry are ordinary inside containers, and refusing them would make
/// `--user` unusable for exactly the rows it is most wanted on.
fn check_user(who: &str) -> u32 {
    if let Some(uid) = heft::proc::uid_for(who) {
        return uid;
    }
    Cli::command()
        .error(
            ErrorKind::InvalidValue,
            format!("invalid value '{who}' for '--user <NAME|UID>': no such user"),
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
