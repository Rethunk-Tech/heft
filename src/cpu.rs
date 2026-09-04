use std::fs;
use std::time::Duration;

use crate::types::{HostHeader, Metrics, Process};

#[derive(Clone, Copy, Debug, Default)]
pub struct HostCpu {
    pub idle: u64,
    pub total: u64,
}

pub fn nproc() -> u32 {
    match fs::read_to_string("/proc/stat") {
        Ok(text) => {
            let n = text
                .lines()
                .filter(|l| {
                    l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(|c| c.is_ascii_digit())
                })
                .count() as u32;
            n.max(1)
        }
        Err(_) => 1,
    }
}

pub fn clk_tck() -> u64 {
    // SAFETY: sysconf is a pure query; a non-positive result is replaced.
    let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if v > 0 { v as u64 } else { 100 }
}

pub fn page_size() -> u64 {
    // SAFETY: sysconf is a pure query; a non-positive result is replaced.
    let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if v > 0 { v as u64 } else { 4096 }
}

pub fn euid() -> u32 {
    // SAFETY: geteuid has no side effects.
    unsafe { libc::geteuid() }
}

pub fn read_host() -> HostCpu {
    let Ok(text) = fs::read_to_string("/proc/stat") else {
        return HostCpu::default();
    };
    let Some(line) = text.lines().find(|l| l.starts_with("cpu ")) else {
        return HostCpu::default();
    };
    parse_host_cpu(line).unwrap_or_default()
}

pub fn parse_host_cpu(line: &str) -> Option<HostCpu> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "cpu" {
        return None;
    }
    let mut vals = [0u64; 10];
    for (i, p) in parts.take(10).enumerate() {
        vals[i] = p.parse().ok()?;
    }
    let total: u64 = vals.iter().sum();
    let idle = vals.get(3).copied().unwrap_or(0) + vals.get(4).copied().unwrap_or(0);
    Some(HostCpu { idle, total })
}

pub fn host_pct(a: &HostCpu, b: &HostCpu) -> f64 {
    let dt = b.total.saturating_sub(a.total) as f64;
    if dt <= 0.0 {
        return 0.0;
    }
    let di = b.idle.saturating_sub(a.idle) as f64;
    ((dt - di) / dt * 100.0).clamp(0.0, 100.0)
}

pub fn process_metrics(
    prev: Option<&Process>,
    cur: &Process,
    elapsed: Duration,
    nproc: u32,
    clk: u64,
    page: u64,
) -> Metrics {
    let secs = elapsed.as_secs_f64().max(1e-6);
    let cores = nproc.max(1) as f64;
    let (core, machine) = match prev {
        Some(p) => {
            let ticks = (cur.utime + cur.stime).saturating_sub(p.utime + p.stime) as f64;
            let core = 100.0 * ticks / (clk as f64 * secs);
            (core, core / cores)
        }
        None => (0.0, 0.0),
    };
    let rss_bytes = cur.rss_pages.map(|p| p.saturating_mul(page));
    let pss_bytes = cur.pss_kb.map(|k| k.saturating_mul(1024));
    let disk_r_bps = rate(prev.and_then(|p| p.read_bytes), cur.read_bytes, secs);
    let disk_w_bps = rate(prev.and_then(|p| p.write_bytes), cur.write_bytes, secs);
    let gfx_pct = engine_pct(prev.and_then(|p| p.gpu.gfx_ns), cur.gpu.gfx_ns, secs);
    let compute_pct = engine_pct(
        prev.and_then(|p| p.gpu.compute_ns),
        cur.gpu.compute_ns,
        secs,
    );
    Metrics {
        cpu_core_pct: core,
        cpu_machine_pct: machine,
        rss_bytes,
        pss_bytes,
        disk_r_bps,
        disk_w_bps,
        vram_bytes: cur.gpu.vram_bytes,
        gtt_bytes: cur.gpu.gtt_bytes,
        gfx_pct,
        compute_pct,
    }
}

fn rate(prev: Option<u64>, cur: Option<u64>, secs: f64) -> Option<f64> {
    let (a, b) = (prev?, cur?);
    Some(b.saturating_sub(a) as f64 / secs)
}

fn engine_pct(prev: Option<u64>, cur: Option<u64>, secs: f64) -> Option<f64> {
    let (a, b) = (prev?, cur?);
    let dns = b.saturating_sub(a) as f64;
    Some((dns / (secs * 1_000_000_000.0) * 100.0).clamp(0.0, 100.0))
}

pub fn header_from(nproc: u32, clk: u64, page: u64, a: &HostCpu, b: &HostCpu) -> HostHeader {
    let (mem_used_bytes, mem_total_bytes) = crate::mem::read_ram();
    let (vram_used_bytes, vram_total_bytes) = crate::mem::read_vram();
    HostHeader {
        nproc,
        clk_tck: clk,
        page_size: page,
        cpu_pct: host_pct(a, b),
        mem_used_bytes,
        mem_total_bytes,
        vram_used_bytes,
        vram_total_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_cpu_line() {
        let a = parse_host_cpu("cpu  10 0 10 80 0 0 0 0 0 0").unwrap();
        let b = parse_host_cpu("cpu  20 0 20 80 0 0 0 0 0 0").unwrap();
        assert!((host_pct(&a, &b) - 100.0).abs() < 0.01);
    }
}
