use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::classify::{self, name_of};
use crate::containers::{self, ContainerIndex};
use crate::cpu::process_metrics;
use crate::identity::{self, docker_scope_id};
use crate::proc;
use crate::rules::{Classes, Facts, Rules, Stage, UnitFlags};
use crate::types::{
    Folder, HostHeader, HostTree, IdentNode, InstanceNode, MemberContainer, Metrics, ProcNode,
    Process, UserNode,
};

/// One tick's processes by pid, as `proc` samples them.
type Procs = HashMap<u32, Process>;

/// The read-only inputs every placement rule needs, bundled so the recursive
/// walk keeps one parameter instead of several.
struct Ctx<'a> {
    containers: &'a ContainerIndex,
    rules: &'a Rules,
    /// This tick's processes, for the crash helper's install-directory sibling.
    procs: &'a Procs,
    /// Per-tick, per-pid facts the stages read. Keyed on pid and valid for one
    /// `Ctx` only: `exec` keeps the pid while changing exe and comm, and a
    /// `Ctx` lives one `build_tree` call, so no invalidation is needed.
    judged: HashMap<u32, Judged>,
}

impl<'a> Ctx<'a> {
    fn new(containers: &'a ContainerIndex, rules: &'a Rules, curr: &'a Procs) -> Self {
        let judged = curr
            .iter()
            .map(|(pid, p)| (*pid, judge(p, rules)))
            .collect();
        Self {
            containers,
            rules,
            procs: curr,
            judged,
        }
    }
    fn facts<'p>(&'p self, p: &'p Process) -> Facts<'p> {
        facts_of(p, self.judged[&p.pid].unit.as_deref())
    }
    fn classes(&self, p: &Process) -> Classes {
        self.judged[&p.pid].classes
    }
    fn judged(&self, p: &Process) -> &Judged {
        &self.judged[&p.pid]
    }
    fn instance(&self, p: &Process) -> String {
        identity::instance_key(p, None, self.judged(p))
    }
}

pub(crate) fn facts_of<'p>(p: &'p Process, unit: Option<&'p str>) -> Facts<'p> {
    Facts {
        comm: &p.comm,
        name: classify::name_ref(p),
        exe: p.exe.as_deref(),
        argv: &p.cmdline,
        cgroup: &p.cgroup,
        unit,
        script: None,
        identity: None,
        container: None,
    }
}

/// What the rule stages need of one process, computed once per pid per tick.
pub(crate) struct Judged {
    pub(crate) unit: Option<String>,
    pub(crate) unit_flags: UnitFlags,
    classes: Classes,
}

fn judge(p: &Process, rules: &Rules) -> Judged {
    let unit = identity::user_unit(&p.cgroup);
    let unit_flags = unit
        .as_deref()
        .map_or_else(UnitFlags::default, |u| rules.unit_flags(u));
    let mut classes = rules.classes(&facts_of(p, unit.as_deref()));
    // A shell wrapper reports the shell as comm; the launcher it execs is the
    // first positional argument. A procedure over argv, not a class rule.
    if let Some(arg) = p.cmdline.iter().skip(1).find(|a| !a.starts_with('-'))
        && rules
            .classes_of_name(classify::basename(arg))
            .intersects(Classes::LAUNCHER)
    {
        classes |= Classes::LAUNCHER;
    }
    Judged {
        unit,
        unit_flags,
        classes,
    }
}

#[derive(Clone)]
struct Place {
    folder: Folder,
    uid: Option<u32>,
    key: String,
    instance: String,
    member: Option<String>,
}
#[must_use]
pub fn build_tree(
    prev: &Procs,
    curr: &Procs,
    elapsed: Duration,
    consts: &HostHeader,
    header: HostTree,
    containers: &ContainerIndex,
    rules: &Rules,
) -> HostTree {
    let places = resolve(curr, &Ctx::new(containers, rules, curr));
    let metrics = metrics_map(prev, curr, elapsed, consts);
    assemble(curr, &places, &metrics, header)
}

