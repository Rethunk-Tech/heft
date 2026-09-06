use std::collections::HashMap;
use std::fs;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::containers::{ContainerIndex, InspectCache};
use crate::cpu;
use crate::group;
use crate::identity;
use crate::types::{GpuCounters, HostTree, Process};
use crate::{gpu, io as pio};

fn collect(want_pss: bool, prev: Option<&HashMap<u32, Process>>) -> HashMap<u32, Process> {
    let mut out = HashMap::new();
    let Ok(dir) = fs::read_dir("/proc") else {
        return out;
    };
    for ent in dir.flatten() {
        let name = ent.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if let Some(p) = read_pid(pid, want_pss, prev.and_then(|m| m.get(&pid))) {
            out.insert(pid, p);
        }
    }
    out
}

fn read_pid(pid: u32, want_pss: bool, prev: Option<&Process>) -> Option<Process> {
    let base = format!("/proc/{pid}");
    let stat = fs::read_to_string(format!("{base}/stat")).ok()?;
    let parsed = parse_stat(&stat)?;
    let uid = read_uid(&format!("{base}/status")).unwrap_or(0);
    let exe = read_exe(&format!("{base}/exe"));
    let cmdline = read_cmdline(&format!("{base}/cmdline"));
    let cgroup = fs::read_to_string(format!("{base}/cgroup"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let rss_pages = read_rss_pages(&format!("{base}/statm"));
    // PSS is a level, not a rate. Kernel threads have no rollup. Prime and
    // TUI ticks between `--pss-interval` reuse last (new PIDs stay blank).
    let pss_kb = pss_kb_for(want_pss, parsed.kthread, prev.and_then(|p| p.pss_kb), pid);
    // PF_KTHREAD has no userspace /proc/pid/io or drm fdinfo.
    let (read_bytes, write_bytes, gpu) = if parsed.kthread {
        (None, None, GpuCounters::default())
    } else {
        let (r, w) = pio::read_io(pid);
        // want_pss is also the GPU full fdinfo walk (PSS tick / --once publish).
        (r, w, gpu::read_pid(pid, want_pss))
    };
    Some(Process {
        pid,
        ppid: parsed.ppid,
        pgrp: parsed.pgrp,
        sid: parsed.sid,
        uid,
        comm: parsed.comm,
        exe,
        cmdline,
        cgroup,
        utime: parsed.utime,
        stime: parsed.stime,
        rss_pages,
        pss_kb,
        read_bytes,
        write_bytes,
        gpu,
    })
}

fn pss_kb_for(want_pss: bool, kthread: bool, prev: Option<u64>, pid: u32) -> Option<u64> {
    if kthread {
        None
    } else if want_pss {
        pio::read_pss_kb(pid)
    } else {
        prev
    }
}

/// Floor 0.05s; PSS cadence is at least the catch-all interval.
#[must_use]
pub fn clamp_intervals(interval_s: f64, pss_s: f64) -> (Duration, Duration) {
    let interval = Duration::from_secs_f64(interval_s.max(0.05));
    let pss = Duration::from_secs_f64(pss_s.max(0.05)).max(interval);
    (interval, pss)
}

fn pss_due(last: Option<Instant>, now: Instant, interval: Duration) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= interval)
}

struct StatFields {
    comm: String,
    ppid: u32,
    pgrp: i32,
    sid: i32,
    utime: u64,
    stime: u64,
    kthread: bool,
}

fn parse_stat(stat: &str) -> Option<StatFields> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    if close <= open {
        return None;
    }
    let comm = stat[open + 1..close].to_string();
    let rest = stat[close + 1..].split_whitespace();
    let fields: Vec<&str> = rest.collect();
    // after comm: state ppid pgrp session ... flags ... utime stime (0-based: 1,2,3,6,11,12)
    let ppid = fields.get(1)?.parse().ok()?;
    let pgrp = fields.get(2)?.parse().ok()?;
    let sid = fields.get(3)?.parse().ok()?;
    // PF_KTHREAD in include/linux/sched.h — no userspace smaps/io/fdinfo.
    let flags: u32 = fields.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);
    let utime = fields.get(11)?.parse().ok()?;
    let stime = fields.get(12)?.parse().ok()?;
    Some(StatFields {
        comm,
        ppid,
        pgrp,
        sid,
        utime,
        stime,
        kthread: flags & 0x0020_0000 != 0,
    })
}

