use std::collections::HashMap;
use std::fs;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::containers::ContainerIndex;
use crate::cpu;
use crate::group;
use crate::identity;
use crate::types::{HostTree, Process};
use crate::{gpu, io as pio};

pub fn enumerate() -> HashMap<u32, Process> {
    collect(true, None)
}

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
    // PSS is a level, not a rate. Prime skips it; kernel threads have no rollup.
    let pss_kb = if want_pss && !parsed.kthread {
        pio::read_pss_kb(pid)
    } else {
        None
    };
    let (read_bytes, write_bytes) = pio::read_io(pid);
    let gpu = gpu::read_pid(pid, prev.map(|p| &p.gpu));
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
    // PF_KTHREAD in include/linux/sched.h — no userspace smaps_rollup.
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

pub fn username(uid: u32) -> String {
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
pub fn placeholder_tree() -> HostTree {
    let nproc = cpu::nproc();
    let cpu = cpu::HostCpu::default();
    let header = cpu::header_from(nproc, cpu::clk_tck(), cpu::page_size(), &cpu, &cpu);
    HostTree {
        nproc: header.nproc,
        cpu_pct: header.cpu_pct,
        cpu_user_pct: header.cpu_user_pct,
        cpu_system_pct: header.cpu_system_pct,
        cpu_wait_pct: header.cpu_wait_pct,
        mem_used_bytes: header.mem_used_bytes,
        mem_total_bytes: header.mem_total_bytes,
        mem_buffers_bytes: header.mem_buffers_bytes,
        mem_cached_bytes: header.mem_cached_bytes,
        vram_used_bytes: header.vram_used_bytes,
        vram_total_bytes: header.vram_total_bytes,
        unified_memory: header.unified_memory,
        users: Vec::new(),
        containers: Vec::new(),
        system: Vec::new(),
    }
}

struct Sampler {
    prev: HashMap<u32, Process>,
    cpu0: cpu::HostCpu,
    t0: Instant,
    nproc: u32,
    clk: u64,
    page: u64,
}

impl Sampler {
    fn prime() -> Self {
        let t0 = Instant::now();
        Self {
            nproc: cpu::nproc(),
            clk: cpu::clk_tck(),
            page: cpu::page_size(),
            cpu0: cpu::read_host(),
            prev: collect(false, None),
            t0,
        }
    }

    fn tick(&mut self) -> HostTree {
        let mut containers = ContainerIndex::load();
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
        let curr = collect(true, Some(&self.prev));
        let elapsed = t1.duration_since(self.t0);
        let header = cpu::header_from(self.nproc, self.clk, self.page, &self.cpu0, &cpu1);
        let tree = group::build_tree(&self.prev, &curr, elapsed, &header, &containers);
        self.prev = curr;
        self.cpu0 = cpu1;
        self.t0 = t1;
        tree
    }
}

pub fn sample_world(interval: Duration) -> HostTree {
    let mut sampler = Sampler::prime();
    thread::sleep(interval);
    sampler.tick()
}

/// Latest complete tree. The UI takes; the sampler only publishes.
pub fn spawn_sampler(
    interval: Duration,
    slot: Arc<Mutex<Option<HostTree>>>,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("heft-sample".into())
        .spawn(move || {
            let mut sampler = Sampler::prime();
            loop {
                let start = Instant::now();
                let tree = sampler.tick();
                *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(tree);
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
}