fn metrics_map(
    prev: &Procs,
    curr: &Procs,
    elapsed: Duration,
    consts: &HostHeader,
) -> HashMap<u32, Metrics> {
    curr.iter()
        .map(|(pid, p)| (*pid, process_metrics(prev.get(pid), p, elapsed, consts)))
        .collect()
}

fn resolve(curr: &Procs, ctx: &Ctx<'_>) -> HashMap<u32, Place> {
    let mut memo: HashMap<u32, Place> = HashMap::new();
    let mut walking = HashSet::new();
    for pid in curr.keys().copied() {
        resolve_one(pid, curr, ctx, &mut memo, &mut walking);
    }
    memo
}

fn resolve_one(
    pid: u32,
    curr: &Procs,
    ctx: &Ctx<'_>,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Option<Place> {
    if let Some(p) = memo.get(&pid) {
        return Some(p.clone());
    }
    let p = curr.get(&pid)?;
    if !walking.insert(pid) {
        return Some(override_place(ctx.rules, raw_place(p, ctx)));
    }
    let place = override_place(ctx.rules, compute_place(p, curr, ctx, memo, walking));
    walking.remove(&pid);
    memo.insert(pid, place.clone());
    Some(place)
}

fn compute_place(
    p: &Process,
    curr: &Procs,
    ctx: &Ctx<'_>,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Place {
    if let Some(place) = direct_place(p, ctx) {
        return place;
    }

    let classes = ctx.classes(p);
    if classes.intersects(Classes::WORKER)
        && let Some(parent) = resolve_one(p.ppid, curr, ctx, memo, walking)
        && parent.folder != Folder::System
        // `key` is the ppid's RESOLVED Place identity, not its process name, and
        // the two disagree both ways: a `systemd` that resolved into a container
        // is a legal fold target here, while a non-systemd process that folded
        // onto the user-manager row is not. Not interchangeable with the raw-name
        // check in `classify::absorbs_generic`.
        && parent.key != "systemd"
    {
        return Place {
            instance: ctx.instance(p),
            ..parent
        };
    }

    // Pipe helpers under a launcher (flatpak bwrap `cat`) or an app (vivaldi).
    // Immediate parent only — never a sibling identity under a mixed shell.
    if classes.intersects(Classes::NOISE)
        && let Some(parent) = resolve_one(p.ppid, curr, ctx, memo, walking)
        && parent.folder != Folder::System
    {
        return Place {
            instance: ctx.instance(p),
            ..parent
        };
    }

    // Launchers and shells share unique-payload folding: the helper has no
    // top-level row when a single child identity exists. Interactive shells
    // are included so a `bash` that launched `claude` bills there; an idle
    // leftover folds into the terminal below, not here.
    if classes.intersects(Classes::LAUNCHER | Classes::SHELL) {
        if let Some(payload) = unique_descendant_ident(p.pid, curr, ctx, memo, walking) {
            return Place {
                instance: ctx.instance(p),
                ..payload
            };
        }
        if classes.intersects(Classes::LAUNCHER) {
            if let Some(hint) = classify::launcher_payload_hint(p, ctx.rules) {
                let mut place = user_place(p, ctx);
                place.key = hint;
                return place;
            }
            if let Some(parent) = resolve_one(p.ppid, curr, ctx, memo, walking)
                && parent.folder != Folder::System
                && !ctx
                    .rules
                    .classes_of_name(&parent.key)
                    .intersects(Classes::LAUNCHER)
            {
                return Place {
                    instance: ctx.instance(p),
                    ..parent
                };
            }
        }
        // Idle interactive shell: not an application. The resolved parent
        // identity is the terminal that owns the tty. A unique payload child
        // already returned above, same walk as a launcher.
        if classify::is_interactive_shell(p, classes)
            && let Some(parent) = resolve_one(p.ppid, curr, ctx, memo, walking)
            && parent.folder != Folder::System
            && ctx
                .rules
                .classes_of_name(&parent.key)
                .intersects(Classes::TERMINAL)
        {
            return Place {
                instance: ctx.instance(p),
                ..parent
            };
        }
    }

    if classes.intersects(Classes::GENERIC)
        && let Some(owner) = owning_app_ancestor(p.ppid, curr, ctx, memo, walking)
    {
        return Place {
            instance: ctx.instance(p),
            ..owner
        };
    }

    user_place(p, ctx)
}

