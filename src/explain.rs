//! `--explain <PID>`: where a process landed in the tree, the identity a
//! placement rule keys on, and what each rules stage made of the process.
//!
//! A placement rule is keyed on the identities the tree shows, and a wrong key
//! is silent: an identity that matches nothing is simply never consulted. The
//! TUI's `i` pane prints the same compact body (`placement_lines`) for a
//! process row, without the pin-to-folder recipe.
//!
//! The per-stage lines are a second, traced evaluation of the same rules, run
//! only here. The tree's verdict also comes from ancestor walks no single
//! stage sees, and a reason string threaded through grouping would be paid
//! for on every process of every tick to serve one invocation.

use std::io::Write;

use serde::Serialize;

use crate::glyph;
use crate::once::printable;
use crate::proc;
use crate::rules::{Facts, Rules, Stage, folder_name};
use crate::types::{Error, Folder, HostTree, IdentNode, ProcNode, Process};

/// `--explain --json`: schema first, then the pid's placement. Not a `HostTree`.
#[derive(Serialize)]
struct ExplainDoc<'a> {
    schema: u8,
    pid: u32,
    visible: bool,
    place: Option<&'a str>,
    identity: Option<&'a str>,
    rules: Vec<MatchedRule>,
}

#[derive(Serialize)]
struct MatchedRule {
    stage: &'static str,
    rule: String,
    outputs: String,
}

/// Where in the tree a pid turned up.
pub(crate) struct Found {
    /// Host → user → folder, as the tree draws it.
    path: String,
    /// The identity row's id: the key a placement rule matches.
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
                return Some((id, c.id.clone(), c.processes.len()));
            }
        }
    }
    None
}

fn found_at(
    parent: &str,
    folder: Folder,
    node: &IdentNode,
    instance: String,
    siblings: usize,
) -> Found {
    let arrow = glyph::arrows().3;
    Found {
        path: format!("{parent} {arrow} {}", folder.title()),
        ident: node.id.clone(),
        // A container or System row ignores every placement rule, the same
        // rule the grouping code applies: `override_place` cannot move one.
        pinnable: matches!(folder, Folder::Applications | Folder::UserServices)
            .then_some(folder_name(folder)),
        instance,
        siblings,
    }
}

