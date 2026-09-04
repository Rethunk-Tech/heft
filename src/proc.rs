use std::collections::HashMap;
use std::fs;
use std::io;
use std::time::{Duration, Instant};

use crate::containers::ContainerIndex;
use crate::cpu;
use crate::group;
use crate::identity;
use crate::types::{HostTree, Process};
use crate::{gpu, io as pio};

pub fn enumerate() -> HashMap<u32, Process> {
    let mut out = HashMap::new();
    let Ok(dir) = fs::read_dir("/proc") else {
        return out;
    };
    for ent in dir.flatten() {
        let name = ent.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if let Some(p) = read_pid(pid) {
            out.insert(pid, p);
        }
    }
    out
}

pub fn read_pid(pid: u32) -> Option<Process> {
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
    let pss_kb = pio::read_pss_kb(pid);
    let (read_bytes, write_bytes) = pio::read_io(pid);
    let gpu = gpu::read_pid(pid);
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
    // after comm: state ppid pgrp session ... utime stime (0-based: 1,2,3,11,12)
    let ppid = fields.get(1)?.parse().ok()?;
    let pgrp = fields.get(2)?.parse().ok()?;
    let sid = fields.get(3)?.parse().ok()?;
    let utime = fields.get(11)?.parse().ok()?;
    let stime = fields.get(12)?.parse().ok()?;
    Some(StatFields {
        comm,
        ppid,
        pgrp,
        sid,
        utime,
        stime,
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

pub fn sample_world(interval: Duration) -> HostTree {
    let mut containers = ContainerIndex::load();
    let nproc = cpu::nproc();
    let clk = cpu::clk_tck();
    let page = cpu::page_size();
    let t0 = Instant::now();
    let cpu0 = cpu::read_host();
    let prev = enumerate();
    let engined = prev.values().find_map(|p| {
        if identity::user_unit(&p.cgroup).as_deref() == Some("engined.service") {
            Some(p.uid)
        } else {
            None
        }
    });
    containers.apply_engined_uid(engined);
    std::thread::sleep(interval);
    let cpu1 = cpu::read_host();
    let curr = enumerate();
    let elapsed = t0.elapsed();
    let header = cpu::header_from(nproc, clk, page, &cpu0, &cpu1);
    group::build_tree(&prev, &curr, elapsed, &header, &containers)
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
    }
}