fn container_place(p: &Process, ctx: &Ctx<'_>) -> Option<Place> {
    let containers = ctx.containers;
    let runtime = ctx.classes(p).intersects(Classes::CONTAINER_RUNTIME);
    if let Some(info) = containers.lookup_process(p, runtime) {
        return Some(Place {
            folder: Folder::Containers,
            uid: info.owner_uid,
            key: info.ident_key.clone(),
            instance: identity::instance_key(p, Some(&info.id), ctx.judged(p)),
            member: info.member_name.clone(),
        });
    }
    let scope = docker_scope_id(&p.cgroup);
    if let Some(id) = scope.clone().or_else(|| containers::helper_id(p, runtime)) {
        return Some(Place {
            folder: Folder::Containers,
            // Only a cgroup id is known on this path, so there is no name or
            // label to attribute an owner from; lookup_process does that.
            uid: None,
            key: containers::docker_title(&id),
            instance: identity::instance_key(p, scope.as_deref(), ctx.judged(p)),
            member: None,
        });
    }
    None
}

/// A systemd-nspawn container, `machinectl` machine, or libvirt VM. Without
/// this they fell through to `user_place`: an nspawn container appeared as one
/// Applications row per process under root, a VM as a `qemu-system-x86_64` row.
///
/// `uid: None` puts it on Host → Containers, where the tree already places a
/// container it cannot attribute. There is no API here to ask who owns it, and
/// the uid running it is a service account — a `qemu` User node holding one
/// VM says less than the machine's own Containers folder does.
fn machine_place(p: &Process, ctx: &Ctx<'_>) -> Option<Place> {
    let name = identity::machine_scope_name(&p.cgroup)?;
    Some(Place {
        folder: Folder::Containers,
        uid: None,
        instance: identity::instance_key(p, Some(&name), ctx.judged(p)),
        key: name,
        member: None,
    })
}

fn system_place(p: &Process, ctx: &Ctx<'_>) -> Place {
    Place {
        folder: Folder::System,
        uid: None,
        key: if identity::is_kernel(p) {
            "kernel".to_string()
        } else {
            name_of(p)
        },
        instance: ctx.instance(p),
        member: None,
    }
}

fn user_place(p: &Process, ctx: &Ctx<'_>) -> Place {
    let j = ctx.judged(p);
    let folder = if j.classes.intersects(Classes::COMPOSITOR)
        || (j.unit_flags.contains(UnitFlags::SERVICE) && !j.unit_flags.contains(UnitFlags::LYING))
    {
        Folder::UserServices
    } else {
        Folder::Applications
    };
    Place {
        folder,
        uid: Some(p.uid),
        key: if j.classes.intersects(Classes::GENERIC) {
            identity::generic_fallback(p, j, ctx.rules)
        } else {
            name_of(p)
        },
        instance: ctx.instance(p),
        member: None,
    }
}

fn session_plumbing_place(p: &Process, ctx: &Ctx<'_>) -> Option<Place> {
    let (key, folder) = ctx.rules.session(&ctx.facts(p))?;
    Some(Place {
        folder,
        uid: Some(p.uid),
        key: key.to_string(),
        instance: ctx.instance(p),
        member: None,
    })
}

