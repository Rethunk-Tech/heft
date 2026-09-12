//! Pressure stall information: how much of an interval a cgroup's tasks spent
//! blocked waiting on a resource rather than running.
//!
//! It is the one thing in heft that separates *slow* from *busy*. `%core` says
//! a row used the CPU, `PSS` says it holds memory, disk R/W says bytes moved —
//! none of them can say a row was stalled, doing nothing, waiting on reclaim
//! or on the disk. The kernel already accounts exactly that, per cgroup, in a
//! world-readable file, so heft reads it without root the way it reads
//! everything else.
//!
//! `some`, not `full`: `some` is "at least one task in this cgroup was
//! stalled", which is what a reader means by "was this thing waiting". `full`
//! is "every task was", which on the single-process cgroups that make up most
//! of a tree is the same number printed twice.
//!
//! Δ`total` over wall clock, not the kernel's `avg10`, for the table: every
//! other rate in a row (`%core`, disk, NETNS) is an interval delta, and a
//! 10-second average sitting among them would damp a spike and then outlive it
//! by ten ticks. The header is the exception and does use `avg10`, because a
//! machine-wide trend is the one place smoothing helps.

use std::collections::HashMap;
use std::fs;

use crate::types::{HostTree, IdentNode, Metrics, Process};

/// Cumulative `some` stall microseconds for one cgroup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    cpu: u64,
    io: u64,
    mem: u64,
}

/// Percent of the interval spent stalled.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Stall {
    cpu: f64,
    io: f64,
    mem: f64,
}

#[derive(Debug, Default)]
pub(crate) struct Sampler {
    prev: HashMap<String, Totals>,
}

impl Sampler {
    /// Reads three pressure files for every distinct non-root cgroup the walk
    /// saw, and returns this interval's rates. Split from `apply` the way
    /// `net::Sampler` is, so `Sampler::prime` can take a baseline before any
    /// tree exists — without one the first published sample would have no
    /// previous total to subtract and the whole column would be blank.
    ///
    /// Measured on a host with 139 distinct cgroups, ten interleaved runs of
    /// `--once` each: 0.31s without pressure sampling, 0.32s with. That is
    /// about 5 ms a tick, for three pressure files per cgroup plus the
    /// `cgroup.procs` reads the solo-process rule needs — never enough to
    /// justify a cadence of its own the way `--pss-interval` was.
    pub(crate) fn tick(&mut self, procs: &HashMap<u32, Process>, secs: f64) -> Stalls {
        let mut curr = HashMap::new();
        let mut rates = HashMap::new();
        for p in procs.values() {
            let Some(path) = cgroup_path(&p.cgroup) else {
                continue;
            };
            if curr.contains_key(path) {
                continue;
            }
            let Some(now) = read_totals(path) else {
                continue;
            };
            if let Some(stall) = delta(self.prev.get(path), now, secs) {
                rates.insert(path.to_string(), stall);
            }
            curr.insert(path.to_string(), now);
        }
        self.prev = curr;
        Stalls(rates)
    }
}

/// This interval's stall percentages, by cgroup path.
#[derive(Clone, Debug, Default)]
pub(crate) struct Stalls(HashMap<String, Stall>);

impl Stalls {
    pub(crate) fn apply(&self, tree: &mut HostTree, procs: &HashMap<u32, Process>) {
        let mut t = Apply {
            rates: &self.0,
            solo: HashMap::new(),
            procs,
        };
        for user in &mut tree.users {
            t.bill(&mut user.applications);
            t.bill(&mut user.user_services);
            t.bill(&mut user.containers);
        }
        t.bill(&mut tree.containers);
        t.bill(&mut tree.system);
    }
}

struct Apply<'a> {
    rates: &'a HashMap<String, Stall>,
    /// Whether a cgroup holds exactly one process, memoised: several process
    /// rows under one identity ask about the same cgroup.
    solo: HashMap<String, Option<u32>>,
    procs: &'a HashMap<u32, Process>,
}