pub(crate) fn locate(tree: &HostTree, pid: u32) -> Option<Found> {
    let arrow = glyph::arrows().3;
    let hit = |idents: &[IdentNode], parent: &str, folder: Folder| {
        find_in(idents, pid)
            .map(|(node, instance, siblings)| found_at(parent, folder, node, instance, siblings))
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

/// The flatten id of an identity row, built the same way `ui::push_folder`
/// names it, so the TUI can ask for that row without a pid.
fn locate_ident(tree: &HostTree, row_id: &str) -> Option<Found> {
    let arrow = glyph::arrows().3;
    for u in &tree.users {
        let who = format!("Host {arrow} {} ({})", u.name, u.uid);
        for (slug, folder, idents) in [
            ("apps", Folder::Applications, u.applications.as_slice()),
            ("services", Folder::UserServices, u.user_services.as_slice()),
            ("containers", Folder::Containers, u.containers.as_slice()),
        ] {
            let folder_id = format!("user:{}/{slug}", u.uid);
            for node in idents {
                if row_id == format!("{folder_id}/{}", node.id) {
                    return Some(found_at(&who, folder, node, String::new(), 0));
                }
            }
        }
    }
    for (prefix, folder, idents) in [
        (
            "host/containers",
            Folder::Containers,
            tree.containers.as_slice(),
        ),
        ("host/system", Folder::System, tree.system.as_slice()),
    ] {
        for node in idents {
            if row_id == format!("{prefix}/{}", node.id) {
                return Some(found_at("Host", folder, node, String::new(), 0));
            }
        }
    }
    None
}

/// Two or three lines for an identity row: path, the identity, and that it is
/// the placement key when a rule can reach the folder.
pub(crate) fn identity_placement(tree: &HostTree, row_id: &str) -> Option<Vec<String>> {
    let found = locate_ident(tree, row_id)?;
    let mut lines = placed_identity(&found);
    if found.pinnable.is_some() {
        let ident = printable(&found.ident);
        lines.push(format!("  \"{ident}\" is the placement key for this row."));
    }
    Some(lines)
}

pub(crate) fn placed_identity(found: &Found) -> Vec<String> {
    let ident = printable(&found.ident);
    vec![
        format!("  placed    {}", printable(&found.path)),
        format!("  identity  {ident}"),
    ]
}

/// Compact `--explain` body: placed path, identity, instance, then `trace`.
/// The pin-to-folder recipe stays in `run`; the TUI overlay has no room for it.
pub(crate) fn placement_lines(
    found: &Found,
    process: Option<&Process>,
    rules: &Rules,
) -> Vec<String> {
    let mut lines = placed_identity(found);
    lines.push(format!(
        "  instance  {} ({} process{})",
        printable(&found.instance),
        found.siblings,
        if found.siblings == 1 { "" } else { "es" }
    ));
    lines.extend(
        trace(rules, process, &found.ident)
            .into_iter()
            .map(|line| printable(&line).into_owned()),
    );
    lines
}

/// One line per stage naming the rule that decided it, `no match`, or `none`
/// for a stage with no rules. It says what each stage makes of this process,
/// not why the tree placed it: a session rule can match a process a container
/// scope took first, and `placed` is the tree's verdict.
fn rule_origin(r: &crate::rules::Compiled) -> String {
    if r.source.rank == usize::MAX {
        format!("built-in {}", r.name())
    } else {
        format!("{}/{}", r.source.label, r.name())
    }
}

fn matched_rules(rules: &Rules, p: &Process, ident: &str) -> Vec<MatchedRule> {
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
        for r in rules.deciding(*stage, f) {
            out.push(MatchedRule {
                stage: label,
                rule: rule_origin(r),
                outputs: r.outputs(),
            });
        }
    }
    out
}

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
                .map(|r| format!("{} -> {}", rule_origin(r), r.outputs()))
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

fn explain_doc<'a>(
    pid: u32,
    visible: bool,
    found: Option<&'a Found>,
    process: Option<&Process>,
    rules: &Rules,
) -> ExplainDoc<'a> {
    let ident = found.map_or("", |f| f.ident.as_str());
    ExplainDoc {
        schema: 1,
        pid,
        visible,
        place: found.map(|f| f.path.as_str()),
        identity: found.map(|f| f.ident.as_str()),
        rules: process
            .map(|p| matched_rules(rules, p, ident))
            .unwrap_or_default(),
    }
}

