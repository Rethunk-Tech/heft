#![expect(
    clippy::cast_precision_loss,
    reason = "every cast here widens a kernel counter -- clock ticks, nanoseconds, \
bytes -- into the f64 a rate is divided in. A counter big enough to lose precision at \
2^53 is centuries of uptime, and the quotient is printed to three significant figures."
)]

use std::fs;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::proc::field_u64;
use crate::types::{HostHeader, HostTree, Metrics, Process};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HostCpu {
    /// user + nice (guest already sits inside these kernel counters)
    pub user: u64,
    /// system + irq + softirq
    pub system: u64,
    pub wait: u64,
    /// includes idle + steal, the unfilled remainder of the header bar
    pub total: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct HostSplit {
    pub user: f64,
    pub system: f64,
    pub wait: f64,
    pub busy: f64,
}

pub(crate) fn nproc() -> u32 {
    match fs::read_to_string(crate::root::path("/proc/stat")) {
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

/// The epoch second the machine booted, which every `/proc/pid/stat`
/// `starttime` is an offset from. Read from `/proc/stat` once per heft run and
/// then pinned: it is a property of this boot, and re-reading it every sample
/// would let an NTP step walk every age heft has already printed. 0 when the
/// line is missing, which `age_secs` turns into a blank rather than a date in
/// 1970.
fn btime() -> u64 {
    static BTIME: OnceLock<u64> = OnceLock::new();
    *BTIME.get_or_init(|| {
        fs::read_to_string(crate::root::path("/proc/stat"))
            .ok()
            .and_then(|t| t.lines().find_map(|l| field_u64(l, "btime")))
            .unwrap_or(0)
    })
}

pub(crate) fn clk_tck() -> u64 {
    // SAFETY: sysconf is a pure query; a non-positive result is replaced.
    let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if v > 0 { v.cast_unsigned() } else { 100 }
}

pub(crate) fn page_size() -> u64 {
    // SAFETY: sysconf is a pure query; a non-positive result is replaced.
    let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if v > 0 { v.cast_unsigned() } else { 4096 }
}

pub(crate) fn euid() -> u32 {
    // SAFETY: geteuid has no side effects.
    unsafe { libc::geteuid() }
}

pub(crate) fn read_host() -> HostCpu {
    let Ok(text) = fs::read_to_string(crate::root::path("/proc/stat")) else {
        return HostCpu::default();
    };
    let Some(line) = text.lines().find(|l| l.starts_with("cpu ")) else {
        return HostCpu::default();
    };
    parse_host_cpu(line).unwrap_or_default()
}

pub(crate) fn parse_host_cpu(line: &str) -> Option<HostCpu> {
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

fn host_split(a: &HostCpu, b: &HostCpu) -> HostSplit {
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

pub(crate) fn process_metrics(
    prev: Option<&Process>,
    cur: &Process,
    elapsed: Duration,
    consts: &HostHeader,
) -> Metrics {
    let secs = elapsed.as_secs_f64().max(1e-6);
    let cores = f64::from(consts.nproc.max(1));
    let (core, machine) = match prev {
        Some(p) => {
            let ticks = (cur.utime + cur.stime).saturating_sub(p.utime + p.stime) as f64;
            let core = 100.0 * ticks / (consts.clk_tck as f64 * secs);
            (core, core / cores)
        }
        None => (0.0, 0.0),
    };
    let rss_bytes = cur.rss_pages.map(|p| p.saturating_mul(consts.page_size));
    let pss_bytes = cur.pss_kb.map(|k| k.saturating_mul(1024));
    let swap_bytes = cur.swap_pss_kb.map(|k| k.saturating_mul(1024));
    let disk_r_bps = rate(prev.and_then(|p| p.read_bytes), cur.read_bytes, secs);
    let disk_w_bps = rate(prev.and_then(|p| p.write_bytes), cur.write_bytes, secs);
    // Two formulas, because the drivers measure two different things: a
    // duration against the wall clock, and a cycle count against the GPU's own
    // clock. `or_else` keeps a driver that publishes ns on the ns path.
    let span = cycles_span(prev, cur);
    let gfx_pct = engine_pct(prev.and_then(|p| p.gpu.gfx_ns), cur.gpu.gfx_ns, secs).or_else(|| {
        cycles_pct(
            prev.and_then(|p| p.gpu.gfx_cycles),
            cur.gpu.gfx_cycles,
            span,
        )
    });
    let compute_pct = engine_pct(
        prev.and_then(|p| p.gpu.compute_ns),
        cur.gpu.compute_ns,
        secs,
    )
    .or_else(|| {
        cycles_pct(
            prev.and_then(|p| p.gpu.compute_cycles),
            cur.gpu.compute_cycles,
            span,
        )
    });
    Metrics {
        cpu_core_pct: core,
        cpu_machine_pct: machine,
        rss_bytes,
        pss_bytes,
        swap_bytes,
        threads: cur.threads,
        age_secs: age_secs(cur.starttime_ticks, consts.clk_tck, btime(), now_epoch()),
        disk_r_bps,
        disk_w_bps,
        vram_bytes: cur.gpu.vram_bytes,
        gtt_bytes: cur.gpu.gtt_bytes,
        gfx_pct,
        compute_pct,
        d_state_procs: u32::from(cur.d_state),
        // No per-process network counter exists to fill these. `/proc/<pid>/net/dev`
        // is the namespace total, socket fdinfo has no byte counter, `/proc/net/tcp`
        // queues are depths rather than totals, and `rchar`/`wchar` miss send/recv.
        // `net::Rates` bills the container rows afterwards.
        net_rx_bps: None,
        net_tx_bps: None,
        // Pressure is accounted per cgroup, never per process, so nothing here
        // can fill these either. `psi::Sampler` bills the rows that are one
        // cgroup afterwards.
        cpu_stall_pct: None,
        io_stall_pct: None,
        mem_stall_pct: None,
    }
}

/// Wall clock in epoch seconds. `SystemTime::now` is a vDSO read rather than a
/// syscall, so paying it per process costs less than threading one timestamp
/// through the metrics map.
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Seconds since a process started: `starttime` is ticks since boot, so it
/// only becomes a wall-clock age against `btime`. Blank rather than 0 whenever
/// a term is missing — a process that started in the future (a stepped clock)
/// is 0 seconds old, not negative.
fn age_secs(starttime_ticks: Option<u64>, clk_tck: u64, btime: u64, now: u64) -> Option<u64> {
    if btime == 0 || clk_tck == 0 || now == 0 {
        return None;
    }
    let started = btime.saturating_add(starttime_ticks? / clk_tck);
    Some(now.saturating_sub(started))
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

/// Advance of the GPU timestamp every xe cycle counter is measured against.
/// Zero means both samples caught the same tick, which is a blank column
/// rather than a division by zero.
fn cycles_span(prev: Option<&Process>, cur: &Process) -> Option<u64> {
    let (a, b) = (prev?.gpu.total_cycles?, cur.gpu.total_cycles?);
    Some(b.saturating_sub(a)).filter(|d| *d > 0)
}

/// xe publishes engine busy as GPU cycles against `drm-total-cycles-<class>`
/// rather than nanoseconds, so its utilisation is that ratio and no wall clock
/// enters it (`drm-total-cycles-<keystr>`, docs.kernel.org/gpu/drm-usage-stats.html).
/// Feeding those
/// cycles to `engine_pct` would print a confident ~0.00% forever.
fn cycles_pct(prev: Option<u64>, cur: Option<u64>, span: Option<u64>) -> Option<f64> {
    let (a, b) = (prev?, cur?);
    let busy = b.saturating_sub(a) as f64;
    Some((busy / span? as f64 * 100.0).clamp(0.0, 100.0))
}

pub(crate) fn host_consts() -> HostHeader {
    HostHeader {
        nproc: nproc(),
        clk_tck: clk_tck(),
        page_size: page_size(),
    }
}

/// The header half of the published tree; `group::build_tree` fills the rest.
pub(crate) fn header_from(c: &HostHeader, a: &HostCpu, b: &HostCpu) -> HostTree {
    let ram = crate::mem::read_ram();
    let gpu = crate::mem::read_gpu();
    let split = host_split(a, b);
    HostTree {
        sampled_at: now_epoch(),
        kernel_threads: crate::proc::kernel_threads(),
        nproc: c.nproc,
        cpu_pct: split.busy,
        cpu_user_pct: split.user,
        cpu_system_pct: split.system,
        cpu_wait_pct: split.wait,
        mem_used_bytes: ram.used_bytes,
        mem_total_bytes: ram.total_bytes,
        mem_buffers_bytes: ram.buffers_bytes,
        mem_cached_bytes: ram.cached_bytes,
        mem_shmem_bytes: ram.shmem_bytes,
        zram_used_bytes: ram.zram_bytes,
        swap_used_bytes: ram.swap_used_bytes,
        swap_total_bytes: ram.swap_total_bytes,
        vram_used_bytes: gpu.vram_used,
        vram_total_bytes: gpu.vram_total,
        unified_memory: crate::mem::is_unified(ram.total_bytes, &gpu),
        ..HostTree::default()
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
    fn xe_engine_pct_is_a_cycle_ratio_not_a_wall_clock_rate() {
        let consts = HostHeader {
            nproc: 8,
            clk_tck: 100,
            page_size: 4096,
        };
        let sample = |busy: u64, stamp: u64| Process {
            gpu: crate::types::GpuCounters {
                gfx_cycles: Some(busy),
                total_cycles: Some(stamp),
                ..crate::types::GpuCounters::default()
            },
            ..Process::default()
        };
        // 250 busy cycles while the GPU clock advanced 1000: a quarter of the
        // engine, whatever the wall-clock gap between the two samples was.
        let (a, b) = (sample(1_000, 5_000), sample(1_250, 6_000));
        for gap in [Duration::from_millis(50), Duration::from_secs(5)] {
            let m = process_metrics(Some(&a), &b, gap, &consts);
            assert!((m.gfx_pct.unwrap() - 25.0).abs() < 1e-9, "{:?}", m.gfx_pct);
        }
        // A clock that did not advance is blank, never 0.00%.
        let m = process_metrics(
            Some(&a),
            &sample(1_250, 5_000),
            Duration::from_secs(1),
            &consts,
        );
        assert_eq!(m.gfx_pct, None);
        // amdgpu / i915 keep the ns path even with cycle fields present.
        let mut ns = b;
        ns.gpu.gfx_ns = Some(500_000_000);
        let mut ns_prev = a;
        ns_prev.gpu.gfx_ns = Some(0);
        let m = process_metrics(Some(&ns_prev), &ns, Duration::from_secs(1), &consts);
        assert!((m.gfx_pct.unwrap() - 50.0).abs() < 1e-9, "{:?}", m.gfx_pct);
    }

    /// The whole point of `btime`: `starttime` is ticks since boot, and
    /// reading it as anything else dates every process to the epoch.
    #[test]
    fn age_is_starttime_against_boot_not_the_epoch() {
        let boot = 1_000_000;
        // 500 ticks at 100 Hz = 5s after boot, read 65s after boot.
        assert_eq!(age_secs(Some(500), 100, boot, boot + 65), Some(60));
        assert_eq!(age_secs(Some(0), 100, boot, boot + 60), Some(60));
        // A blank is not an age of zero: without btime, a tick rate or a
        // starttime there is no figure at all.
        assert_eq!(age_secs(Some(500), 100, 0, boot), None);
        assert_eq!(age_secs(Some(500), 0, boot, boot), None);
        assert_eq!(age_secs(None, 100, boot, boot), None);
        // A clock stepped backwards makes a process 0s old, never negative.
        assert_eq!(age_secs(Some(6000), 100, boot, boot + 10), Some(0));
    }

    #[test]
    fn guest_fields_do_not_inflate_total() {
        let cpu = parse_host_cpu("cpu  10 0 10 80 0 0 0 0 50 25").unwrap();
        assert_eq!(cpu.user, 10);
        assert_eq!(cpu.total, 100);
    }
}