fn read_uid(status_path: &str) -> Option<u32> {
    // /proc/<pid> inode uid is euid; grouping uses ruid (Uid: field 1). They
    // diverge on setuid (e.g. fusermount3).
    let text = fs::read_to_string(status_path).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn read_exe(path: &str) -> Option<String> {
    match fs::read_link(path) {
        Ok(p) => Some(strip_deleted(&p.to_string_lossy()).to_string()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => None,
        Err(_) => None,
    }
}

fn strip_deleted(s: &str) -> &str {
    s.strip_suffix(" (deleted)").unwrap_or(s)
}

fn read_cmdline(path: &str) -> Vec<String> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn read_rss_pages(path: &str) -> Option<u64> {
    let text = fs::read_to_string(path).ok()?;
    text.split_whitespace().nth(1)?.parse().ok()
}
pub(crate) fn username(uid: u32) -> String {
    if let Ok(text) = fs::read_to_string("/etc/passwd") {
        for line in text.lines() {
            let mut it = line.split(':');
            let name = it.next().unwrap_or("");
            let _ = it.next();
            if it.next().and_then(|s| s.parse::<u32>().ok()) == Some(uid) && !name.is_empty() {
                return name.to_string();
            }
        }
    }
    uid.to_string()
}

/// Header totals from world-readable files only — no per-PID `/proc` walk.
pub(crate) fn placeholder_tree() -> HostTree {
    let nproc = cpu::nproc();
    let cpu = cpu::HostCpu::default();
    let header = cpu::header_from(nproc, cpu::clk_tck(), cpu::page_size(), &cpu, &cpu);
    HostTree::from(&header)
}

struct Sampler {
    prev: HashMap<u32, Process>,
    cpu0: cpu::HostCpu,
    t0: Instant,
    last_pss: Option<Instant>,
    pss_interval: Duration,
    nproc: u32,
    clk: u64,
    page: u64,
    inspect_cache: InspectCache,
}

impl Sampler {
    fn prime(pss_interval: Duration) -> Self {
        let t0 = Instant::now();
        Self {
            nproc: cpu::nproc(),
            clk: cpu::clk_tck(),
            page: cpu::page_size(),
            cpu0: cpu::read_host(),
            prev: collect(false, None),
            t0,
            last_pss: None,
            pss_interval,
            inspect_cache: InspectCache::default(),
        }
    }

    fn tick(&mut self, force_pss: bool) -> HostTree {
        let mut containers = ContainerIndex::load(&mut self.inspect_cache);
        let engined = self.prev.values().find_map(|p| {
            if identity::user_unit(&p.cgroup).as_deref() == Some("engined.service") {
                Some(p.uid)
            } else {
                None
            }
        });
        containers.apply_engined_uid(engined);
        let t1 = Instant::now();
        let cpu1 = cpu::read_host();
        let want_pss = force_pss || pss_due(self.last_pss, t1, self.pss_interval);
        let curr = collect(want_pss, Some(&self.prev));
        if want_pss {
            self.last_pss = Some(t1);
        }
        let elapsed = t1.duration_since(self.t0);
        let header = cpu::header_from(self.nproc, self.clk, self.page, &self.cpu0, &cpu1);
        let tree = group::build_tree(&self.prev, &curr, elapsed, &header, &containers);
        self.prev = curr;
        self.cpu0 = cpu1;
        self.t0 = t1;
        tree
    }
}
pub(crate) fn sample_world(interval: Duration) -> HostTree {
    let mut sampler = Sampler::prime(interval);
    thread::sleep(interval);
    sampler.tick(true)
}

/// Latest complete tree. The UI takes; the sampler only publishes.
///
/// # Errors
///
/// Returns an error if the sampler thread cannot be spawned.
pub(crate) fn spawn_sampler(
    interval: Duration,
    pss_interval: Duration,
    slot: Arc<Mutex<Option<HostTree>>>,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("heft-sample".into())
        .spawn(move || {
            let mut sampler = Sampler::prime(pss_interval);
            loop {
                let start = Instant::now();
                let tree = sampler.tick(false);
                *slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tree);
                thread::sleep(interval.saturating_sub(start.elapsed()));
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_with_spaces_in_comm() {
        let mut tail = String::from("10 (my app) S 1 10 10 0 0 0 0 0 0 0 30 40");
        for _ in 0..20 {
            tail.push_str(" 0");
        }
        let p = parse_stat(&tail).unwrap();
        assert_eq!(p.comm, "my app");
        assert_eq!(p.ppid, 1);
        assert_eq!(p.pgrp, 10);
        assert_eq!(p.utime, 30);
        assert_eq!(p.stime, 40);
        assert!(!p.kthread);
    }

    #[test]
    fn kthread_flag_from_stat() {
        let mut tail = String::from("10 (kworker/0:0) S 2 0 0 0 0 2097152 0 0 0 0 1 2");
        for _ in 0..20 {
            tail.push_str(" 0");
        }
        let p = parse_stat(&tail).unwrap();
        assert!(p.kthread);
        assert_eq!(p.ppid, 2);
    }

    #[test]
    fn clamp_floors_and_pss_at_least_interval() {
        let (i, p) = clamp_intervals(1.0, 5.0);
        assert_eq!(i, Duration::from_secs(1));
        assert_eq!(p, Duration::from_secs(5));
        let (i, p) = clamp_intervals(0.01, 0.01);
        assert_eq!(i, Duration::from_millis(50));
        assert_eq!(p, Duration::from_millis(50));
        let (i, p) = clamp_intervals(10.0, 5.0);
        assert_eq!(i, Duration::from_secs(10));
        assert_eq!(p, Duration::from_secs(10));
    }

    #[test]
    fn pss_due_first_then_interval() {
        let start = Instant::now();
        let five = Duration::from_secs(5);
        assert!(pss_due(None, start, five));
        assert!(!pss_due(Some(start), start, five));
        assert!(!pss_due(Some(start), start + Duration::from_secs(4), five));
        assert!(pss_due(Some(start), start + five, five));
    }

    #[test]
    fn carried_pss_skips_kthread_and_new_pids() {
        assert_eq!(pss_kb_for(false, true, Some(12), 1), None);
        assert_eq!(pss_kb_for(false, false, Some(12), 1), Some(12));
        assert_eq!(pss_kb_for(false, false, None, 1), None);
    }
}
