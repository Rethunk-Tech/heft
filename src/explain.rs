//! `--explain <PID>`: where a process landed in the tree, and the key that
//! moves it.
//!
//! `grouping.json` is keyed on "the identities the tree shows you", and until
//! this existed there was no way to read one off a running heft. The rules
//! behind an identity are the largest thing in the codebase -- launchers,
//! workers, `lying_unit`, `generic_fallback`, `session_helper_ident`, folder
//! placement -- and the TUI's `i` pane shows a process's cgroup, exe and
//! cmdline but never what any of that resolved to. So writing an override
//! meant guessing the key, and a wrong guess is silent: an identity that
//! matches nothing is simply never consulted.
//!
//! It reports the resolved placement rather than narrating the rules that got
//! there. The verdict is what a user needs to write the file, the rules are
//! what `AGENTS.md` is for, and a reason string threaded through grouping
//! would be paid for on every process of every tick to serve one invocation.

use std::time::Duration;

use crate::proc;
use crate::types::{Error, HostTree, IdentNode, ProcNode};

/// Where in the tree a pid turned up.
struct Found {
    /// Host → user → folder, as the tree draws it.
    path: String,
    /// The identity row's title: the grouping.json key.
    ident: String,
    /// The `applications` / `user_services` list an override would use, or
    /// `None` where no override can reach the row.
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
    let hit = |idents: &[IdentNode], path: String, pinnable| {
        find_in(idents, pid).map(|(id, instance, siblings)| Found {
            path,
            ident: id.title.clone(),
            pinnable,
            instance,
            siblings,
        })
    };
    for u in &tree.users {
        let who = format!("{} ({})", u.name, u.uid);
        if let Some(f) = hit(
            &u.applications,
            format!("Host → {who} → Applications"),
            Some("applications"),
        )
        .or_else(|| {
            hit(
                &u.user_services,
                format!("Host → {who} → User Services"),
                Some("user_services"),
            )
        })
        // A container row ignores every override, the same rule the grouping
        // code applies: `override_place` cannot move one.
        .or_else(|| hit(&u.containers, format!("Host → {who} → Containers"), None))
        {
            return Some(f);
        }
    }
    hit(&tree.containers, "Host → Containers".into(), None)
        .or_else(|| hit(&tree.system, "Host → System".into(), None))
}

/// # Errors
///
/// Returns an error if the sample cannot be taken.
pub fn run(pid: u32, interval: Duration) -> Result<(), Error> {
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
        // Not an error: the pid may simply have gone, and that is the answer.
        println!("pid {pid}: not visible");
        println!();
        println!("  It exited, or /proc hides it -- another user's process, a");
        println!("  hidepid mount, or a PID namespace. heft can only group what");
        println!("  it can walk.");
        return Ok(());
    }

    println!("pid {pid}");
    for (k, v) in &facts {
        println!("  {k:<9} {v}");
    }

    let Some(f) = found else {
        println!();
        println!("  placed    nowhere: heft read this process but no row holds it.");
        println!("            A kernel thread with no cgroup, or it exited between");
        println!("            the two walks a sample takes.");
        return Ok(());
    };

    println!();
    println!("  placed    {}", f.path);
    println!("  identity  {}", f.ident);
    println!(
        "  instance  {} ({} process{})",
        f.instance,
        f.siblings,
        if f.siblings == 1 { "" } else { "es" }
    );

    let ov = crate::config::load_overrides();
    let active = ov.folder_for(&f.ident).is_some() || ov.fold_key(&f.ident).is_some();
    println!(
        "  override  {}",
        if active {
            "yes -- grouping.json already names this identity"
        } else {
            "none"
        }
    );

    println!();
    match f.pinnable {
        Some(list) => {
            println!("  \"{}\" is the grouping.json key for this row.", f.ident);
            println!();
            println!(
                "  To pin it to a folder, in {}:",
                crate::config::overrides_path().display()
            );
            println!("    {{ \"{list}\": [\"{}\"] }}", f.ident);
            println!();
            println!("  To bill it to another row instead:");
            println!(
                "    {{ \"fold\": {{ \"{}\": \"<other identity>\" }} }}",
                f.ident
            );
        }
        None => {
            println!("  No override can move this row. A container is placed by its");
            println!("  runtime labels and a kernel thread by the cgroup it is in, so");
            println!("  `applications`, `user_services` and `fold` all skip them.");
            println!("  `container_owners` sets a container's owning uid by name.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{IdentNode, InstanceNode, Metrics, UserNode};

    fn proc_node(pid: u32) -> ProcNode {
        ProcNode {
            pid,
            name: format!("p{pid}"),
            cmdline: String::new(),
            metrics: Metrics::default(),
            children: Vec::new(),
        }
    }

    fn ident(title: &str, pids: &[u32]) -> IdentNode {
        IdentNode {
            id: title.into(),
            title: title.into(),
            nproc: pids.len() as u32,
            metrics: Metrics::default(),
            instances: vec![InstanceNode {
                key: format!("{title}/1"),
                nproc: pids.len() as u32,
                metrics: Metrics::default(),
                processes: pids.iter().copied().map(proc_node).collect(),
            }],
            containers: Vec::new(),
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
            containers: Vec::new(),
        }
    }

    #[test]
    fn a_pid_resolves_to_the_identity_that_is_the_override_key() {
        let tree = tree_with(user(vec![ident("cursor", &[10, 11])], Vec::new()), vec![]);
        let f = locate(&tree, 11).expect("found");
        assert_eq!(f.ident, "cursor", "the row title, which is the json key");
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

    /// An override naming a System or Containers row is ignored by the
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
