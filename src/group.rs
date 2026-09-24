use std::collections::HashMap;
use std::time::Duration;

use crate::classify::{self, name_of};
use crate::containers::{self, ContainerIndex};
use crate::cpu::process_metrics;
use crate::identity::{self, docker_scope_id};
use crate::proc;
use crate::rules::{Classes, Facts, Rules, Stage, UnitFlags};
use crate::types::{
    Folder, HostHeader, HostTree, IdentNode, InstanceNode, MemberContainer, Metrics, PidMap,
    PidSet, ProcNode, Process, UserNode,
};

/// One tick's processes by pid, as `proc` samples them.
type Procs = PidMap<Process>;

/// The read-only inputs every placement rule needs, bundled so the recursive
/// walk keeps one parameter instead of several.
struct Ctx<'a> {
    containers: &'a ContainerIndex,
    rules: &'a Rules,
    /// This tick's processes, for the crash helper's install-directory sibling.
    procs: &'a Procs,
    /// Each pid's children in pid order, so the payload search visits a child
    /// without scanning every process at every level.
    children: PidMap<Vec<u32>>,
    /// Per-tick, per-pid facts the stages read. Keyed on pid and valid for one
    /// `Ctx` only: `exec` keeps the pid while changing exe and comm, and a
    /// `Ctx` lives one `build_tree` call, so no invalidation is needed.
    judged: PidMap<Judged>,
    /// The app each `app-…` scope names, for the scopes where a process of
    /// that name is running: everything else in such a scope is that app's
    /// own helper, whatever it calls itself. Empty for a scope that names
    /// nothing running in it, which is what keeps a terminal's or a browser's
    /// scope from swallowing the commands launched from it.
    scope_app: HashMap<String, String>,
}