fn crash_helper_place(p: &Process, ctx: &Ctx<'_>) -> Option<Place> {
    let owner = classify::crash_helper_app(p, ctx.classes(p), ctx.rules, ctx.procs.values())?;
    Some(Place {
        folder: Folder::Applications,
        uid: Some(p.uid),
        key: owner,
        instance: ctx.instance(p),
        member: None,
    })
}

/// The bucket rules that need no ancestor walk, so the cycle-breaking path can
/// answer with the same verdict `compute_place` would give instead of a subset.
fn direct_place(p: &Process, ctx: &Ctx<'_>) -> Option<Place> {
    if let Some(place) = container_place(p, ctx) {
        return Some(place);
    }
    if let Some(place) = machine_place(p, ctx) {
        return Some(place);
    }
    if identity::is_kernel(p)
        || (identity::in_system_slice(&p.cgroup) && !identity::in_user_slice(&p.cgroup))
    {
        return Some(system_place(p, ctx));
    }
    session_plumbing_place(p, ctx)
        .or_else(|| crash_helper_place(p, ctx))
        .or_else(|| {
            let app = ctx.rules.app(&ctx.facts(p))?;
            Some(Place {
                key: app.to_string(),
                ..user_place(p, ctx)
            })
        })
}

fn raw_place(p: &Process, ctx: &Ctx<'_>) -> Place {
    direct_place(p, ctx).unwrap_or_else(|| user_place(p, ctx))
}

/// Apply the placement stage to a finished placement. It runs last, so a pin
/// beats every built-in table; it runs only on the two user-owned folders, so
/// no rule can pull a container or a kernel thread out of where it belongs.
///
/// Two lookups: `placement(old)` for `fold_to`, then the folder from that same
/// rule when it carries one, else from `placement(key)` when the key changed.
fn override_place(rules: &Rules, place: Place) -> Place {
    if rules.is_empty(Stage::Placement)
        || !matches!(place.folder, Folder::Applications | Folder::UserServices)
    {
        return place;
    }
    let r1 = rules.placement(&place.key);
    let key = r1.and_then(|r| r.fold_to.clone()).unwrap_or(place.key);
    let folder = match r1.and_then(|r| r.folder) {
        Some(f) => f,
        None if r1.is_some_and(|r| r.fold_to.is_some()) => rules
            .placement(&key)
            .and_then(|r| r.folder)
            .unwrap_or(place.folder),
        None => place.folder,
    };
    Place {
        folder,
        key,
        ..place
    }
}

fn owning_app_ancestor(
    mut pid: u32,
    curr: &Procs,
    ctx: &Ctx<'_>,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Option<Place> {
    for _ in 0..32 {
        let proc = curr.get(&pid)?;
        let classes = ctx.classes(proc);
        if classes
            .intersects(Classes::LAUNCHER | Classes::GENERIC | Classes::SHELL | Classes::NOISE)
        {
            pid = proc.ppid;
            continue;
        }
        if !classify::absorbs_generic(classes) {
            return None;
        }
        let place = resolve_one(pid, curr, ctx, memo, walking)?;
        if place.folder == Folder::System || place.folder == Folder::Containers {
            return None;
        }
        return Some(place);
    }
    None
}

fn unique_descendant_ident(
    pid: u32,
    curr: &Procs,
    ctx: &Ctx<'_>,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Option<Place> {
    let mut kids = Vec::new();
    for child in curr.values().filter(|c| c.ppid == pid) {
        let classes = ctx.classes(child);
        if classes.intersects(Classes::LAUNCHER | Classes::SHELL | Classes::NOISE) {
            if let Some(p) = unique_descendant_ident(child.pid, curr, ctx, memo, walking) {
                kids.push(p);
            }
            continue;
        }
        if classes.intersects(Classes::WORKER) {
            if let Some(p) = unique_descendant_ident(child.pid, curr, ctx, memo, walking) {
                kids.push(p);
            } else {
                // Zygote-only sandbox: no non-worker grandchild to resolve, so
                // the helper answers for itself. `raw_place` rather than
                // `user_place` because a worker can still be a crash helper
                // that names its own app, or live in a container.
                kids.push(raw_place(child, ctx));
            }
            continue;
        }
        if let Some(place) = resolve_one(child.pid, curr, ctx, memo, walking) {
            kids.push(place);
        }
    }
    if kids.is_empty() {
        return None;
    }
    let first = kids[0].key.clone();
    if kids.iter().all(|k| k.key == first) {
        Some(kids[0].clone())
    } else {
        None
    }
}