impl Apply<'_> {
    fn bill(&mut self, idents: &mut [IdentNode]) {
        for ident in idents {
            let pids: Vec<u32> = ident
                .instances
                .iter()
                .flat_map(|i| i.processes.iter())
                .flat_map(collect_pids)
                .collect();
            self.set_row(&mut ident.metrics, &pids);
            for inst in &mut ident.instances {
                let pids: Vec<u32> = inst.processes.iter().flat_map(collect_pids).collect();
                self.set_row(&mut inst.metrics, &pids);
                for p in &mut inst.processes {
                    self.set_process(p);
                }
            }
            for member in &mut ident.containers {
                let pids: Vec<u32> = member.processes.iter().flat_map(collect_pids).collect();
                self.set_row(&mut member.metrics, &pids);
                for p in &mut member.processes {
                    self.set_process(p);
                }
            }
        }
    }

    /// A row carries a figure only when every process under it lives in one
    /// non-root cgroup, because that is the only case where the kernel's
    /// number is *this row's* number. Pressure is a percentage of an interval
    /// and cannot be summed, so a row spanning several cgroups has nothing
    /// legitimate to show — the blank contract NETNS already established.
    ///
    /// The root cgroup is excluded for the reason a `--network=host`
    /// container's RX/TX is: its pressure is the machine's, and printing the
    /// machine's figure on a kernel-thread row would read as that row's cost.
    fn set_row(&self, m: &mut Metrics, pids: &[u32]) {
        if let Some(path) = self.one_cgroup(pids) {
            write(m, self.rates.get(path));
        }
    }

    /// A process is not a cgroup, so a process row shows a figure only where
    /// the cgroup holds exactly that pid and the kernel's number really is
    /// this process's. Four siblings sharing a scope would otherwise each
    /// print the same stall, which reads as four separate costs.
    fn set_process(&mut self, node: &mut crate::types::ProcNode) {
        if let Some(path) = self.solo_cgroup(node.pid) {
            write(&mut node.metrics, self.rates.get(&path));
        }
        for child in &mut node.children {
            self.set_process(child);
        }
    }

    fn one_cgroup(&self, pids: &[u32]) -> Option<&str> {
        let mut found: Option<&str> = None;
        for pid in pids {
            let path = cgroup_path(&self.procs.get(pid)?.cgroup)?;
            match found {
                None => found = Some(path),
                Some(f) if f == path => {}
                Some(_) => return None,
            }
        }
        found
    }

    fn solo_cgroup(&mut self, pid: u32) -> Option<String> {
        let path = cgroup_path(&self.procs.get(&pid)?.cgroup)?.to_string();
        let only = *self
            .solo
            .entry(path.clone())
            .or_insert_with(|| sole_member(&path));
        (only == Some(pid)).then_some(path)
    }
}

const fn write(m: &mut Metrics, s: Option<&Stall>) {
    let Some(s) = s else {
        return;
    };
    m.cpu_stall_pct = Some(s.cpu);
    m.io_stall_pct = Some(s.io);
    m.mem_stall_pct = Some(s.mem);
}

fn collect_pids(node: &crate::types::ProcNode) -> Vec<u32> {
    let mut out = vec![node.pid];
    for c in &node.children {
        out.extend(collect_pids(c));
    }
    out
}

/// `0::/user.slice/...` to `/user.slice/...`, and `None` for the root cgroup
/// or a v1-only line heft has no v2 path for.
fn cgroup_path(cgroup: &str) -> Option<&str> {
    let path = cgroup.lines().find_map(|l| l.strip_prefix("0::"))?;
    (path != "/" && path.starts_with('/')).then_some(path)
}

fn read_totals(path: &str) -> Option<Totals> {
    let base = format!(
        "{}/sys/fs/cgroup{}",
        crate::root::prefix(),
        path.trim_end_matches('/')
    );
    Some(Totals {
        cpu: read_some_total(&format!("{base}/cpu.pressure"))?,
        io: read_some_total(&format!("{base}/io.pressure"))?,
        mem: read_some_total(&format!("{base}/memory.pressure"))?,
    })
}

fn read_some_total(path: &str) -> Option<u64> {
    parse_some(&fs::read_to_string(path).ok()?, "total=")?
        .parse()
        .ok()
}

fn parse_some<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find(|l| l.starts_with("some "))?
        .split_whitespace()
        .find_map(|f| f.strip_prefix(key))
}

/// The one pid in this cgroup, or `None` if it holds any other number.
fn sole_member(path: &str) -> Option<u32> {
    let text = fs::read_to_string(format!(
        "{}/sys/fs/cgroup{path}/cgroup.procs",
        crate::root::prefix()
    ))
    .ok()?;
    let mut it = text.split_whitespace();
    let first = it.next()?.parse().ok()?;
    it.next().is_none().then_some(first)
}

