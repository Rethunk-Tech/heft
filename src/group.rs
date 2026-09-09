use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::classify::{self, name_of};
use crate::config::Overrides;
use crate::containers::{self, ContainerIndex};
use crate::cpu::process_metrics;
use crate::identity::{self, docker_scope_id};
use crate::proc;
use crate::types::{
    Folder, HostHeader, HostTree, IdentNode, InstanceNode, MemberContainer, Metrics, ProcNode,
    Process, UserNode,
};

/// The two read-only inputs every placement rule needs, bundled so the
/// recursive walk keeps one parameter instead of two.
struct Ctx<'a> {
    containers: &'a ContainerIndex,
    ov: &'a Overrides,
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
    prev: &HashMap<u32, Process>,
    curr: &HashMap<u32, Process>,
    elapsed: Duration,
    consts: &HostHeader,
    header: HostTree,
    containers: &ContainerIndex,
    ov: &Overrides,
) -> HostTree {
    let places = resolve(curr, &Ctx { containers, ov });
    let metrics = metrics_map(prev, curr, elapsed, consts);
    let mut tree = assemble(curr, &places, &metrics, header);
    crate::once::sort_default(&mut tree);
    tree
}

fn metrics_map(
    prev: &HashMap<u32, Process>,
    curr: &HashMap<u32, Process>,
    elapsed: Duration,
    consts: &HostHeader,
) -> HashMap<u32, Metrics> {
    curr.iter()
        .map(|(pid, p)| (*pid, process_metrics(prev.get(pid), p, elapsed, consts)))
        .collect()
}

fn resolve(curr: &HashMap<u32, Process>, ctx: &Ctx<'_>) -> HashMap<u32, Place> {
    let mut memo: HashMap<u32, Place> = HashMap::new();
    let mut walking = HashSet::new();
    for pid in curr.keys().copied() {
        resolve_one(pid, curr, ctx, &mut memo, &mut walking);
    }
    memo
}

fn resolve_one(
    pid: u32,
    curr: &HashMap<u32, Process>,
    ctx: &Ctx<'_>,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Option<Place> {
    if let Some(p) = memo.get(&pid) {
        return Some(p.clone());
    }
    let p = curr.get(&pid)?;
    if !walking.insert(pid) {
        return Some(override_place(ctx.ov, raw_place(p, ctx)));
    }
    let place = override_place(ctx.ov, compute_place(p, curr, ctx, memo, walking));
    walking.remove(&pid);
    memo.insert(pid, place.clone());
    Some(place)
}

fn compute_place(
    p: &Process,
    curr: &HashMap<u32, Process>,
    ctx: &Ctx<'_>,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Place {
    if let Some(place) = direct_place(p, ctx) {
        return place;
    }

    if classify::is_worker(p)
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
            instance: identity::instance_key(p, None),
            ..parent
        };
    }

    // Pipe helpers under a launcher (flatpak bwrap `cat`) or an app (vivaldi).
    // Immediate parent only — never a sibling identity under a mixed shell.
    if classify::is_session_noise(p)
        && let Some(parent) = resolve_one(p.ppid, curr, ctx, memo, walking)
        && parent.folder != Folder::System
    {
        return Place {
            instance: identity::instance_key(p, None),
            ..parent
        };
    }

    if classify::is_foldable_helper(p) {
        if let Some(payload) = unique_descendant_ident(p.pid, curr, ctx, memo, walking) {
            return Place {
                instance: identity::instance_key(p, None),
                ..payload
            };
        }
        if classify::is_launcher(p) {
            if let Some(hint) = classify::launcher_payload_hint(p) {
                let mut place = user_place(p);
                place.key = hint;
                return place;
            }
            if let Some(parent) = resolve_one(p.ppid, curr, ctx, memo, walking)
                && parent.folder != Folder::System
                && !classify::is_launcher_name(&parent.key)
            {
                return Place {
                    instance: identity::instance_key(p, None),
                    ..parent
                };
            }
        }
        // Idle interactive shell: not an application. The resolved parent
        // identity is the terminal that owns the tty. A unique payload child
        // already returned above, same walk as a launcher.
        if classify::is_interactive_shell(p)
            && let Some(parent) = resolve_one(p.ppid, curr, ctx, memo, walking)
            && parent.folder != Folder::System
            && classify::is_terminal_name(&parent.key)
        {
            return Place {
                instance: identity::instance_key(p, None),
                ..parent
            };
        }
    }

    if classify::is_generic(p)
        && let Some(owner) = owning_app_ancestor(p.ppid, curr, ctx, memo, walking)
    {
        return Place {
            instance: identity::instance_key(p, None),
            ..owner
        };
    }

    user_place(p)
}

fn container_place(p: &Process, containers: &ContainerIndex) -> Option<Place> {
    if let Some(info) = containers.lookup_process(p) {
        return Some(Place {
            folder: Folder::Containers,
            uid: info.owner_uid,
            key: info.ident_key.clone(),
            instance: identity::instance_key(p, Some(&info.id)),
            member: info.member_name.clone(),
        });
    }
    let scope = docker_scope_id(&p.cgroup);
    if let Some(id) = scope.clone().or_else(|| containers::helper_id(p)) {
        return Some(Place {
            folder: Folder::Containers,
            // Only a cgroup id is known on this path, so there is no name or
            // label to attribute an owner from; lookup_process does that.
            uid: None,
            key: containers::docker_title(&id),
            instance: identity::instance_key(p, scope.as_deref()),
            member: None,
        });
    }
    None
}

fn system_place(p: &Process) -> Place {
    Place {
        folder: Folder::System,
        uid: None,
        key: if identity::is_kernel(p) {
            "kernel".to_string()
        } else {
            name_of(p)
        },
        instance: identity::instance_key(p, None),
        member: None,
    }
}

