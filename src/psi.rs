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

use std::ffi::CStr;
use std::os::fd::OwnedFd;

use rustix::fs::{CWD, Mode, OFlags};

use crate::proc::read_str;
use crate::types::{HostTree, IdentNode, Metrics, PidMap, Process};

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
    pub(crate) fn tick(&mut self, procs: &PidMap<Process>, secs: f64) -> Stalls {
        let mut curr = HashMap::new();
        let mut rates = HashMap::new();
        let mut buf = Vec::new();
        for p in procs.values() {
            let Some(path) = cgroup_path(&p.cgroup) else {
                continue;
            };
            if curr.contains_key(path) {
                continue;
            }
            let Some(now) = read_totals(path, &mut buf) else {
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
    pub(crate) fn apply(&self, tree: &mut HostTree, procs: &PidMap<Process>) {
        let mut seen: HashMap<&str, u32> = HashMap::new();
        for path in procs.values().filter_map(|p| cgroup_path(&p.cgroup)) {
            *seen.entry(path).or_default() += 1;
        }
        let t = Apply {
            rates: &self.0,
            seen,
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
    /// How many walked pids each cgroup holds this tick.
    seen: HashMap<&'a str, u32>,
    procs: &'a PidMap<Process>,
}

impl<'a> Apply<'a> {
    /// One pid buffer for the whole pass: each instance's processes are walked
    /// once, its row judged on its own slice and the identity on all of them.
    fn bill(&self, idents: &mut [IdentNode]) {
        let mut pids = Vec::new();
        for ident in idents {
            pids.clear();
            for inst in &mut ident.instances {
                let start = pids.len();
                push_pids(&inst.processes, &mut pids);
                self.set_row(&mut inst.metrics, &pids[start..]);
                for p in &mut inst.processes {
                    self.set_process(p);
                }
            }
            self.set_row(&mut ident.metrics, &pids);
            for member in &mut ident.containers {
                pids.clear();
                push_pids(&member.processes, &mut pids);
                self.set_row(&mut member.metrics, &pids);
                for p in &mut member.processes {
                    self.set_process(p);
                }
            }
        }
    }

    /// One non-root cgroup is that cgroup's `some` rate. Several is the max of
    /// each member's `some`, per resource independently. Sum can exceed 100%
    /// (stall intervals overlap); average hides a member that was fully
    /// stalled. Max is the worst constituent and stays ≤100%.
    ///
    /// Folder, User and Host rows are never billed here. Root-cgroup rows stay
    /// blank for the same reason as a `--network=host` container's RX/TX: that
    /// pressure is the machine's. Process rows go through `set_process`.
    fn set_row(&self, m: &mut Metrics, pids: &[u32]) {
        if let Some(path) = self.one_cgroup(pids) {
            write(m, self.rates.get(path));
            return;
        }
        write(m, self.max_some(pids).as_ref());
    }

    fn max_some(&self, pids: &[u32]) -> Option<Stall> {
        let mut first: Option<&str> = None;
        let mut multi = false;
        let mut acc: Option<Stall> = None;
        for pid in pids {
            let path = cgroup_path(&self.procs.get(pid)?.cgroup)?;
            match first {
                None => first = Some(path),
                Some(f) if f == path => {}
                Some(_) => multi = true,
            }
            let Some(s) = self.rates.get(path) else {
                continue;
            };
            acc = Some(match acc {
                None => *s,
                Some(a) => Stall {
                    cpu: a.cpu.max(s.cpu),
                    io: a.io.max(s.io),
                    mem: a.mem.max(s.mem),
                },
            });
        }
        if multi { acc } else { None }
    }

    /// A process is not a cgroup, so a process row shows a figure only where
    /// the cgroup holds exactly that pid and the kernel's number really is
    /// this process's. Four siblings sharing a scope would otherwise each
    /// print the same stall, which reads as four separate costs.
    fn set_process(&self, node: &mut crate::types::ProcNode) {
        if let Some(path) = self.solo_cgroup(node.pid) {
            write(&mut node.metrics, self.rates.get(path));
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

    /// Two walked pids in one cgroup already settle that it is not solo, so
    /// `cgroup.procs` is read only for a cgroup the walk saw once, where it
    /// still matters because it also lists pids `/proc` hides. It can
    /// disagree with a full read only when a pid leaves the cgroup between
    /// the walk and the apply, and then it blanks a row for one tick.
    fn solo_cgroup(&self, pid: u32) -> Option<&'a str> {
        let path = cgroup_path(&self.procs.get(&pid)?.cgroup)?;
        (self.seen.get(path) == Some(&1) && sole_member(path) == Some(pid)).then_some(path)
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

fn push_pids(nodes: &[crate::types::ProcNode], out: &mut Vec<u32>) {
    for node in nodes {
        out.push(node.pid);
        push_pids(&node.children, out);
    }
}

/// `0::/user.slice/...` to `/user.slice/...`, and `None` for the root cgroup
/// or a v1-only line heft has no v2 path for.
fn cgroup_path(cgroup: &str) -> Option<&str> {
    let path = cgroup.lines().find_map(|l| l.strip_prefix("0::"))?;
    (path != "/" && path.starts_with('/')).then_some(path)
}

/// The cgroup directory is resolved once, so each pressure file is a
/// one-name lookup rather than the whole path again.
fn read_totals(path: &str, buf: &mut Vec<u8>) -> Option<Totals> {
    let dir = rustix::fs::open(
        format!("{}/sys/fs/cgroup{path}", crate::root::prefix()).as_str(),
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    Some(Totals {
        cpu: read_some_total(&dir, c"cpu.pressure", buf)?,
        io: read_some_total(&dir, c"io.pressure", buf)?,
        mem: read_some_total(&dir, c"memory.pressure", buf)?,
    })
}

/// `proc::read_str`, not `fs::read_to_string`, for the reason on `read_at`.
fn read_some_total(dir: &OwnedFd, name: &CStr, buf: &mut Vec<u8>) -> Option<u64> {
    parse_some(&read_str(dir, name, buf)?, "total=")?
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
    let mut buf = Vec::new();
    let text = read_str(
        CWD,
        format!("{}/sys/fs/cgroup{path}/cgroup.procs", crate::root::prefix()),
        &mut buf,
    )?;
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
    let mut buf = Vec::new();
    let mut read = |res: &str| {
        let text = read_str(
            CWD,
            format!("{}/proc/pressure/{res}", crate::root::prefix()),
            &mut buf,
        )?;
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

    fn proc_in(pid: u32, cgroup: &str) -> Process {
        Process {
            pid,
            cgroup: cgroup.into(),
            ..Process::default()
        }
    }

    #[test]
    fn multi_cgroup_row_takes_the_max_some() {
        let rates = HashMap::from([
            (
                "/a".into(),
                Stall {
                    cpu: 10.0,
                    io: 0.0,
                    mem: 0.0,
                },
            ),
            (
                "/b".into(),
                Stall {
                    cpu: 40.0,
                    io: 0.0,
                    mem: 0.0,
                },
            ),
        ]);
        let procs = PidMap::from_iter([(1, proc_in(1, "0::/a")), (2, proc_in(2, "0::/b"))]);
        let t = Apply {
            rates: &rates,
            seen: HashMap::new(),
            procs: &procs,
        };

        let mut multi = Metrics::default();
        t.set_row(&mut multi, &[1, 2]);
        assert_eq!(multi.cpu_stall_pct, Some(40.0));

        let mut solo = Metrics::default();
        t.set_row(&mut solo, &[1]);
        assert_eq!(solo.cpu_stall_pct, Some(10.0));
    }
}
