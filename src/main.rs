use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Duration;

use clap::{CommandFactory, Parser, error::ErrorKind};

use heft::cli::{Cli, Glyphs};

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Before sampling, not after: the TUI cannot open a terminal it has not
    // got, and surfacing that at the end of a `/proc` walk would burn a whole
    // `--interval` first. Non-zero, and no quiet fall back to `--once`, which
    // would surprise anyone piping heft expecting a TUI.
    // `--explain`, `--fixture` and `--check-rules` write to stdout like the
    // other two, so they are exempt for the same reason: they are not the TUI.
    if !cli.once
        && !cli.json
        && !cli.fixture
        && !cli.check_rules
        && cli.explain.is_none()
        && !std::io::stdout().is_terminal()
    {
        eprintln!(
            "heft: the TUI needs a terminal on stdout. Use --once for one table, or --json for one JSON document."
        );
        return ExitCode::FAILURE;
    }
    // Before any sampling: it reads rule files and nothing else, so it runs
    // in a container with no `/proc` worth walking and no docker socket.
    if cli.check_rules {
        return exit_code(heft::rules::print_check());
    }
    // Before any read, and once, for the same reason `glyph` is: it cannot
    // change while heft runs. A missing root is a usage error rather than a
    // tree of blanks -- every metric would come back unreadable and look like
    // a permissions problem.
    if let Some(root) = cli.proc_root.as_deref()
        && !std::path::Path::new(root).join("proc").is_dir()
    {
        usage(format!(
            "invalid value {root:?} for '--proc-root <DIR>': no proc directory there"
        ))
    }
    heft::root::init(cli.proc_root.as_deref());
    // Before anything renders, and once: the answer cannot change while heft
    // runs, so no render site has to carry it.
    heft::glyph::init(match cli.glyphs {
        Glyphs::Auto => None,
        Glyphs::Unicode => Some(heft::glyph::Set::Unicode),
        Glyphs::Legacy => Some(heft::glyph::Set::Legacy),
        Glyphs::Ascii => Some(heft::glyph::Set::Ascii),
    });
    // A typed value below the floor is a usage error, the same split as a bad
    // `--sort`: silently raising it left the caller computing rates against a
    // cadence heft was not using. `--pss-interval` is only checked against the
    // floor, not against `--interval`: it is documented as "at least
    // `--interval`", and `--interval 10` alone would otherwise fail against
    // the default of 5.
    let interval = check_interval("interval", cli.interval);
    let pss_interval = check_interval("pss-interval", cli.pss_interval).max(interval);
    let view = resolve_view(&cli);
    let result = if let Some(pid) = cli.explain {
        heft::explain::run(pid, interval)
    } else if cli.fixture {
        heft::proc::print_fixture().map(|()| true)
    } else {
        match (cli.json, cli.once, cli.follow) {
            (true, _, false) => heft::once::print_json(interval, &view),
            (true, _, true) => heft::once::follow_json(interval, pss_interval, &view),
            (_, true, false) => heft::once::print_table(interval, &view),
            (_, true, true) => heft::once::follow_table(interval, pss_interval, &view),
            _ => heft::ui::run(interval, pss_interval, view, cli.trend),
        }
        .map(|()| true)
    };
    exit_code(result)
}