fn user_place(p: &Process) -> Place {
    let unit = identity::user_unit(&p.cgroup);
    let folder = if classify::is_compositor(p)
        || unit
            .as_deref()
            .is_some_and(|u| !identity::lying_unit(u) && identity::is_user_service_unit(u))
    {
        Folder::UserServices
    } else {
        Folder::Applications
    };
    Place {
        folder,
        uid: Some(p.uid),
        key: if classify::is_generic(p) {
            identity::generic_fallback(p, unit.as_deref())
        } else {
            name_of(p)
        },
        instance: identity::instance_key(p, None),
        member: None,
    }
}

fn session_plumbing_place(p: &Process) -> Option<Place> {
    let key = classify::session_helper_ident(p)?;
    Some(Place {
        folder: Folder::UserServices,
        uid: Some(p.uid),
        key: key.to_string(),
        instance: identity::instance_key(p, None),
        member: None,
    })
}

fn crash_helper_place(p: &Process) -> Option<Place> {
    let owner = classify::crash_helper_app(p)?;
    Some(Place {
        folder: Folder::Applications,
        uid: Some(p.uid),
        key: owner,
        instance: identity::instance_key(p, None),
        member: None,
    })
}

/// The bucket rules that need no ancestor walk, so the cycle-breaking path can
/// answer with the same verdict `compute_place` would give instead of a subset.
fn direct_place(p: &Process, ctx: &Ctx<'_>) -> Option<Place> {
    if let Some(place) = container_place(p, ctx.containers) {
        return Some(place);
    }
    if identity::is_kernel(p)
        || (identity::in_system_slice(&p.cgroup) && !identity::in_user_slice(&p.cgroup))
    {
        return Some(system_place(p));
    }
    session_plumbing_place(p).or_else(|| crash_helper_place(p))
}

fn raw_place(p: &Process, ctx: &Ctx<'_>) -> Place {
    direct_place(p, ctx).unwrap_or_else(|| user_place(p))
}

/// Apply the user's overrides to a finished placement. They run last, so a pin
/// beats every built-in table; they run only on the two user-owned folders, so
/// no override can pull a container or a kernel thread out of where it belongs.
fn override_place(ov: &Overrides, place: Place) -> Place {
    if ov.no_placement_overrides()
        || !matches!(place.folder, Folder::Applications | Folder::UserServices)
    {
        return place;
    }
    // Fold first: the pin then names the row the user is left looking at.
    let key = ov.fold_key(&place.key).map_or(place.key, str::to_string);
    let folder = ov.folder_for(&key).unwrap_or(place.folder);
    Place {
        folder,
        key,
        ..place
    }
}

fn owning_app_ancestor(
    mut pid: u32,
    curr: &HashMap<u32, Process>,
    ctx: &Ctx<'_>,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Option<Place> {
    for _ in 0..32 {
        let proc = curr.get(&pid)?;
        if classify::is_launcher(proc)
            || classify::is_generic(proc)
            || classify::is_foldable_helper(proc)
            || classify::is_session_noise(proc)
        {
            pid = proc.ppid;
            continue;
        }
        if !classify::absorbs_generic(proc) {
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
    curr: &HashMap<u32, Process>,
    ctx: &Ctx<'_>,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Option<Place> {
    let mut kids = Vec::new();
    for child in curr.values().filter(|c| c.ppid == pid) {
        if classify::is_foldable_helper(child) || classify::is_session_noise(child) {
            if let Some(p) = unique_descendant_ident(child.pid, curr, ctx, memo, walking) {
                kids.push(p);
            }
            continue;
        }
        if classify::is_worker(child) {
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
    curr: &HashMap<u32, Process>,
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
                    name: proc::username(uid),
                    applications: Vec::new(),
                    user_services: Vec::new(),
                    containers: Vec::new(),
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
    curr: &HashMap<u32, Process>,
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

fn proc_forest(
    pids: &[u32],
    curr: &HashMap<u32, Process>,
    metrics: &HashMap<u32, Metrics>,
) -> Vec<ProcNode> {
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
        .map(|pid| proc_node(pid, &children, curr, metrics))
        .collect()
}

fn proc_node(
    pid: u32,
    children: &HashMap<u32, Vec<u32>>,
    curr: &HashMap<u32, Process>,
    metrics: &HashMap<u32, Metrics>,
) -> ProcNode {
    let name = curr.get(&pid).map_or_else(|| pid.to_string(), name_of);
    let cmdline = curr
        .get(&pid)
        .map(|p| p.cmdline.join(" "))
        .unwrap_or_default();
    let mut kids = children.get(&pid).cloned().unwrap_or_default();
    kids.sort_unstable();
    ProcNode {
        pid,
        name,
        cmdline,
        metrics: metrics.get(&pid).cloned().unwrap_or_default(),
        children: kids
            .into_iter()
            .map(|c| proc_node(c, children, curr, metrics))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Ctx, Folder, Process, raw_place};
    use crate::config::Overrides;
    use crate::containers::ContainerIndex;

    #[test]
    fn cycle_path_still_bills_a_crash_helper_to_its_app() {
        let place = raw_place(
            &Process {
                comm: "crashhelper".into(),
                exe: Some("/usr/lib64/firefox/crashhelper".into()),
                cmdline: vec!["crashhelper".into(), "12766".into()],
                uid: 1000,
                cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice".into(),
                ..Process::default()
            },
            &Ctx {
                containers: &ContainerIndex::default(),
                ov: &Overrides::default(),
            },
        );
        assert_eq!(place.folder, Folder::Applications);
        assert_eq!(place.key, "firefox");
    }
}