fn assemble(
    curr: &Procs,
    places: &HashMap<u32, Place>,
    metrics: &HashMap<u32, Metrics>,
    mut header: HostTree,
) -> HostTree {
    #[derive(Default)]
    struct Bucket {
        pids: Vec<u32>,
        members: HashMap<String, Vec<u32>>,
    }

    let mut buckets: HashMap<(Folder, Option<u32>, String), Bucket> = HashMap::new();
    for (pid, place) in places {
        let b = buckets
            .entry((place.folder, place.uid, place.key.clone()))
            .or_default();
        b.pids.push(*pid);
        if let Some(m) = &place.member {
            b.members.entry(m.clone()).or_default().push(*pid);
        }
    }

    let passwd = proc::Passwd::read();
    let mut users: HashMap<u32, UserNode> = HashMap::new();
    let mut host_containers = Vec::new();
    let mut system = Vec::new();

    for ((folder, uid, key), bucket) in buckets {
        let node = ident_node(&key, &bucket.pids, &bucket.members, curr, places, metrics);
        match folder {
            Folder::System => system.push(node),
            Folder::Containers if uid.is_none() => host_containers.push(node),
            Folder::Applications | Folder::UserServices | Folder::Containers => {
                let uid = uid.unwrap_or(0);
                let user = users.entry(uid).or_insert_with(|| UserNode {
                    uid,
                    name: passwd.name(uid),
                    ..UserNode::default()
                });
                match folder {
                    Folder::Applications => user.applications.push(node),
                    Folder::UserServices => user.user_services.push(node),
                    Folder::Containers => user.containers.push(node),
                    Folder::System => {}
                }
            }
        }
    }

    let our = crate::cpu::euid();
    let mut user_list: Vec<UserNode> = users.into_values().collect();
    user_list.sort_by(|a, b| {
        (a.uid != our)
            .cmp(&(b.uid != our))
            .then(a.name.cmp(&b.name))
            .then(a.uid.cmp(&b.uid))
    });
    header.users = user_list;
    header.containers = host_containers;
    header.system = system;
    header
}

fn ident_node(
    key: &str,
    pids: &[u32],
    members_map: &HashMap<String, Vec<u32>>,
    curr: &Procs,
    places: &HashMap<u32, Place>,
    metrics: &HashMap<u32, Metrics>,
) -> IdentNode {
    let mut by_inst: HashMap<String, Vec<u32>> = HashMap::new();
    for pid in pids {
        let inst = places
            .get(pid)
            .map_or_else(|| format!("pid:{pid}"), |p| p.instance.clone());
        by_inst.entry(inst).or_default().push(*pid);
    }
    let instances: Vec<InstanceNode> = by_inst
        .into_iter()
        .map(|(k, inst_pids)| InstanceNode {
            nproc: u32::try_from(inst_pids.len()).unwrap_or(u32::MAX),
            metrics: sum_metrics(&inst_pids, metrics),
            processes: proc_forest(&inst_pids, curr, metrics),
            key: k,
        })
        .collect();
    let members: Vec<MemberContainer> = members_map
        .iter()
        .map(|(name, mpids)| MemberContainer {
            id: name.clone(),
            title: name.clone(),
            nproc: u32::try_from(mpids.len()).unwrap_or(u32::MAX),
            metrics: sum_metrics(mpids, metrics),
            processes: proc_forest(mpids, curr, metrics),
        })
        .collect();
    IdentNode {
        id: key.to_string(),
        title: key.to_string(),
        nproc: u32::try_from(pids.len()).unwrap_or(u32::MAX),
        metrics: sum_metrics(pids, metrics),
        instances,
        containers: members,
    }
}