/// `Ok(false)` is a clean run whose answer is "no": a failed rules example or
/// a pid heft cannot see.
fn exit_code(result: Result<bool, heft::types::Error>) -> ExitCode {
    match result {
        Ok(true) => ExitCode::SUCCESS,
        // Rust ignores SIGPIPE, so a closed reader surfaces as EPIPE rather
        // than killing us. `heft --once | head` is the normal case, not a
        // failure, so exit quietly instead of reporting it.
        Err(e) if is_broken_pipe(e.as_ref()) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// The one `View` every surface gets: the saved one, or the default for
/// `--json`, with each flag the user typed laid over it.
fn resolve_view(cli: &Cli) -> heft::config::View {
    // A saved view is a human's TUI preference. `--json` is a documented
    // contract, so only an explicit flag reshapes it.
    let mut view = if cli.json {
        heft::config::View::default()
    } else {
        heft::config::load_view()
    };
    if let Some(label) = &cli.sort {
        view.sort = check_column("sort", label, &heft::once::sort_labels()).to_string();
    }
    if let Some(filter) = &cli.filter {
        check_filter(filter);
        view.filter.clone_from(filter);
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
    // Adds to what the view hides rather than replacing it, or `--hide vram`
    // would bring back the stall columns a default view keeps out.
    for label in &cli.hide {
        let label = check_hide(label).to_string();
        if !view.hide_columns.contains(&label) {
            view.hide_columns.push(label);
        }
    }
    if !cli.order.is_empty() {
        let mut seen = std::collections::HashSet::new();
        view.column_order = cli
            .order
            .iter()
            .map(|label| {
                // Ordering is presentation, so every column can be moved,
                // including `spark`, which no sort can land on: `--order`
                // validates against every column label, not the sortable ones
                // `--sort` takes.
                let label = check_column("order", label, &heft::once::column_labels());
                if !seen.insert(label) {
                    usage(format!(
                        "invalid value '{label}' for '--order <COLUMN>': listed more than once"
                    ))
                }
                label.to_string()
            })
            .collect();
    }
    // Unlike `--filter`, this one applies to `--json` too: it prunes User
    // nodes, which the JSON tree has, where "keep the ancestors" had nothing
    // to mean there.
    view.users = cli.user.iter().map(|who| check_user(who)).collect();
    view
}

/// The typed seconds as a `Duration`, or a usage error. `is_finite` first, so a
/// NaN is refused rather than slipping past a `<` comparison: `--interval nan`
/// parses as a float. A finite value above the floor can still be too large
/// for a `Duration` (`1e30`); that is refused too, not capped, because a typed
/// argument can be corrected.
fn check_interval(flag: &str, secs: f64) -> Duration {
    let why = if !secs.is_finite() || secs < heft::proc::MIN_INTERVAL {
        format!("the floor is {} seconds", heft::proc::MIN_INTERVAL)
    } else {
        match Duration::try_from_secs_f64(secs) {
            Ok(d) => return d,
            Err(e) => e.to_string(),
        }
    };
    usage(format!(
        "invalid value '{secs}' for '--{flag} <{}>': {why}",
        flag.to_uppercase().replace('-', "_"),
    ))
}

/// Every invalid typed value exits the way clap's own does: same prefix,
/// usage line and exit code.
fn usage(msg: impl std::fmt::Display) -> ! {
    Cli::command().error(ErrorKind::InvalidValue, msg).exit()
}

/// A typo on the command line is told to the user, where `Sort::from_label`
/// silently falls back for a saved view: a stale `view.json` must not stop the
/// monitor, but an argument just typed can still be corrected.
fn check_column<'a>(flag: &str, label: &'a str, labels: &[&'static str]) -> &'a str {
    if labels.contains(&label) {
        return label;
    }
    usage(format!(
        "invalid value '{label}' for '--{flag} <COLUMN>'\n  [possible values: {}]",
        labels.join(", ")
    ))
}

/// Same split as `--sort`: a typo on the command line is a usage error, a
/// stale `view.json` entry only warns. `name` is refused rather than listed
/// among the possible values, because hiding it would leave a table of
/// numbers with no labels.
fn check_hide(label: &str) -> &str {
    if label == "name" {
        usage("invalid value 'name' for '--hide <COLUMN>': the name column cannot be hidden")
    }
    check_column("hide", label, &heft::once::hideable_labels())
}

/// A pattern that does not compile is reported the way a bad `--sort` is: a
/// saved view must not stop the monitor, but an argument just typed can still
/// be corrected. The message is the engine's own, which names the offset.
fn check_filter(pattern: &str) {
    if heft::once::Filter::new(pattern).is_some() {
        return;
    }
    usage(format!(
        "invalid value '{pattern}' for '--filter <REGEX>': not a valid regex"
    ))
}

/// A name nothing on this machine answers to is a typo worth reporting, the
/// same as a bad `--sort`. A bare number is never rejected: uids without a
/// passwd entry are ordinary inside containers, and refusing them would make
/// `--user` unusable for exactly the rows it is most wanted on.
fn check_user(who: &str) -> u32 {
    if let Some(uid) = heft::proc::uid_for(who) {
        return uid;
    }
    usage(format!(
        "invalid value '{who}' for '--user <NAME|UID>': no such user"
    ))
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
