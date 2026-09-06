use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::classify::{self, name_of};
use crate::containers::{self, ContainerIndex};
use crate::cpu::process_metrics;
use crate::identity::{self, docker_scope_id};
use crate::proc;
use crate::types::{
    Folder, HostHeader, HostTree, IdentNode, InstanceNode, MemberContainer, Metrics, ProcNode,
    Process, UserNode,
};

#[derive(Clone)]
struct Place {
    folder: Folder,
    uid: Option<u32>,
    key: String,
    title: String,
    instance: String,
    member: Option<String>,
}
#[must_use]
pub fn build_tree(
    prev: &HashMap<u32, Process>,
    curr: &HashMap<u32, Process>,
    elapsed: Duration,
    header: &HostHeader,
    containers: &ContainerIndex,
) -> HostTree {
    let places = resolve(curr, containers);
    let metrics = metrics_map(prev, curr, elapsed, header);
    assemble(curr, &places, &metrics, header)
}

fn metrics_map(
    prev: &HashMap<u32, Process>,
    curr: &HashMap<u32, Process>,
    elapsed: Duration,
    header: &HostHeader,
) -> HashMap<u32, Metrics> {
    curr.iter()
        .map(|(pid, p)| {
            (
                *pid,
                process_metrics(
                    prev.get(pid),
                    p,
                    elapsed,
                    header.nproc,
                    header.clk_tck,
                    header.page_size,
                ),
            )
        })
        .collect()
}

fn resolve(curr: &HashMap<u32, Process>, containers: &ContainerIndex) -> HashMap<u32, Place> {
    let mut memo: HashMap<u32, Place> = HashMap::new();
    let mut walking = HashSet::new();
    for pid in curr.keys().copied() {
        resolve_one(pid, curr, containers, &mut memo, &mut walking);
    }
    memo
}

fn resolve_one(
    pid: u32,
    curr: &HashMap<u32, Process>,
    containers: &ContainerIndex,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Option<Place> {
    if let Some(p) = memo.get(&pid) {
        return Some(p.clone());
    }
    let p = curr.get(&pid)?;
    if !walking.insert(pid) {
        return Some(raw_place(p, containers));
    }
    let place = compute_place(p, curr, containers, memo, walking);
    walking.remove(&pid);
    memo.insert(pid, place.clone());
    Some(place)
}

fn compute_place(
    p: &Process,
    curr: &HashMap<u32, Process>,
    containers: &ContainerIndex,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Place {
    if let Some(place) = container_place(p, containers) {
        return place;
    }
    if identity::is_kernel(p) {
        return system_place(p);
    }
    if identity::in_system_slice(&p.cgroup) && !identity::in_user_slice(&p.cgroup) {
        return system_place(p);
    }

    if let Some(place) = session_plumbing_place(p) {
        return place;
    }

    if let Some(place) = crash_helper_place(p) {
        return place;
    }

    if classify::is_worker(p)
        && let Some(parent) = resolve_one(p.ppid, curr, containers, memo, walking)
        && parent.folder != Folder::System
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
        && let Some(parent) = resolve_one(p.ppid, curr, containers, memo, walking)
        && parent.folder != Folder::System
    {
        return Place {
            instance: identity::instance_key(p, None),
            ..parent
        };
    }

    if classify::is_foldable_helper(p) {
        if let Some(payload) = unique_descendant_ident(p.pid, curr, containers, memo, walking) {
            return Place {
                instance: identity::instance_key(p, None),
                ..payload
            };
        }
        if classify::is_launcher(p) {
            if let Some(hint) = classify::launcher_payload_hint(p) {
                let mut place = user_place(p);
                place.key.clone_from(&hint);
                place.title = hint;
                return place;
            }
            if let Some(parent) = resolve_one(p.ppid, curr, containers, memo, walking)
                && parent.folder != Folder::System
                && !classify::is_launcher_name(&parent.key)
            {
                return Place {
                    instance: identity::instance_key(p, None),
                    ..parent
                };
            }
        }
    }

    if classify::is_generic(p)
        && let Some(owner) = owning_app_ancestor(p.ppid, curr, containers, memo, walking)
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
            title: info.ident_title.clone(),
            instance: identity::instance_key(p, Some(&info.id)),
            member: info.member_name.clone(),
        });
    }
    let scope = docker_scope_id(&p.cgroup);
    if let Some(id) = scope.clone().or_else(|| containers::helper_id(p)) {
        let short: String = id.chars().take(12).collect();
        let title = format!("docker-{short}");
        return Some(Place {
            folder: Folder::Containers,
            // Only a cgroup id is known on this path, so there is no name or
            // label to attribute an owner from; lookup_process does that.
            uid: None,
            key: title.clone(),
            title,
            instance: identity::instance_key(p, scope.as_deref()),
            member: None,
        });
    }
    None
}

