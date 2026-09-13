//! `--explain <PID>`: where a process landed in the tree, the identity a
//! placement rule keys on, and what each rules stage made of the process.
//!
//! A placement rule is keyed on the identities the tree shows, and a wrong key
//! is silent: an identity that matches nothing is simply never consulted. The
//! TUI's `i` pane shows a process's cgroup, exe and cmdline but never what any
//! of that resolved to, so this is where the key is read off.
//!
//! The per-stage lines are a second, traced evaluation of the same rules, run
//! only here. The tree's verdict also comes from ancestor walks no single
//! stage sees, and a reason string threaded through grouping would be paid
//! for on every process of every tick to serve one invocation.

use std::time::Duration;

use crate::glyph;
use crate::once::printable;
use crate::proc;
use crate::rules::{Facts, Rules, Stage, folder_name};
use crate::types::{Error, Folder, HostTree, IdentNode, ProcNode, Process};

/// Where in the tree a pid turned up.
struct Found {
    /// Host → user → folder, as the tree draws it.
    path: String,
    /// The identity row's title: the key a placement rule matches.
    ident: String,
    /// The folder a placement rule would pin, or `None` where no rule can
    /// reach the row.
    pinnable: Option<&'static str>,
    instance: String,
    siblings: usize,
}

/// Processes nest: a worker folds under the parent it bills to, so the pid
/// being asked about is often a child rather than a top-level entry.
fn holds(procs: &[ProcNode], pid: u32) -> bool {
    procs
        .iter()
        .any(|p| p.pid == pid || holds(&p.children, pid))
}

fn find_in(idents: &[IdentNode], pid: u32) -> Option<(&IdentNode, String, usize)> {
    for id in idents {
        for inst in &id.instances {
            if holds(&inst.processes, pid) {
                return Some((id, inst.key.clone(), inst.processes.len()));
            }
        }
        for c in &id.containers {
            if holds(&c.processes, pid) {
                return Some((id, c.title.clone(), c.processes.len()));
            }
        }
    }
    None
}

fn locate(tree: &HostTree, pid: u32) -> Option<Found> {
    let arrow = glyph::arrows().3;
    let hit = |idents: &[IdentNode], parent: &str, folder: Folder| {
        find_in(idents, pid).map(|(id, instance, siblings)| Found {
            path: format!("{parent} {arrow} {}", folder.title()),
            ident: id.title.clone(),
            // A container or System row ignores every placement rule, the same
            // rule the grouping code applies: `override_place` cannot move one.
            pinnable: matches!(folder, Folder::Applications | Folder::UserServices)
                .then_some(folder_name(folder)),
            instance,
            siblings,
        })
    };
    for u in &tree.users {
        let who = format!("Host {arrow} {} ({})", u.name, u.uid);
        if let Some(f) = hit(&u.applications, &who, Folder::Applications)
            .or_else(|| hit(&u.user_services, &who, Folder::UserServices))
            .or_else(|| hit(&u.containers, &who, Folder::Containers))
        {
            return Some(f);
        }
    }
    hit(&tree.containers, "Host", Folder::Containers)
        .or_else(|| hit(&tree.system, "Host", Folder::System))
}

/// One line per stage naming the rule that decided it, `no match`, or `none`
/// for a stage with no rules. It says what each stage makes of this process,
/// not why the tree placed it: a session rule can match a process a container
/// scope took first, and `placed` is the tree's verdict.
fn trace(rules: &Rules, p: Option<&Process>, ident: &str) -> Vec<String> {
    let Some(p) = p else {
        return vec!["  rules     (process gone)".to_string()];
    };
    let unit = crate::identity::user_unit(&p.cgroup);
    let process = crate::group::facts_of(p, unit.as_deref());
    let stages = [
        (
            Stage::Unit,
            "unit",
            Facts {
                unit: unit.as_deref(),
                ..Facts::default()
            },
        ),
        (Stage::Class, "class", process),
        (Stage::Session, "session", process),
        (Stage::App, "app", process),
        (
            Stage::Placement,
            "placement",
            Facts {
                identity: Some(ident),
                ..Facts::default()
            },
        ),
    ];
    let mut out = Vec::new();
    for (stage, label, f) in &stages {
        let hits = rules.deciding(*stage, f);
        let verdicts: Vec<String> = if rules.is_empty(*stage) {
            vec!["none".into()]
        } else if hits.is_empty() {
            vec!["no match".into()]
        } else {
            hits.iter()
                .map(|r| {
                    let origin = if r.source.rank == usize::MAX {
                        format!("built-in {}", r.name())
                    } else {
                        format!("{}/{}", r.source.label, r.name())
                    };
                    format!("{origin} -> {}", r.outputs())
                })
                .collect()
        };
        for (j, v) in verdicts.iter().enumerate() {
            let lead = if out.is_empty() { "rules" } else { "" };
            let label = if j == 0 { *label } else { "" };
            out.push(format!("  {lead:<9} {label:<10} {v}"));
        }
    }
    out
}