impl<'a> Ctx<'a> {
    fn new(containers: &'a ContainerIndex, rules: &'a Rules, curr: &'a Procs) -> Self {
        let judged: PidMap<Judged> = curr
            .iter()
            .map(|(pid, p)| (*pid, judge(p, rules)))
            .collect();
        let mut children: PidMap<Vec<u32>> = PidMap::default();
        for p in curr.values() {
            children.entry(p.ppid).or_default().push(p.pid);
        }
        for kids in children.values_mut() {
            kids.sort_unstable();
        }
        let mut members: HashMap<&str, Vec<&str>> = HashMap::new();
        for (pid, p) in curr {
            let j = &judged[pid];
            if let Some(u) = j.unit.as_deref()
                && !j.unit_flags.contains(UnitFlags::LYING)
            {
                members.entry(u).or_default().push(classify::name_ref(p));
            }
        }
        let scope_app = members
            .iter()
            .filter_map(|(unit, names)| {
                let (full, short) = identity::app_scope_names(unit)?;
                let app = [Some(full), short]
                    .into_iter()
                    .flatten()
                    .find(|c| names.iter().any(|n| n.eq_ignore_ascii_case(c)))?;
                Some(((*unit).to_string(), app.to_string()))
            })
            .collect();
        drop(members);
        Self {
            scope_app,
            containers,
            rules,
            procs: curr,
            children,
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
    /// The `app-…` scope's app, for a process whose unit is one of the scopes
    /// `scope_app` resolved.
    fn scope_app(&self, j: &Judged) -> Option<&str> {
        self.scope_app.get(j.unit.as_deref()?).map(String::as_str)
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
) -> PidMap<Metrics> {
    curr.iter()
        .map(|(pid, p)| (*pid, process_metrics(prev.get(pid), p, elapsed, consts)))
        .collect()
}

fn resolve(curr: &Procs, ctx: &Ctx<'_>) -> PidMap<Place> {
    let mut memo: PidMap<Place> = PidMap::default();
    let mut walking = PidSet::default();
    for pid in curr.keys().copied() {
        resolve_one(pid, curr, ctx, &mut memo, &mut walking, 0);
    }
    memo
}

/// How many walker frames one placement may stack, shared by `resolve_one`
/// climbing parents and `unique_descendant_ident` descending launchers, shells
/// and workers. A real tree nests those a handful deep (flatpak's bwrap, bwrap,
/// zypak-helper, zygote is four); a fork chain thousands deep would otherwise
/// overflow the sampler thread's stack, which aborts rather than panics.
///
/// At the bound a parent answers with `raw_place`, the verdict the cycle path
/// already gives, and the payload search finds nothing. Where that cut lands
/// in a chain this deep depends on resolve order.
const MAX_WALK: usize = 64;

fn resolve_one(
    pid: u32,
    curr: &Procs,
    ctx: &Ctx<'_>,
    memo: &mut PidMap<Place>,
    walking: &mut PidSet,
    depth: usize,
) -> Option<Place> {
    if let Some(p) = memo.get(&pid) {
        return Some(p.clone());
    }
    let p = curr.get(&pid)?;
    if depth >= MAX_WALK || !walking.insert(pid) {
        return Some(override_place(ctx.rules, raw_place(p, ctx)));
    }
    let place = override_place(ctx.rules, compute_place(p, curr, ctx, memo, walking, depth));
    walking.remove(&pid);
    memo.insert(pid, place.clone());
    Some(place)
}

fn compute_place(
    p: &Process,
    curr: &Procs,
    ctx: &Ctx<'_>,
    memo: &mut PidMap<Place>,
    walking: &mut PidSet,
    depth: usize,
) -> Place {
    if let Some(place) = direct_place(p, ctx) {
        return place;
    }

    let classes = ctx.classes(p);
    if classes.intersects(Classes::WORKER) {
        if let Some(parent) = resolve_one(p.ppid, curr, ctx, memo, walking, depth + 1)
            && parent.folder != Folder::System
            // Asked of the ppid's RESOLVED identity, not its process: a `systemd`
            // that resolved into a container is a legal fold target, while any
            // process that folded onto a no_absorb row is not. Not interchangeable
            // with `classify::absorbs_generic`, which judges the raw process.
            && !ctx
                .rules
                .classes_of_name(&parent.key)
                .intersects(Classes::NO_ABSORB)
        {
            return Place {
                instance: ctx.instance(p),
                ..parent
            };
        }
        if let Some(place) = lying_scope_place(p, curr, ctx, memo, walking, depth + 1) {
            return place;
        }
    }

    // Pipe helpers under a launcher (flatpak bwrap `cat`) or an app (vivaldi).
    // Immediate parent only — never a sibling identity under a mixed shell.
    // A no_absorb parent is not a fold target; use the lying-scope owner.
    if classes.intersects(Classes::NOISE) {
        let parent_is_no_absorb = curr
            .get(&p.ppid)
            .is_some_and(|q| ctx.classes(q).intersects(Classes::NO_ABSORB));
        if parent_is_no_absorb {
            if let Some(place) = lying_scope_place(p, curr, ctx, memo, walking, depth + 1) {
                return place;
            }
        } else if let Some(parent) = resolve_one(p.ppid, curr, ctx, memo, walking, depth + 1)
            && parent.folder != Folder::System
        {
            return Place {
                instance: ctx.instance(p),
                ..parent
            };
        }
    }

    // Launchers and shells share unique-payload folding: the helper has no
    // top-level row when a single child identity exists. A shell with only
    // noise descendants folds into the launching app or a terminal below.
    if classes.intersects(Classes::LAUNCHER | Classes::SHELL) {
        if let Some(payload) = unique_descendant_ident(p.pid, curr, ctx, memo, walking, depth + 1) {
            return Place {
                instance: ctx.instance(p),
                ..payload
            };
        }
        if classes.intersects(Classes::LAUNCHER) {
            if let Some(prefix) = classify::appimage_mount_prefix(p)
                && let Some(app) = ctx
                    .procs
                    .values()
                    .filter(|q| {
                        classify::in_own_session(p, q)
                            && q.exe
                                .as_deref()
                                .is_some_and(|e| classify::in_appimage_mount(e, &prefix))
                            && !ctx.classes(q).intersects(
                                Classes::LAUNCHER | Classes::CRASH_HELPER | Classes::WORKER,
                            )
                    })
                    .min_by_key(|q| q.pid)
                && let Some(place) = resolve_one(app.pid, curr, ctx, memo, walking, depth + 1)
                && !matches!(place.folder, Folder::System | Folder::Containers)
            {
                return Place {
                    instance: ctx.instance(p),
                    ..place
                };
            }
            if let Some(hint) = classify::launcher_payload_hint(p, ctx.rules) {
                let mut place = user_place(p, ctx);
                place.key = hint;
                return place;
            }
            if let Some(parent) = resolve_one(p.ppid, curr, ctx, memo, walking, depth + 1)
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
        // No unique non-noise payload: fold into the launching app, else a
        // terminal-class ancestor. Covers idle interactive shells and
        // `bash -c` that only ran utilities.
        if classes.intersects(Classes::SHELL) {
            if let Some(owner) = owning_app_ancestor(p.ppid, curr, ctx, memo, walking, depth + 1)
                && matches!(owner.folder, Folder::Applications | Folder::UserServices)
            {
                return Place {
                    instance: ctx.instance(p),
                    ..owner
                };
            }
            let mut pid = p.ppid;
            for d in (depth + 1)..MAX_WALK {
                let Some(anc) = curr.get(&pid) else {
                    break;
                };
                if let Some(place) = resolve_one(pid, curr, ctx, memo, walking, d)
                    && place.folder != Folder::System
                    && ctx
                        .rules
                        .classes_of_name(&place.key)
                        .intersects(Classes::TERMINAL)
                {
                    return Place {
                        instance: ctx.instance(p),
                        ..place
                    };
                }
                pid = anc.ppid;
            }
        }
    }

    if classes.intersects(Classes::GENERIC)
        && let Some(owner) = owning_app_ancestor(p.ppid, curr, ctx, memo, walking, depth + 1)
    {
        return Place {
            instance: ctx.instance(p),
            ..owner
        };
    }

    // An orphan reparented to `systemd --user` keeps the cgroup of whoever
    // started it. A lying unit skips sibling adoption: that sibling is often a
    // utility, not the app. Prefer the Applications ancestor, else the
    // terminal the scope names.
    if classes.intersects(Classes::GENERIC)
        && curr
            .get(&p.ppid)
            .is_some_and(|q| ctx.classes(q).intersects(Classes::NO_ABSORB))
    {
        if ctx.judged(p).unit_flags.contains(UnitFlags::LYING) {
            if let Some(place) = lying_scope_place(p, curr, ctx, memo, walking, depth + 1) {
                return place;
            }
        } else if let Some(owner) = ctx
            .procs
            .values()
            .filter(|q| {
                q.pid != p.pid
                    && q.cgroup == p.cgroup
                    && (!ctx.classes(q).intersects(
                        Classes::LAUNCHER
                            | Classes::GENERIC
                            | Classes::SHELL
                            | Classes::NOISE
                            | Classes::WORKER,
                    ) || ctx.rules.app(&ctx.facts(q)).is_some())
            })
            .min_by_key(|q| q.pid)
            && let Some(place) = resolve_one(owner.pid, curr, ctx, memo, walking, depth + 1)
            && matches!(place.folder, Folder::Applications | Folder::UserServices)
        {
            return Place {
                instance: ctx.instance(p),
                ..place
            };
        }
    }

    // Any other orphan (`wl-copy` forking into the background) keeps the
    // process group of the app that ran it. Applications only: against any
    // live leader, `bwrap`, `ibus-x11` and `gsd-disk-utility-notify` would
    // fold into gnome-shell, gnome-settings-daemon and systemd rows.
    if let Ok(leader) = u32::try_from(p.pgrp)
        && leader != p.pid
        && curr
            .get(&p.ppid)
            .is_some_and(|q| ctx.classes(q).intersects(Classes::NO_ABSORB))
        && let Some(place) = resolve_one(leader, curr, ctx, memo, walking, depth + 1)
        && place.folder == Folder::Applications
    {
        return Place {
            instance: ctx.instance(p),
            ..place
        };
    }

    user_place(p, ctx)
}

/// Owner for a process in a lying `app-…` scope whose parent is
/// `systemd --user`: an Applications ancestor, else a terminal whose name is
/// a hyphen-prefix of the scope stem.
fn lying_scope_place(
    p: &Process,
    curr: &Procs,
    ctx: &Ctx<'_>,
    memo: &mut PidMap<Place>,
    walking: &mut PidSet,
    depth: usize,
) -> Option<Place> {
    if let Some(owner) = owning_app_ancestor(p.ppid, curr, ctx, memo, walking, depth)
        && owner.folder == Folder::Applications
    {
        return Some(Place {
            instance: ctx.instance(p),
            ..owner
        });
    }
    let unit = ctx.judged(p).unit.as_deref()?;
    let (full, _) = identity::app_scope_names(unit)?;
    let stem = full.to_ascii_lowercase();
    let term = ctx
        .procs
        .values()
        .filter(|q| {
            q.uid == p.uid && ctx.classes(q).intersects(Classes::TERMINAL) && {
                let n = classify::name_ref(q).to_ascii_lowercase();
                stem == n || stem.starts_with(&format!("{n}-"))
            }
        })
        .min_by_key(|q| q.pid)?;
    let place = resolve_one(term.pid, curr, ctx, memo, walking, depth)?;
    if place.folder == Folder::System {
        return None;
    }
    Some(Place {
        instance: ctx.instance(p),
        ..place
    })
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
    let j = ctx.judged(p);
    if identity::is_kernel(p) {
        return Place {
            folder: Folder::System,
            uid: None,
            key: "kernel".to_string(),
            instance: ctx.instance(p),
            member: None,
        };
    }
    let key = service_unit_stem(j).map_or_else(|| name_of(p), str::to_string);
    // A system `.service` whose stem matches a user Applications process
    // (anydesk --service beside the tray) bills to that application rather
    // than a System row. Skip user `.service` peers so session daemons stay
    // on System when the user side is User Services.
    if let Some(peer) = ctx.procs.values().find(|q| {
        q.pid != p.pid
            && identity::in_user_slice(&q.cgroup)
            && !ctx.judged(q).unit_flags.contains(UnitFlags::SERVICE)
            && classify::name_ref(q).eq_ignore_ascii_case(&key)
    }) {
        return Place {
            folder: Folder::Applications,
            uid: Some(peer.uid),
            key,
            instance: ctx.instance(p),
            member: None,
        };
    }
    Place {
        folder: Folder::System,
        uid: None,
        key,
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
        key: match ctx.scope_app(j) {
            Some(app) => app.to_string(),
            None if j.classes.intersects(Classes::GENERIC) => {
                identity::generic_fallback(p, j, ctx.rules)
            }
            None => service_unit_stem(j).map_or_else(|| name_of(p), str::to_string),
        },
        instance: ctx.instance(p),
        member: None,
    }
}

/// Non-lying `.service` unit stem, used as the identity key when display name
/// would otherwise split processes that share one unit.
fn service_unit_stem(j: &Judged) -> Option<&str> {
    let unit = j.unit.as_deref()?;
    if j.unit_flags.contains(UnitFlags::LYING) || !unit.ends_with(".service") {
        return None;
    }
    let stem = identity::unit_stem(unit);
    (!stem.is_empty()).then_some(stem)
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
    let owner = classify::crash_helper_app(
        p,
        ctx.classes(p),
        ctx.rules,
        |q| ctx.classes(q),
        ctx.procs.values(),
    )?;
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

/// A worker with no payload of its own, the zygote fallback.
///
/// `raw_place` still names a crash helper's app from its path, and a
/// container, before the basename. When that answer is the helper's own
/// basename and every other process in the same cgroup — skipping launchers,
/// workers, noise, shells and crash helpers — resolves to one identity, the
/// helper bills there. The launcher that adopted this leaf then bills there
/// too. No such process, or more than one identity, leaves the helper as its
/// own row.
fn worker_leaf_place(
    child: &Process,
    curr: &Procs,
    ctx: &Ctx<'_>,
    memo: &mut PidMap<Place>,
    walking: &mut PidSet,
    depth: usize,
) -> Place {
    let place = raw_place(child, ctx);
    let own = classify::name_ref(child);
    if place.key != own {
        return place;
    }
    let mut siblings: Vec<&Process> = ctx
        .procs
        .values()
        .filter(|q| {
            q.pid != child.pid
                && q.cgroup == child.cgroup
                && !ctx.classes(q).intersects(
                    Classes::LAUNCHER
                        | Classes::WORKER
                        | Classes::NOISE
                        | Classes::SHELL
                        | Classes::CRASH_HELPER,
                )
        })
        .collect();
    if siblings.is_empty() {
        return place;
    }
    siblings.sort_unstable_by_key(|q| q.pid);
    let mut agreed: Option<Place> = None;
    for sib in siblings {
        let Some(next) = resolve_one(sib.pid, curr, ctx, memo, walking, depth + 1) else {
            return place;
        };
        if next.key == own || agreed.as_ref().is_some_and(|prev| prev.key != next.key) {
            return place;
        }
        if agreed.is_none() {
            agreed = Some(next);
        }
    }
    agreed.unwrap_or(place)
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
    memo: &mut PidMap<Place>,
    walking: &mut PidSet,
    depth: usize,
) -> Option<Place> {
    for _ in depth..MAX_WALK {
        let proc = curr.get(&pid)?;
        let classes = ctx.classes(proc);
        // An `app` rule names this ancestor even when it is itself a generic
        // interpreter (an editor's bundled `node`), so it owns the chain
        // rather than being walked past to the user manager.
        let named = ctx.rules.app(&ctx.facts(proc)).is_some();
        if !named
            && classes
                .intersects(Classes::LAUNCHER | Classes::GENERIC | Classes::SHELL | Classes::NOISE)
        {
            pid = proc.ppid;
            continue;
        }
        if !named && !classify::absorbs_generic(classes) {
            return None;
        }
        let place = resolve_one(pid, curr, ctx, memo, walking, depth)?;
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
    memo: &mut PidMap<Place>,
    walking: &mut PidSet,
    depth: usize,
) -> Option<Place> {
    if depth >= MAX_WALK {
        return None;
    }
    let mut kids = Vec::new();
    let children = ctx.children.get(&pid).map_or(&[][..], Vec::as_slice);
    for child in children.iter().filter_map(|c| curr.get(c)) {
        let classes = ctx.classes(child);
        if classes.intersects(Classes::LAUNCHER | Classes::SHELL | Classes::NOISE) {
            if let Some(p) = unique_descendant_ident(child.pid, curr, ctx, memo, walking, depth + 1)
            {
                kids.push(p);
            }
            continue;
        }
        if classes.intersects(Classes::WORKER) {
            if let Some(p) = unique_descendant_ident(child.pid, curr, ctx, memo, walking, depth + 1)
            {
                kids.push(p);
            } else {
                // No non-worker grandchild. A crash helper or a container still
                // names itself through `raw_place`; a helper whose answer is
                // its own basename bills to the one other identity in its cgroup.
                kids.push(worker_leaf_place(child, curr, ctx, memo, walking, depth));
            }
            continue;
        }
        if let Some(place) = resolve_one(child.pid, curr, ctx, memo, walking, depth + 1) {
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
    places: &PidMap<Place>,
    metrics: &PidMap<Metrics>,
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
    let mut users: PidMap<UserNode> = PidMap::default();
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
    places: &PidMap<Place>,
    metrics: &PidMap<Metrics>,
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
            nproc: u32::try_from(mpids.len()).unwrap_or(u32::MAX),
            metrics: sum_metrics(mpids, metrics),
            processes: proc_forest(mpids, curr, metrics),
        })
        .collect();
    IdentNode {
        id: key.to_string(),
        nproc: u32::try_from(pids.len()).unwrap_or(u32::MAX),
        metrics: sum_metrics(pids, metrics),
        instances,
        containers: members,
    }
}

fn sum_metrics(pids: &[u32], metrics: &PidMap<Metrics>) -> Metrics {
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

fn proc_forest(pids: &[u32], curr: &Procs, metrics: &PidMap<Metrics>) -> Vec<ProcNode> {
    let set: PidSet = pids.iter().copied().collect();
    let mut children: PidMap<Vec<u32>> = PidMap::default();
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
    children: &PidMap<Vec<u32>>,
    curr: &Procs,
    metrics: &PidMap<Metrics>,
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
fn descendants(pid: u32, children: &PidMap<Vec<u32>>) -> Vec<u32> {
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
    use super::{Ctx, Folder, PidMap, Process, raw_place};
    use crate::containers::ContainerIndex;
    use crate::rules::Rules;

    fn place_alone(p: Process) -> super::Place {
        let curr = PidMap::from_iter([(p.pid, p)]);
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
            cmdline: vec!["crashhelper".into(), "12766".into()].into(),
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

    const CHAIN: u32 = 10_000;

    /// Pid 1 is `root`, and pids 2 to `CHAIN` each run `link` as the child of
    /// the pid before, all in one process group of one app scope.
    fn chain(root: &str, link: &[&str]) -> PidMap<Process> {
        (1..=CHAIN)
            .map(|pid| {
                let argv: Vec<String> = if pid == 1 {
                    vec![root.into()]
                } else {
                    link.iter().map(|&a| a.into()).collect()
                };
                let p = Process {
                    pid,
                    ppid: pid - 1,
                    pgrp: 1,
                    uid: 1000,
                    comm: argv[0].clone(),
                    exe: Some(format!("/usr/bin/{}", argv[0])),
                    cmdline: argv.into(),
                    cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice".into(),
                    rss_pages: Some(1),
                    ..Process::default()
                };
                (pid, p)
            })
            .collect()
    }

    fn tree_of(curr: &PidMap<Process>) -> crate::types::HostTree {
        let consts = crate::types::HostHeader {
            nproc: 1,
            clk_tck: 100,
            page_size: 4096,
        };
        super::build_tree(
            &PidMap::default(),
            curr,
            std::time::Duration::from_secs(1),
            &consts,
            crate::types::HostTree::default(),
            &ContainerIndex::default(),
            &Rules::builtin(),
        )
    }

    fn placed(tree: &crate::types::HostTree) -> u32 {
        tree.users.iter().map(crate::types::user_nproc).sum()
    }

    /// A shell chain under a terminal walks the payload search down and the
    /// idle-shell fold up; a worker chain walks the parent fold up; a launcher
    /// chain does both. None of them may cost a stack frame per process.
    #[test]
    fn placing_a_deep_shell_chain_does_not_recurse_per_process() {
        let tree = tree_of(&chain("konsole", &["bash"]));
        assert_eq!(placed(&tree), CHAIN);
    }

    #[test]
    fn placing_a_deep_worker_chain_does_not_recurse_per_process() {
        let tree = tree_of(&chain("cursor", &["cursor", "--type=renderer"]));
        assert_eq!(placed(&tree), CHAIN);
    }

    #[test]
    fn placing_a_deep_launcher_chain_does_not_recurse_per_process() {
        let tree = tree_of(&chain("konsole", &["bwrap"]));
        assert_eq!(placed(&tree), CHAIN);
    }

    #[test]
    fn a_deep_process_chain_is_capped_without_losing_a_process() {
        use crate::types::ProcNode;

        fn walk(nodes: &[ProcNode], depth: usize, max: &mut usize, count: &mut u64, rss: &mut u64) {
            for n in nodes {
                *max = (*max).max(depth);
                *count += 1;
                *rss += n.metrics.rss_bytes.unwrap_or(0);
                walk(&n.children, depth + 1, max, count, rss);
            }
        }

        const N: u32 = CHAIN;
        let tree = tree_of(&chain("chain", &["chain"]));

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

    #[test]
    fn a_worker_does_not_fold_onto_a_user_no_absorb_identity() {
        use crate::rules::{LoadedFile, Source};

        let at = |pid, ppid, argv: &[&str]| Process {
            pid,
            ppid,
            uid: 1000,
            comm: argv[0].into(),
            exe: Some(format!("/usr/bin/{}", argv[0])),
            cmdline: argv.iter().map(|&a| a.into()).collect(),
            cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice".into(),
            ..Process::default()
        };
        let curr = PidMap::from_iter([
            (1, at(1, 0, &["mgr"])),
            (2, at(2, 1, &["cursor", "--type=renderer"])),
        ]);
        let containers = ContainerIndex::default();
        let worker_key = |rules: &Rules| {
            super::resolve(&curr, &Ctx::new(&containers, rules, &curr))[&2]
                .key
                .clone()
        };
        assert_eq!(worker_key(&Rules::builtin()), "mgr");

        let mut files = vec![LoadedFile {
            source: Source {
                rank: 0,
                label: "xdg".into(),
            },
            name: "90-mine.json".into(),
            text: r#"{"stage": "class", "rules": [{"id": "mgr", "match": {"name": "mgr"}, "classes": ["no_absorb"]}]}"#.into(),
        }];
        files.extend(crate::rules::builtin_files());
        let rules = Rules::from_files(&files);
        assert!(rules.problems.is_empty(), "{:?}", rules.problems);
        assert_eq!(worker_key(&rules), "cursor");
    }

    #[test]
    fn an_interpreter_under_an_app_named_interpreter_bills_to_that_app() {
        let scope = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-cursor-9.scope";
        let at = |pid, ppid, exe: &str, argv: &[&str], cgroup: &str| Process {
            pid,
            ppid,
            uid: 1000,
            comm: argv[0].into(),
            exe: Some(exe.into()),
            cmdline: argv.iter().map(|&a| a.into()).collect(),
            cgroup: cgroup.into(),
            ..Process::default()
        };
        let curr = PidMap::from_iter([
            (
                1,
                at(
                    1,
                    0,
                    "/usr/lib/systemd/systemd",
                    &["systemd", "--user"],
                    "0::/user.slice/user-1000.slice/user@1000.service/init.scope",
                ),
            ),
            (
                2,
                at(
                    2,
                    1,
                    "/home/u/.config/Cursor/User/globalStorage/agent/node",
                    &["node", "cursor-agent"],
                    scope,
                ),
            ),
            (
                3,
                at(
                    3,
                    2,
                    "/usr/bin/node-24",
                    &["npm", "exec", "shadcn@latest", "mcp"],
                    scope,
                ),
            ),
            (
                4,
                at(
                    4,
                    1,
                    "/usr/bin/bun",
                    &["bun", "apps/server/src/main.ts"],
                    scope,
                ),
            ),
            (
                5,
                Process {
                    pgrp: 5,
                    ..at(5, 2, "/home/u/.local/bin/claude", &["claude"], scope)
                },
            ),
            (
                6,
                Process {
                    pgrp: 5,
                    ..at(6, 1, "/usr/bin/wl-copy", &["wl-copy"], scope)
                },
            ),
        ]);
        let containers = ContainerIndex::default();
        let rules = Rules::builtin();
        let placed = super::resolve(&curr, &Ctx::new(&containers, &rules, &curr));
        assert_eq!(placed[&2].key, "cursor");
        assert_eq!(placed[&3].key, "cursor");
        // Orphaned to the user manager, it still bills to its scope's app.
        assert_eq!(placed[&4].key, "cursor");
        // Not an interpreter, so it bills to its process group's app instead.
        assert_eq!(placed[&6].key, "claude");
    }

    /// Steam's helpers share its scope and nothing else: no name, no path and
    /// no ancestry ties `srt-logger` to `steam`. The scope does, and only
    /// because `steam` itself runs in it, which is what keeps the commands
    /// launched from a browser or a terminal out of that app's row.
    #[test]
    fn an_app_scope_that_names_a_process_in_it_owns_the_rest_of_that_scope() {
        let at = |pid, exe: &str, name: &str, cgroup: &str| Process {
            pid,
            ppid: 1,
            uid: 1000,
            comm: name.into(),
            exe: Some(exe.into()),
            cmdline: vec![name.into()].into(),
            cgroup: cgroup.into(),
            ..Process::default()
        };
        let user = "0::/user.slice/user-1000.slice/user@1000.service/app.slice";
        let steam = format!("{user}/app-gnome-steam-9.scope");
        let browser = format!("{user}/app-com.vivaldi.Vivaldi-8.scope");
        let curr = PidMap::from_iter([
            (
                2,
                at(2, "/home/u/.local/share/Steam/steam", "steam", &steam),
            ),
            (
                3,
                at(
                    3,
                    "/home/u/.local/share/Steam/libexec/srt-logger",
                    "srt-logger",
                    &steam,
                ),
            ),
            (4, at(4, "/usr/bin/vivaldi-bin", "vivaldi-bin", &browser)),
            (5, at(5, "/home/u/.local/bin/claude", "claude", &browser)),
        ]);
        let containers = ContainerIndex::default();
        let rules = Rules::builtin();
        let placed = super::resolve(&curr, &Ctx::new(&containers, &rules, &curr));
        assert_eq!(placed[&3].key, "steam");
        // The scope names a desktop id no process carries, so it names nothing.
        assert_eq!(placed[&4].key, "vivaldi-bin");
        assert_eq!(placed[&5].key, "claude");
    }
}
