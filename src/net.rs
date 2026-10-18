use std::collections::HashMap;
use std::fs;

use crate::containers::ContainerIndex;
use crate::identity::docker_scope_id;
use crate::num::f64_of;
use crate::types::{HostTree, IdentNode, PidMap, ProcNode, Process};

/// Cumulative rx/tx bytes for one network namespace, tagged with the pid they
/// were read through so a restart cannot be mistaken for a counter jump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sample {
    pid: u32,
    rx: u64,
    tx: u64,
}

/// Per-namespace rates keyed the two ways a container reaches the tree: the
/// identity row (a compose or supabase project sums its containers) and the
/// member row under it. `host` is the same `net/dev` read for a pid that is
/// not in a container's own netns — every such process sees that one total.
#[derive(Clone, Debug, Default)]
pub(crate) struct Rates {
    by_ident: HashMap<String, (f64, f64)>,
    by_member: HashMap<String, (f64, f64)>,
    host: Option<(f64, f64)>,
}

#[derive(Debug, Default)]
pub(crate) struct Sampler {
    prev: HashMap<String, Sample>,
    host: Option<Sample>,
}

impl Sampler {
    /// One `net/dev` read per container netns, plus one for a pid that is not
    /// in those namespaces. The file is per network namespace, so every host
    /// process would return the same total; one read is that figure.
    pub(crate) fn tick(
        &mut self,
        idx: &ContainerIndex,
        procs: &PidMap<Process>,
        secs: f64,
    ) -> Rates {
        let mut curr = HashMap::new();
        let mut rates = Rates::default();
        for (id, pid) in netns_pids(idx, procs) {
            let Some(info) = idx.get(&id) else {
                continue;
            };
            let Some(sample) = read_pid(pid) else {
                continue;
            };
            let delta = delta(self.prev.get(&id), sample, secs);
            curr.insert(id, sample);
            let Some((rx, tx)) = delta else {
                continue;
            };
            add(&mut rates.by_ident, &info.ident_key, rx, tx);
            if let Some(name) = &info.member_name {
                add(&mut rates.by_member, name, rx, tx);
            }
        }
        self.prev = curr;
        rates.host = self.tick_host(idx, procs, secs);
        rates
    }

    fn tick_host(
        &mut self,
        idx: &ContainerIndex,
        procs: &PidMap<Process>,
        secs: f64,
    ) -> Option<(f64, f64)> {
        let pid = host_ns_pid(idx, procs)?;
        let sample = read_pid(pid)?;
        let rate = delta(self.host.as_ref(), sample, secs);
        self.host = Some(sample);
        rate
    }
}

/// The lowest pid in each container's own cgroup scope. Scope membership is
/// what makes the pid safe to read: heft also bills the containerd shim,
/// `conmon` and `docker-proxy` to a container, and every one of those runs in
/// the root network namespace. Inspect's `State.Pid` would name the right
/// process but goes stale — `docker restart` keeps the container id, so the
/// inspect cache never refetches it and the rate would stay blank for good.
fn netns_pids(idx: &ContainerIndex, procs: &PidMap<Process>) -> HashMap<String, u32> {
    let mut out: HashMap<String, u32> = HashMap::new();
    for p in procs.values() {
        let Some(info) = docker_scope_id(&p.cgroup).and_then(|id| idx.get(&id)) else {
            continue;
        };
        if !info.own_netns {
            continue;
        }
        let chosen = out.entry(info.id.clone()).or_insert(p.pid);
        *chosen = (*chosen).min(p.pid);
    }
    out
}

/// Lowest pid whose cgroup is not a container with its own netns. That pid's
/// `net/dev` is the host namespace; pids inside a namespaced container would
/// return that container's total instead.
fn host_ns_pid(idx: &ContainerIndex, procs: &PidMap<Process>) -> Option<u32> {
    procs
        .values()
        .filter(|p| {
            docker_scope_id(&p.cgroup)
                .and_then(|id| idx.get(&id))
                .is_none_or(|info| !info.own_netns)
        })
        .map(|p| p.pid)
        .min()
}

impl Rates {
    /// Container rows keep the per-container netns figure. Process rows that
    /// are not under a Containers folder carry the host-namespace read; a
    /// process inside a container netns must not also hold that total, or a
    /// parent that summed both would count the bytes twice. `accumulate` still
    /// refuses the sum, so folder, User and Host stay blank.
    pub(crate) fn apply(&self, tree: &mut HostTree) {
        for user in &mut tree.users {
            self.bill(&mut user.containers);
            self.bill_host_folder(&mut user.applications);
            self.bill_host_folder(&mut user.user_services);
        }
        self.bill(&mut tree.containers);
        self.bill_host_folder(&mut tree.system);
    }

    fn bill_host_folder(&self, idents: &mut [IdentNode]) {
        let rate = self.host.as_ref();
        for ident in idents {
            for inst in &mut ident.instances {
                bill_procs(&mut inst.processes, rate);
            }
        }
    }