/// `Ok(false)` when the pid is not visible, so a script can tell a miss from a
/// hit by the exit code.
///
/// # Errors
///
/// Returns an error if stdout cannot be written.
pub fn run(pid: u32, json: bool) -> Result<bool, Error> {
    let tree = proc::sample_placement();
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
    // Written, not printed: `println!` panics on a closed reader, while an
    // io error reaches `main`, which ends `--explain PID | head` quietly.
    let mut out = std::io::stdout().lock();
    let p = proc::read_pid(pid, false, false, None, &mut Vec::new());
    if json {
        let doc = explain_doc(pid, visible, found.as_ref(), p.as_ref(), Rules::load());
        writeln!(out, "{}", serde_json::to_string(&doc).expect("ExplainDoc"))?;
        return Ok(visible || found.is_some());
    }

    if !visible && found.is_none() {
        // Not an error message: the pid may simply have gone, and that is the
        // answer. Still a miss, so the exit code says so.
        writeln!(out, "pid {pid}: not visible\n")?;
        writeln!(
            out,
            concat!(
                "  It exited, or /proc hides it -- another user's process, a\n",
                "  hidepid mount, or a PID namespace. heft can only group what\n",
                "  it can walk.",
            )
        )?;
        return Ok(false);
    }

    writeln!(out, "pid {pid}")?;
    for (k, v) in &facts {
        writeln!(out, "  {k:<9} {}", printable(v))?;
    }

    let Some(f) = found else {
        writeln!(
            out,
            concat!(
                "\n",
                "  placed    nowhere: heft read this process but no row holds it.\n",
                "            A kernel thread with no cgroup, or it exited after\n",
                "            the walk grouping used.",
            )
        )?;
        return Ok(true);
    };

    // Re-read rather than carried on the tree: the tree has no per-process
    // facts, and a pid that exited since the sample says so here.
    writeln!(out)?;
    for line in placement_lines(&f, p.as_ref(), Rules::load()) {
        writeln!(out, "{line}")?;
    }

    writeln!(out)?;
    let ident = printable(&f.ident);
    if let Some(list) = f.pinnable {
        writeln!(out, "  \"{ident}\" is the placement key for this row.\n")?;
        writeln!(
            out,
            "  To pin it to a folder, in {}/rules.d/90-mine.json:",
            printable(&crate::config::config_dir().display().to_string())
        )?;
        writeln!(
            out,
            "    {{ \"stage\": \"placement\", \"rules\": [ {{ \"id\": \"pin\", \"match\": {{ \"identity\": {} }}, \"folder\": \"{list}\" }} ] }}",
            json_key(&f.ident)
        )?;
        writeln!(
            out,
            concat!(
                "\n",
                "  To bill it to another row instead, `\"fold_to\": \"<other identity>\"`\n",
                "  in place of `folder`.",
            )
        )?;
    } else {
        writeln!(
            out,
            concat!(
                "  No placement rule can move this row. A container is placed by its\n",
                "  runtime labels and a kernel thread by the cgroup it is in, so\n",
                "  `fold_to` and `folder` skip them. A rule with `owner_uid` sets a\n",
                "  container's owning uid by name.",
            )
        )?;
    }
    Ok(true)
}