fn sum_metrics(pids: &[u32], metrics: &HashMap<u32, Metrics>) -> Metrics {
    let mut m = Metrics::default();
    for pid in pids {
        if let Some(x) = metrics.get(pid) {
            m.accumulate(x);
        }
    }
    m
}

/// The deepest a `ProcNode` nests, a root being depth 1. A node at depth
/// `MAX_PROC_DEPTH - 1` takes every descendant as a flat child in pid order, so
/// no process is dropped and no walker over `children` recurses further than
/// this: an unprivileged fork chain thousands deep would otherwise overflow the
/// sampler thread's stack, which aborts rather than panics.
///
/// The bound is `--json`, parsed back by `serde_json`, which refuses the 128th
/// nested array or object. A `ProcNode` at depth `d` sits `8 + 2d` levels in
/// (`{` document, `host`, `users[`, user, `applications[`, identity,
/// `instances[`, instance, `processes[`, then an object and a `children[` per
/// level; a member container's `containers[`/`processes[` is the same depth),
/// and the deepest node has no `children` array. 8 + 2 × 48 = 104 of 127.
const MAX_PROC_DEPTH: usize = 48;

fn proc_forest(pids: &[u32], curr: &Procs, metrics: &HashMap<u32, Metrics>) -> Vec<ProcNode> {
    let set: HashSet<u32> = pids.iter().copied().collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut roots = Vec::new();
    for pid in pids {
        let ppid = curr.get(pid).map_or(0, |p| p.ppid);
        if set.contains(&ppid) {
            children.entry(ppid).or_default().push(*pid);
        } else {
            roots.push(*pid);
        }
    }
    roots.sort_unstable();
    roots
        .into_iter()
        .map(|pid| proc_node(pid, 1, &children, curr, metrics))
        .collect()
}

fn proc_node(
    pid: u32,
    depth: usize,
    children: &HashMap<u32, Vec<u32>>,
    curr: &Procs,
    metrics: &HashMap<u32, Metrics>,
) -> ProcNode {
    let name = curr.get(&pid).map_or_else(|| pid.to_string(), name_of);
    let cmdline = curr
        .get(&pid)
        .map(|p| p.cmdline.join(" "))
        .unwrap_or_default();
    let mut kids = match (depth + 1).cmp(&MAX_PROC_DEPTH) {
        std::cmp::Ordering::Less => children.get(&pid).cloned().unwrap_or_default(),
        std::cmp::Ordering::Equal => descendants(pid, children),
        std::cmp::Ordering::Greater => Vec::new(),
    };
    kids.sort_unstable();
    ProcNode {
        pid,
        name,
        cmdline,
        metrics: metrics.get(&pid).cloned().unwrap_or_default(),
        children: kids
            .into_iter()
            .map(|c| proc_node(c, depth + 1, children, curr, metrics))
            .collect(),
    }
}