fn system_place(p: &Process) -> Place {
    let title = if identity::is_kernel(p) {
        "kernel".to_string()
    } else {
        name_of(p)
    };
    Place {
        folder: Folder::System,
        uid: None,
        key: title.clone(),
        title,
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
    let title = if classify::is_generic(p) {
        identity::generic_fallback(p, unit.as_deref())
    } else {
        name_of(p)
    };
    Place {
        folder,
        uid: Some(p.uid),
        key: title.clone(),
        title,
        instance: identity::instance_key(p, None),
        member: None,
    }
}

fn session_plumbing_place(p: &Process) -> Option<Place> {
    let (key, title) = classify::session_helper_ident(p)?;
    Some(Place {
        folder: Folder::UserServices,
        uid: Some(p.uid),
        key,
        title,
        instance: identity::instance_key(p, None),
        member: None,
    })
}

fn crash_helper_place(p: &Process) -> Option<Place> {
    let owner = classify::crash_helper_app(p)?;
    Some(Place {
        folder: Folder::Applications,
        uid: Some(p.uid),
        key: owner.clone(),
        title: owner,
        instance: identity::instance_key(p, None),
        member: None,
    })
}

fn raw_place(p: &Process, containers: &ContainerIndex) -> Place {
    container_place(p, containers).unwrap_or_else(|| {
        if identity::is_kernel(p)
            || (identity::in_system_slice(&p.cgroup) && !identity::in_user_slice(&p.cgroup))
        {
            system_place(p)
        } else if let Some(place) = session_plumbing_place(p) {
            place
        } else {
            user_place(p)
        }
    })
}

fn owning_app_ancestor(
    mut pid: u32,
    curr: &HashMap<u32, Process>,
    containers: &ContainerIndex,
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
        let place = resolve_one(pid, curr, containers, memo, walking)?;
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
    containers: &ContainerIndex,
    memo: &mut HashMap<u32, Place>,
    walking: &mut HashSet<u32>,
) -> Option<Place> {
    let mut kids = Vec::new();
    for child in curr.values().filter(|c| c.ppid == pid) {
        if classify::is_foldable_helper(child) || classify::is_session_noise(child) {
            if let Some(p) = unique_descendant_ident(child.pid, curr, containers, memo, walking) {
                kids.push(p);
            }
            continue;
        }
        if classify::is_worker(child) {
            if let Some(p) = unique_descendant_ident(child.pid, curr, containers, memo, walking) {
                kids.push(p);
            } else {
                // Zygote-only sandbox: no non-worker grandchild to resolve.
                kids.push(user_place(child));
            }
            continue;
        }
        if let Some(place) = resolve_one(child.pid, curr, containers, memo, walking) {
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
    header: &HostHeader,
) -> HostTree {
    #[derive(Default)]
    struct Bucket {
        title: String,
        pids: Vec<u32>,
        members: HashMap<String, Vec<u32>>,
    }

    let mut buckets: HashMap<(Folder, Option<u32>, String), Bucket> = HashMap::new();
    for (pid, place) in places {
        let b = buckets
            .entry((place.folder, place.uid, place.key.clone()))
            .or_insert_with(|| Bucket {
                title: place.title.clone(),
                ..Bucket::default()
            });
        b.pids.push(*pid);
        if let Some(m) = &place.member {
            b.members.entry(m.clone()).or_default().push(*pid);
        }
    }

    let mut users: HashMap<u32, UserNode> = HashMap::new();
    let mut host_containers = Vec::new();
    let mut system = Vec::new();

    for ((folder, uid, key), bucket) in buckets {
        let node = ident_node(
            &key,
            &bucket.title,
            &bucket.pids,
            &bucket.members,
            curr,
            places,
            metrics,
        );
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
    for u in &mut user_list {
        sort_idents(&mut u.applications);
        sort_idents(&mut u.user_services);
        sort_idents(&mut u.containers);
    }
    sort_idents(&mut host_containers);
    sort_idents(&mut system);

    HostTree {
        users: user_list,
        containers: host_containers,
        system,
        ..HostTree::from(header)
    }
}

fn ident_node(
    key: &str,
    title: &str,
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
    let mut instances: Vec<InstanceNode> = by_inst
        .into_iter()
        .map(|(k, inst_pids)| InstanceNode {
            nproc: u32::try_from(inst_pids.len()).unwrap_or(u32::MAX),
            metrics: sum_metrics(&inst_pids, metrics),
            processes: proc_forest(&inst_pids, curr, metrics),
            key: k,
        })
        .collect();
    instances.sort_by(|a, b| {
        b.metrics
            .cpu_machine_pct
            .total_cmp(&a.metrics.cpu_machine_pct)
            .then(a.key.cmp(&b.key))
    });
    let mut members: Vec<MemberContainer> = members_map
        .iter()
        .map(|(name, mpids)| MemberContainer {
            id: name.clone(),
            title: name.clone(),
            nproc: u32::try_from(mpids.len()).unwrap_or(u32::MAX),
            metrics: sum_metrics(mpids, metrics),
            processes: proc_forest(mpids, curr, metrics),
        })
        .collect();
    members.sort_by(|a, b| a.title.cmp(&b.title));
    IdentNode {
        id: key.to_string(),
        title: title.to_string(),
        nproc: u32::try_from(pids.len()).unwrap_or(u32::MAX),
        metrics: sum_metrics(pids, metrics),
        instances,
        containers: members,
    }
}

fn sort_idents(v: &mut [IdentNode]) {
    v.sort_by(|a, b| {
        b.metrics
            .pss_bytes
            .unwrap_or(0)
            .cmp(&a.metrics.pss_bytes.unwrap_or(0))
            .then(a.title.cmp(&b.title))
    });
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
    let mut kids = children.get(&pid).cloned().unwrap_or_default();
    kids.sort_unstable();
    ProcNode {
        pid,
        name,
        metrics: metrics.get(&pid).cloned().unwrap_or_default(),
        children: kids
            .into_iter()
            .map(|c| proc_node(c, children, curr, metrics))
            .collect(),
    }
}