    fn bill(&self, idents: &mut [IdentNode]) {
        for ident in idents {
            set(&mut ident.metrics, self.by_ident.get(&ident.id));
            for member in &mut ident.containers {
                set(&mut member.metrics, self.by_member.get(&member.id));
            }
        }
    }
}

fn set(m: &mut crate::types::Metrics, rate: Option<&(f64, f64)>) {
    m.net_rx_bps = rate.map(|r| r.0);
    m.net_tx_bps = rate.map(|r| r.1);
}

fn bill_procs(procs: &mut [ProcNode], rate: Option<&(f64, f64)>) {
    for p in procs {
        set(&mut p.metrics, rate);
        bill_procs(&mut p.children, rate);
    }
}

fn add(map: &mut HashMap<String, (f64, f64)>, key: &str, rx: f64, tx: f64) {
    let e = map.entry(key.to_string()).or_insert((0.0, 0.0));
    e.0 += rx;
    e.1 += tx;
}

/// `None` discards the interval instead of publishing a bogus rate. Counters
/// are cumulative per namespace, and a restarted container keeps its id while
/// getting a new pid and a fresh namespace, so the delta would run negative.
fn delta(prev: Option<&Sample>, cur: Sample, secs: f64) -> Option<(f64, f64)> {
    let p = prev?;
    if p.pid != cur.pid || cur.rx < p.rx || cur.tx < p.tx {
        return None;
    }
    Some((f64_of(cur.rx - p.rx) / secs, f64_of(cur.tx - p.tx) / secs))
}

fn read_pid(pid: u32) -> Option<Sample> {
    let text = fs::read_to_string(format!("{}/proc/{pid}/net/dev", crate::root::prefix())).ok()?;
    let (rx, tx) = parse_dev(&text);
    Some(Sample { pid, rx, tx })
}