/// Every descendant of `pid`, walked with an explicit stack so a chain of any
/// depth costs heap rather than stack. Each pid has one parent, so no pid
/// reachable from a root is visited twice.
fn descendants(pid: u32, children: &HashMap<u32, Vec<u32>>) -> Vec<u32> {
    let mut out = Vec::new();
    let mut stack = children.get(&pid).cloned().unwrap_or_default();
    while let Some(c) = stack.pop() {
        out.push(c);
        if let Some(k) = children.get(&c) {
            stack.extend_from_slice(k);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Ctx, Folder, Process, raw_place};
    use crate::containers::ContainerIndex;
    use crate::rules::Rules;
    use std::collections::HashMap;

    fn place_alone(p: Process) -> super::Place {
        let curr = HashMap::from([(p.pid, p)]);
        let rules = Rules::builtin();
        let containers = ContainerIndex::default();
        let ctx = Ctx::new(&containers, &rules, &curr);
        raw_place(&curr[&0], &ctx)
    }

    #[test]
    fn cycle_path_still_bills_a_crash_helper_to_its_app() {
        let place = place_alone(Process {
            comm: "crashhelper".into(),
            exe: Some("/usr/lib64/firefox/crashhelper".into()),
            cmdline: vec!["crashhelper".into(), "12766".into()],
            uid: 1000,
            cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice".into(),
            ..Process::default()
        });
        assert_eq!(place.folder, Folder::Applications);
        assert_eq!(place.key, "firefox");
    }

    /// machine.slice is neither a docker/libpod scope nor system.slice, so a
    /// VM's qemu process and every process inside an nspawn container would
    /// fall through to `user_place` and show up as ordinary Applications rows
    /// under whichever uid ran them.
    #[test]
    fn a_vm_is_a_container_row_rather_than_root_s_application() {
        let place = |cgroup: &str, uid: u32| {
            place_alone(Process {
                comm: "qemu-system-x86".into(),
                exe: Some("/usr/bin/qemu-system-x86_64".into()),
                uid,
                cgroup: cgroup.into(),
                ..Process::default()
            })
        };
        let vm = place(
            r"0::/machine.slice/machine-qemu-3-fedora.scope/libvirt/emulator",
            107,
        );
        assert_eq!(vm.folder, Folder::Containers);
        assert_eq!(vm.key, "fedora");
        // Nothing here can say who owns it, so it is the machine's own, the
        // same place an unattributable Docker container sits.
        assert_eq!(vm.uid, None);

        // The slice alone is not a machine: a stray process directly in
        // machine.slice has no scope to name and must not become a row.
        assert_ne!(place("0::/machine.slice", 0).folder, Folder::Containers);
    }
    #[test]
    fn a_deep_process_chain_is_capped_without_losing_a_process() {
        use crate::types::{HostHeader, HostTree, ProcNode};

        fn walk(nodes: &[ProcNode], depth: usize, max: &mut usize, count: &mut u64, rss: &mut u64) {
            for n in nodes {
                *max = (*max).max(depth);
                *count += 1;
                *rss += n.metrics.rss_bytes.unwrap_or(0);
                walk(&n.children, depth + 1, max, count, rss);
            }
        }

        const N: u32 = 10_000;
        let curr: HashMap<u32, Process> = (1..=N)
            .map(|pid| {
                let p = Process {
                    pid,
                    ppid: pid - 1,
                    pgrp: 1,
                    uid: 1000,
                    comm: "chain".into(),
                    exe: Some("/usr/bin/chain".into()),
                    cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice".into(),
                    rss_pages: Some(1),
                    ..Process::default()
                };
                (pid, p)
            })
            .collect();
        let consts = HostHeader {
            nproc: 1,
            clk_tck: 100,
            page_size: 4096,
        };
        let tree = super::build_tree(
            &HashMap::new(),
            &curr,
            std::time::Duration::from_secs(1),
            &consts,
            HostTree::default(),
            &ContainerIndex::default(),
            &Rules::builtin(),
        );

        let ident = &tree.users[0].applications[0];
        assert_eq!(ident.nproc, N);
        let (mut max, mut count, mut rss) = (0, 0, 0);
        for inst in &ident.instances {
            walk(&inst.processes, 1, &mut max, &mut count, &mut rss);
        }
        assert_eq!(max, super::MAX_PROC_DEPTH);
        assert_eq!(count, u64::from(N));
        assert_eq!(Some(rss), ident.metrics.rss_bytes);

        let text = serde_json::to_string(&serde_json::json!({ "host": tree })).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap();
    }
}