/// `None` discards the interval rather than publishing a bogus figure. The
/// counters are cumulative, and a cgroup destroyed and recreated under the
/// same path (a restarted unit, a container that came back) starts from zero.
#[expect(
    clippy::cast_precision_loss,
    reason = "a pressure counter is microseconds of stall, so 2^53 of them is 285 years"
)]
fn delta(prev: Option<&Totals>, cur: Totals, secs: f64) -> Option<Stall> {
    let p = prev?;
    if cur.cpu < p.cpu || cur.io < p.io || cur.mem < p.mem || secs <= 0.0 {
        return None;
    }
    // total is microseconds of stall; the interval is seconds.
    let pct = |a: u64, b: u64| ((a - b) as f64 / (secs * 1e6) * 100.0).min(100.0);
    Some(Stall {
        cpu: pct(cur.cpu, p.cpu),
        io: pct(cur.io, p.io),
        mem: pct(cur.mem, p.mem),
    })
}

/// The kernel's own 10-second averages for the machine, for the header only.
/// Absent when the kernel was built without `CONFIG_PSI` or booted `psi=0`, in
/// which case the header simply carries no PSI tail, the way a swapless host
/// gets no swap tank.
pub(crate) fn host_avg10(tree: &mut HostTree) {
    let read = |res: &str| {
        let text =
            fs::read_to_string(format!("{}/proc/pressure/{res}", crate::root::prefix())).ok()?;
        parse_some(&text, "avg10=")?.parse::<f64>().ok()
    };
    tree.psi_cpu_avg10 = read("cpu");
    tree.psi_io_avg10 = read("io");
    tree.psi_mem_avg10 = read("memory");
}

/// `  psi 1.2/0.4/0.0` — cpu, io, memory, in the kernel's own 10-second
/// average, for the `--once` and `--json` host line. Not the TUI header: that
/// row exists to draw a bar to scale, and a text tail on it costs the bar the
/// columns it needs to line up with the one below. Empty when
/// the kernel publishes no pressure at all, the way a swapless host gets no
/// swap tank rather than a zeroed one; a single resource the kernel does not
/// account shows `-` rather than shifting the other two along.
pub(crate) fn header_tail(tree: &HostTree) -> String {
    if tree
        .psi_cpu_avg10
        .or(tree.psi_io_avg10)
        .or(tree.psi_mem_avg10)
        .is_none()
    {
        return String::new();
    }
    let f = |v: Option<f64>| v.map_or_else(|| "-".to_string(), crate::once::fmt_pct);
    format!(
        "  psi {}/{}/{}",
        f(tree.psi_cpu_avg10),
        f(tree.psi_io_avg10),
        f(tree.psi_mem_avg10)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "some avg10=1.25 avg60=0.40 avg300=0.00 total=9656812688\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=12\n";

    #[test]
    fn parses_the_some_line_and_never_the_full_one() {
        assert_eq!(parse_some(SAMPLE, "total="), Some("9656812688"));
        assert_eq!(parse_some(SAMPLE, "avg10="), Some("1.25"));
    }

    #[test]
    fn the_root_cgroup_is_not_a_row() {
        assert_eq!(cgroup_path("0::/"), None);
        assert_eq!(
            cgroup_path("0::/user.slice/user-1000.slice"),
            Some("/user.slice/user-1000.slice")
        );
    }

    #[test]
    fn a_recreated_cgroup_discards_the_interval_instead_of_going_negative() {
        let prev = Totals {
            cpu: 1_000_000,
            io: 0,
            mem: 0,
        };
        let after_restart = Totals {
            cpu: 5,
            io: 0,
            mem: 0,
        };
        assert!(delta(Some(&prev), after_restart, 1.0).is_none());
        assert!(delta(None, prev, 1.0).is_none(), "no baseline, no rate");

        // Half a second of stall in a one-second interval is 50%.
        let cur = Totals {
            cpu: 1_500_000,
            io: 0,
            mem: 0,
        };
        let s = delta(Some(&prev), cur, 1.0).expect("a forward counter yields a rate");
        assert!((s.cpu - 50.0).abs() < 1e-9, "{}", s.cpu);
    }
}