/// Sums every interface but `lo`, which carries traffic that never left the
/// namespace and can outweigh the real interfaces. The host-side veth is not
/// read at all — its direction is inverted and its name is not derivable without `/proc/<pid>/ns/net`, which is
/// EACCES for a root-owned pid.
fn parse_dev(text: &str) -> (u64, u64) {
    let mut rx = 0;
    let mut tx = 0;
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        // rx bytes is the first field, tx bytes the ninth.
        let mut fields = rest.split_whitespace().map(|f| f.parse::<u64>().ok());
        if let (Some(Some(r)), Some(Some(t))) = (fields.next(), fields.nth(7)) {
            rx += r;
            tx += t;
        }
    }
    (rx, tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containers::{HostConfig, Inspect, ListItem};
    use crate::types::{InstanceNode, MemberContainer, Metrics, ProcNode, UserNode};

    const DEV: &str = "Inter-|   Receive                    |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 47332844   1000    0    0    0     0          0         0 47332844   1000    0    0    0     0       0          0
  eth0: 26295295    500    0    0    0     0          0         0  1048576    200    0    0    0     0       0          0
  eth1:     1024      1    0    0    0     0          0         0      512      1    0    0    0     0       0          0
";

    #[test]
    fn loopback_excluded_and_interfaces_summed() {
        assert_eq!(parse_dev(DEV), (26_295_295 + 1024, 1_048_576 + 512));
        assert_eq!(parse_dev("Inter-|  Receive | Transmit\n"), (0, 0));
    }

    #[test]
    fn restart_and_regression_discard_the_interval() {
        let a = Sample {
            pid: 10,
            rx: 100,
            tx: 200,
        };
        assert_eq!(delta(None, a, 1.0), None);
        let grown = Sample {
            rx: 300,
            tx: 400,
            ..a
        };
        assert_eq!(delta(Some(&a), grown, 2.0), Some((100.0, 100.0)));
        // Same id, new pid: a restarted container's namespace starts at zero.
        let restarted = Sample {
            pid: 11,
            rx: 5,
            tx: 5,
        };
        assert_eq!(delta(Some(&a), restarted, 1.0), None);
        assert_eq!(delta(Some(&a), Sample { rx: 1, ..a }, 1.0), None);
    }

    fn index(mode: &str) -> ContainerIndex {
        let id = "0123456789abcdef".to_string();
        let inspects = HashMap::from([(
            id.clone(),
            Inspect {
                host_config: Some(HostConfig {
                    network_mode: Some(mode.into()),
                }),
                ..Inspect::default()
            },
        )]);
        ContainerIndex::from_list(
            &[ListItem {
                id,
                names: vec!["/probe".into()],
                labels: None,
                state: Some("running".into()),
            }],
            &inspects,
            |_| None,
            &crate::rules::Rules::builtin(),
        )
    }

    fn proc_in(pid: u32, cgroup: &str) -> Process {
        Process {
            pid,
            cgroup: cgroup.into(),
            ..Process::default()
        }
    }

    #[test]
    fn only_a_scope_member_of_a_namespaced_container_is_read() {
        let scope = "0::/system.slice/docker-0123456789abcdef.scope";
        let procs = PidMap::from_iter([
            (40, proc_in(40, scope)),
            (12, proc_in(12, scope)),
            // Billed to the container but running in the root namespace: the
            // shim's cgroup is docker.service, not the container scope.
            (9, proc_in(9, "0::/system.slice/docker.service")),
        ]);
        assert_eq!(
            netns_pids(&index("bridge"), &procs),
            HashMap::from([("0123456789abcdef".to_string(), 12)])
        );
        // --network=host shares the root namespace; nothing is read.
        assert!(netns_pids(&index("host"), &procs).is_empty());
        assert_eq!(host_ns_pid(&index("bridge"), &procs), Some(9));
        assert_eq!(host_ns_pid(&index("host"), &procs), Some(9));
    }

    fn ident(id: &str, member: &str) -> IdentNode {
        IdentNode {
            id: id.into(),
            nproc: 1,
            containers: vec![MemberContainer {
                id: member.into(),
                nproc: 1,
                ..MemberContainer::default()
            }],
            ..IdentNode::default()
        }
    }

    #[test]
    fn only_container_rows_are_billed() {
        let mut rates = Rates::default();
        rates.by_ident.insert("supabase:demo".into(), (10.0, 20.0));
        rates
            .by_member
            .insert("supabase_db_demo".into(), (10.0, 20.0));
        let mut tree = HostTree {
            users: vec![UserNode {
                uid: 1000,
                name: "u".into(),
                applications: vec![ident("supabase:demo", "supabase_db_demo")],
                containers: vec![ident("supabase:demo", "supabase_db_demo")],
                ..UserNode::default()
            }],
            ..HostTree::default()
        };
        rates.apply(&mut tree);
        let user = &tree.users[0];
        let c = &user.containers[0];
        assert_eq!(c.metrics.net_rx_bps, Some(10.0));
        assert_eq!(c.containers[0].metrics.net_tx_bps, Some(20.0));
        // An identity that is not under Containers never carries a rate, even
        // when it happens to share the key.
        let app = &user.applications[0];
        assert_eq!(app.metrics.net_rx_bps, None);
        assert_eq!(app.containers[0].metrics.net_rx_bps, None);
    }

    fn proc_row(pid: u32) -> ProcNode {
        ProcNode {
            pid,
            ..ProcNode::default()
        }
    }

    /// Host-namespace traffic lands on a process that is not under Containers.
    /// A process billed to a container row stays blank so the namespace is
    /// not counted twice; EACCES (no host sample) stays blank, not zero.
    #[test]
    fn a_host_namespace_process_carries_the_host_rate() {
        let mut rates = Rates {
            host: Some((3.0, 4.0)),
            ..Rates::default()
        };
        rates.by_ident.insert("ctr".into(), (10.0, 20.0));
        let mut tree = HostTree {
            users: vec![UserNode {
                applications: vec![IdentNode {
                    id: "app".into(),
                    instances: vec![InstanceNode {
                        processes: vec![proc_row(1)],
                        ..InstanceNode::default()
                    }],
                    ..IdentNode::default()
                }],
                containers: vec![IdentNode {
                    id: "ctr".into(),
                    instances: vec![InstanceNode {
                        processes: vec![proc_row(12)],
                        ..InstanceNode::default()
                    }],
                    ..IdentNode::default()
                }],
                ..UserNode::default()
            }],
            ..HostTree::default()
        };
        rates.apply(&mut tree);
        let user = &tree.users[0];
        assert_eq!(
            user.applications[0].instances[0].processes[0]
                .metrics
                .net_rx_bps,
            Some(3.0)
        );
        assert_eq!(
            user.applications[0].instances[0].processes[0]
                .metrics
                .net_tx_bps,
            Some(4.0)
        );
        assert_eq!(user.containers[0].metrics.net_rx_bps, Some(10.0));
        assert_eq!(
            user.containers[0].instances[0].processes[0]
                .metrics
                .net_rx_bps,
            None
        );
        rates.host = None;
        rates.apply(&mut tree);
        assert_eq!(
            tree.users[0].applications[0].instances[0].processes[0]
                .metrics
                .net_rx_bps,
            None
        );
    }

    #[test]
    fn nothing_above_a_container_accumulates_a_rate() {
        let mut m = Metrics {
            net_rx_bps: Some(1.0),
            net_tx_bps: Some(2.0),
            ..Metrics::default()
        };
        let mut sum = Metrics::default();
        sum.accumulate(&m);
        assert_eq!(sum.net_rx_bps, None);
        assert_eq!(sum.net_tx_bps, None);
        m.accumulate(&Metrics::default());
        assert_eq!(m.net_rx_bps, Some(1.0));
    }
}