/// `Ok(false)` when the pid is not visible, so a script can tell a miss from a
/// hit by the exit code.
///
/// # Errors
///
/// Returns an error if the sample cannot be taken.
pub fn run(pid: u32, interval: Duration) -> Result<bool, Error> {
    let tree = proc::sample_world(interval);
    let found = locate(&tree, pid);
    // `proc::detail` answers with the fields it could read, so a pid that is
    // not there at all comes back as a column of blanks rather than nothing.
    // The directory is the question being asked.
    let visible = std::path::Path::new(&format!("{}/proc/{pid}", crate::root::prefix())).is_dir();
    let facts = if visible {
        proc::detail(pid)
    } else {
        Vec::new()
    };

    if !visible && found.is_none() {
        // Not an error message: the pid may simply have gone, and that is the
        // answer. Still a miss, so the exit code says so.
        println!("pid {pid}: not visible");
        println!();
        println!("  It exited, or /proc hides it -- another user's process, a");
        println!("  hidepid mount, or a PID namespace. heft can only group what");
        println!("  it can walk.");
        return Ok(false);
    }

    println!("pid {pid}");
    for (k, v) in &facts {
        println!("  {k:<9} {}", printable(v));
    }

    let Some(f) = found else {
        println!();
        println!("  placed    nowhere: heft read this process but no row holds it.");
        println!("            A kernel thread with no cgroup, or it exited between");
        println!("            the two walks a sample takes.");
        return Ok(true);
    };

    // The tree keeps raw strings; process-chosen text is escaped only here,
    // where it reaches a terminal.
    let ident = printable(&f.ident);
    println!();
    println!("  placed    {}", printable(&f.path));
    println!("  identity  {ident}");
    println!(
        "  instance  {} ({} process{})",
        printable(&f.instance),
        f.siblings,
        if f.siblings == 1 { "" } else { "es" }
    );

    // Re-read rather than carried on the tree: the tree has no per-process
    // facts, and a pid that exited since the sample says so here.
    let p = proc::read_pid(pid, false, false, None, &mut Vec::new());
    for line in trace(Rules::load(), p.as_ref(), &f.ident) {
        println!("{}", printable(&line));
    }

    println!();
    if let Some(list) = f.pinnable {
        println!("  \"{ident}\" is the placement key for this row.");
        println!();
        println!(
            "  To pin it to a folder, in {}/rules.d/90-mine.json:",
            printable(&crate::config::config_dir().display().to_string())
        );
        // JSON-quoted first, so a quote or backslash in the identity still
        // pastes as a valid rule; `printable` then covers the C1 controls
        // JSON leaves raw.
        let key = serde_json::Value::from(f.ident.as_str()).to_string();
        println!(
            "    {{ \"stage\": \"placement\", \"rules\": [ {{ \"id\": \"pin\", \"match\": {{ \"identity\": {} }}, \"folder\": \"{list}\" }} ] }}",
            printable(&key)
        );
        println!();
        println!("  To bill it to another row instead, `\"fold_to\": \"<other identity>\"`");
        println!("  in place of `folder`.");
    } else {
        println!("  No placement rule can move this row. A container is placed by its");
        println!("  runtime labels and a kernel thread by the cgroup it is in, so");
        println!("  `fold_to` and `folder` skip them. A rule with `owner_uid` sets a");
        println!("  container's owning uid by name.");
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{IdentNode, InstanceNode, UserNode};

    fn proc_node(pid: u32) -> ProcNode {
        ProcNode {
            pid,
            name: format!("p{pid}"),
            ..ProcNode::default()
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "a test builder is handed a handful of pids, never 4 billion"
    )]
    fn ident(title: &str, pids: &[u32]) -> IdentNode {
        IdentNode {
            id: title.into(),
            title: title.into(),
            nproc: pids.len() as u32,
            instances: vec![InstanceNode {
                key: format!("{title}/1"),
                nproc: pids.len() as u32,
                processes: pids.iter().copied().map(proc_node).collect(),
                ..InstanceNode::default()
            }],
            ..IdentNode::default()
        }
    }

    fn tree_with(user: UserNode, system: Vec<IdentNode>) -> HostTree {
        let mut t = proc::placeholder_tree();
        t.users = vec![user];
        t.system = system;
        t
    }

    fn user(apps: Vec<IdentNode>, services: Vec<IdentNode>) -> UserNode {
        UserNode {
            uid: 1000,
            name: "nomad".into(),
            applications: apps,
            user_services: services,
            ..UserNode::default()
        }
    }

    #[test]
    fn trace_names_the_deciding_rule_per_stage() {
        let kicker = Process {
            comm: "kicker".into(),
            exe: Some("/opt/trinity/bin/tdeinit".into()),
            ..Process::default()
        };
        let lines = trace(&Rules::builtin(), Some(&kicker), "tdeinit");
        let line = |stage: &str| {
            lines
                .iter()
                .find(|l| l.split_whitespace().any(|w| w == stage))
                .cloned()
                .unwrap_or_default()
        };
        assert!(
            line("session")
                .ends_with("built-in 40-trinity.json:trinity-session -> tdeinit user_services"),
            "{lines:#?}"
        );
        assert!(line("unit").ends_with("no match"), "{lines:#?}");
        assert!(line("placement").ends_with("none"), "{lines:#?}");
        assert!(lines[0].starts_with("  rules     unit"), "{lines:#?}");
        assert_eq!(
            trace(&Rules::builtin(), None, "x"),
            ["  rules     (process gone)"]
        );
    }

    #[test]
    fn a_pid_resolves_to_the_identity_that_is_the_placement_key() {
        let tree = tree_with(user(vec![ident("cursor", &[10, 11])], Vec::new()), vec![]);
        let f = locate(&tree, 11).expect("found");
        assert_eq!(
            f.ident, "cursor",
            "the row title, which is the placement key"
        );
        assert_eq!(f.path, "Host → nomad (1000) → Applications");
        assert_eq!(f.pinnable, Some("applications"));
        assert_eq!(f.siblings, 2);
    }

    #[test]
    fn a_user_service_offers_the_other_list() {
        let tree = tree_with(user(Vec::new(), vec![ident("syncthing", &[7])]), vec![]);
        let f = locate(&tree, 7).expect("found");
        assert_eq!(f.pinnable, Some("user_services"));
    }

    /// A placement rule naming a System or Containers row is ignored by the
    /// grouping code, so `--explain` must not offer a key that does nothing.
    #[test]
    fn a_system_row_is_reported_as_unmovable() {
        let tree = tree_with(user(Vec::new(), Vec::new()), vec![ident("kworker", &[3])]);
        let f = locate(&tree, 3).expect("found");
        assert_eq!(f.path, "Host → System");
        assert_eq!(f.pinnable, None);
    }

    /// A worker folds under the process it bills to, so the pid asked about is
    /// usually a child rather than a top-level entry of the instance.
    #[test]
    fn a_folded_child_resolves_to_the_row_it_bills_to() {
        let mut parent = proc_node(10);
        let mut worker = proc_node(11);
        worker.children = vec![proc_node(12)];
        parent.children = vec![worker];
        let mut id = ident("cursor", &[]);
        id.instances[0].processes = vec![parent];
        let tree = tree_with(user(vec![id], Vec::new()), vec![]);
        for pid in [10, 11, 12] {
            assert_eq!(locate(&tree, pid).expect("found").ident, "cursor", "{pid}");
        }
    }

    #[test]
    fn a_pid_in_no_row_is_not_an_error() {
        let tree = tree_with(user(vec![ident("cursor", &[10])], Vec::new()), vec![]);
        assert!(locate(&tree, 99).is_none());
    }
}