/// `ident` as a JSON string that pastes into a rule file and puts no raw
/// control byte on the terminal. `serde_json` escapes a quote, a backslash and
/// C0 controls, but leaves DEL and the C1 controls raw.
fn json_key(ident: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for c in serde_json::Value::from(ident).to_string().chars() {
        if c.is_control() {
            // Every control left is at most U+009F, so four digits hold it.
            let _ = write!(out, "\\u{:04x}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
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

    fn ident(id: &str, pids: &[u32]) -> IdentNode {
        let nproc = u32::try_from(pids.len()).expect("a handful of pids");
        IdentNode {
            id: id.into(),
            nproc,
            instances: vec![InstanceNode {
                key: format!("{id}/1"),
                nproc,
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
    fn the_suggested_key_is_json_for_any_identity() {
        let ident = "x\u{9b}y\u{7f}\"\\\u{1b}";
        let key = json_key(ident);
        assert!(!key.chars().any(char::is_control), "{key}");
        assert_eq!(serde_json::from_str::<String>(&key).unwrap(), ident);
    }

    #[test]
    fn a_pid_in_no_row_is_not_an_error() {
        let tree = tree_with(user(vec![ident("cursor", &[10])], Vec::new()), vec![]);
        assert!(locate(&tree, 99).is_none());
    }

    #[test]
    fn placement_lines_name_the_row_and_omit_the_recipe() {
        let tree = tree_with(user(vec![ident("cursor", &[10, 11])], Vec::new()), vec![]);
        let found = locate(&tree, 11).expect("found");
        let lines = placement_lines(&found, None, &Rules::builtin());
        assert!(
            lines
                .iter()
                .any(|l| l.contains("placed") && l.contains("Applications")),
            "{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("identity") && l.contains("cursor")),
            "{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("instance") && l.contains("2 processes")),
            "{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("rules") && l.contains("process gone")),
            "{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .all(|l| !l.contains("90-mine.json") && !l.contains("fold_to")),
            "{lines:#?}"
        );
    }

    #[test]
    fn explain_json_is_a_small_object_with_schema_first() {
        let tree = tree_with(user(vec![ident("cursor", &[10, 11])], Vec::new()), vec![]);
        let found = locate(&tree, 11).expect("found");
        let kicker = Process {
            comm: "kicker".into(),
            exe: Some("/opt/trinity/bin/tdeinit".into()),
            ..Process::default()
        };
        let text = serde_json::to_string(&explain_doc(
            11,
            true,
            Some(&found),
            Some(&kicker),
            &Rules::builtin(),
        ))
        .unwrap();
        assert!(text.starts_with("{\"schema\":1,"), "{text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["pid"], 11);
        assert_eq!(v["visible"], true);
        assert!(
            v["place"].as_str().unwrap().contains("Applications"),
            "{v:?}"
        );
        assert_eq!(v["identity"], "cursor");
        let rules = v["rules"].as_array().unwrap();
        assert!(
            rules.iter().any(|r| r["stage"] == "session"
                && r["rule"].as_str().unwrap().contains("trinity-session")),
            "{rules:?}"
        );
    }

    #[test]
    fn explain_json_from_the_gui_fixture() {
        let fix: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/gui/world.json")).unwrap();
        let mut curr = crate::types::PidMap::default();
        for p in fix["processes"].as_array().unwrap() {
            let pid = u32::try_from(p["pid"].as_u64().unwrap()).unwrap();
            curr.insert(
                pid,
                Process {
                    pid,
                    ppid: u32::try_from(p["ppid"].as_u64().unwrap()).unwrap(),
                    pgrp: i32::try_from(p["pgrp"].as_u64().unwrap_or(u64::from(pid))).unwrap(),
                    uid: u32::try_from(p["uid"].as_u64().unwrap()).unwrap(),
                    comm: p["comm"].as_str().unwrap().into(),
                    exe: p["exe"].as_str().map(str::to_string),
                    cmdline: p["cmdline"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|a| a.as_str().unwrap().to_string())
                        .collect::<Vec<_>>()
                        .into(),
                    cgroup: p["cgroup"].as_str().unwrap().into(),
                    ..Process::default()
                },
            );
        }
        let header = crate::types::HostHeader {
            nproc: u32::try_from(fix["nproc"].as_u64().unwrap()).unwrap(),
            clk_tck: fix["clk_tck"].as_u64().unwrap(),
            page_size: fix["page_size"].as_u64().unwrap(),
        };
        let tree = crate::group::build_tree(
            &curr,
            &curr,
            std::time::Duration::from_secs(1),
            &header,
            HostTree::default(),
            &crate::containers::ContainerIndex::default(),
            &Rules::builtin(),
        );
        let pid = 501;
        let found = locate(&tree, pid).expect("cursor in gui fixture");
        let text = serde_json::to_string(&explain_doc(
            pid,
            true,
            Some(&found),
            curr.get(&pid),
            &Rules::builtin(),
        ))
        .unwrap();
        assert!(text.starts_with("{\"schema\":1,"), "{text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["pid"], pid);
        assert_eq!(v["visible"], true);
        assert!(v["identity"].as_str().unwrap().contains("cursor"), "{v:?}");
        assert!(v.get("host").is_none(), "{v:?}");
        assert!(v["place"].is_string(), "{v:?}");
    }

    #[test]
    fn identity_placement_is_the_key_without_rules() {
        let tree = tree_with(user(vec![ident("cursor", &[10])], Vec::new()), vec![]);
        let lines = identity_placement(&tree, "user:1000/apps/cursor").expect("found");
        assert_eq!(lines.len(), 3, "{lines:#?}");
        assert!(
            lines[0].contains("placed") && lines[0].contains("Applications"),
            "{lines:#?}"
        );
        assert!(
            lines[1].contains("identity") && lines[1].contains("cursor"),
            "{lines:#?}"
        );
        assert!(
            lines[2].contains("\"cursor\" is the placement key for this row."),
            "{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .all(|l| !l.contains("rules") && !l.contains("90-mine.json"))
        );
        let system = tree_with(user(Vec::new(), Vec::new()), vec![ident("kworker", &[3])]);
        let sys = identity_placement(&system, "host/system/kworker").expect("found");
        assert_eq!(sys.len(), 2, "{sys:#?}");
        assert!(sys.iter().all(|l| !l.contains("placement key")), "{sys:#?}");
        assert!(identity_placement(&tree, "user:1000/apps").is_none());
    }
}
