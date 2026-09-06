use std::fs;
use std::time::Duration;

use crate::types::{HostHeader, Metrics, Process};

#[derive(Clone, Copy, Debug, Default)]
pub struct HostCpu {
    /// user + nice (guest already sits inside these kernel counters)
    pub user: u64,
    /// system + irq + softirq
    pub system: u64,
    pub wait: u64,
    /// includes idle + steal, the unfilled remainder of the header bar
    pub total: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostSplit {
    pub user: f64,
    pub system: f64,
    pub wait: f64,
    pub busy: f64,
}

pub fn nproc() -> u32 {
    match fs::read_to_string("/proc/stat") {
        Ok(text) => {
            let n = text
                .lines()
                .filter(|l| {
                    l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(u8::is_ascii_digit)
                })
                .count();
            u32::try_from(n).unwrap_or(u32::MAX).max(1)
        }
        Err(_) => 1,
    }
}

pub fn clk_tck() -> u64 {
    // SAFETY: sysconf is a pure query; a non-positive result is replaced.
    let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if v > 0 { v.cast_unsigned() } else { 100 }
}

pub fn page_size() -> u64 {
    // SAFETY: sysconf is a pure query; a non-positive result is replaced.
    let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if v > 0 { v.cast_unsigned() } else { 4096 }
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
    let mut vals = [0u64; 8];
    let mut n = 0;
    for (i, p) in parts.take(8).enumerate() {
        vals[i] = p.parse().ok()?;
        n = i + 1;
    }
    if n < 4 {
        return None;
    }
    let user = vals[0].saturating_add(vals[1]);
    let system = vals[2].saturating_add(vals[5]).saturating_add(vals[6]);
    let idle = vals[3].saturating_add(vals[7]);
    let wait = vals[4];
    Some(HostCpu {
        user,
        system,
        wait,
        total: user
            .saturating_add(system)
            .saturating_add(wait)
            .saturating_add(idle),
    })
}

pub fn host_split(a: &HostCpu, b: &HostCpu) -> HostSplit {
    let dt = b.total.saturating_sub(a.total) as f64;
    if dt <= 0.0 {
        return HostSplit::default();
    }
    let pct = |lo: u64, hi: u64| (hi.saturating_sub(lo) as f64 / dt * 100.0).clamp(0.0, 100.0);
    let user = pct(a.user, b.user);
    let system = pct(a.system, b.system);
    let wait = pct(a.wait, b.wait);
    HostSplit {
        user,
        system,
        wait,
        // a counter reset leaves each term clamped at 100 on its own, so the
        // sum needs its own ceiling to stay a percentage of one machine
        busy: (user + system + wait).min(100.0),
    }
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
    let cores = f64::from(nproc.max(1));
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
    let ram = crate::mem::read_ram();
    let gpu = crate::mem::read_gpu();
    let split = host_split(a, b);
    HostHeader {
        nproc,
        clk_tck: clk,
        page_size: page,
        cpu_pct: split.busy,
        cpu_user_pct: split.user,
        cpu_system_pct: split.system,
        cpu_wait_pct: split.wait,
        mem_used_bytes: ram.used_bytes,
        mem_total_bytes: ram.total_bytes,
        mem_buffers_bytes: ram.buffers_bytes,
        mem_cached_bytes: ram.cached_bytes,
        vram_used_bytes: gpu.vram_used,
        vram_total_bytes: gpu.vram_total,
        unified_memory: crate::mem::is_unified(ram.total_bytes, &gpu),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_cpu_line() {
        let a = parse_host_cpu("cpu  10 0 10 80 0 0 0 0 0 0").unwrap();
        let b = parse_host_cpu("cpu  20 0 20 80 0 0 0 0 0 0").unwrap();
        assert!((host_split(&a, &b).busy - 100.0).abs() < 0.01);
    }

    #[test]
    fn counter_reset_keeps_busy_at_one_machine() {
        // idle resets to 0 while user and system each jump a full dt
        let a = parse_host_cpu("cpu  100 0 100 100 0 0 0 0").unwrap();
        let b = parse_host_cpu("cpu  200 0 200 0 0 0 0 0").unwrap();
        let s = host_split(&a, &b);
        assert!((s.user - 100.0).abs() < 0.01);
        assert!((s.system - 100.0).abs() < 0.01);
        assert!((s.busy - 100.0).abs() < 0.01);
    }

    #[test]
    fn host_cpu_split_user_sys_wait() {
        let a = parse_host_cpu("cpu  100 10 50 800 40 5 5 0").unwrap();
        let b = parse_host_cpu("cpu  200 20 100 850 90 10 10 0").unwrap();
        let s = host_split(&a, &b);
        let dt = 270.0;
        assert!((s.user - 110.0 / dt * 100.0).abs() < 0.01);
        assert!((s.system - 60.0 / dt * 100.0).abs() < 0.01);
        assert!((s.wait - 50.0 / dt * 100.0).abs() < 0.01);
        assert!((s.busy - (s.user + s.system + s.wait)).abs() < 0.01);
    }

    #[test]
    fn guest_fields_do_not_inflate_total() {
        let cpu = parse_host_cpu("cpu  10 0 10 80 0 0 0 0 50 25").unwrap();
        assert_eq!(cpu.user, 10);
        assert_eq!(cpu.total, 100);
    }
}
