use std::fs;
use std::path::Path;

use crate::proc::field_u64;
use crate::types::{HostTree, sum_opt};

/// The sysfs GPU memory counters. Not `HostTree` fields alone: `gtt_total` is
/// read only to decide `is_unified`, and the header never prints it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GpuPool {
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub gtt_total: Option<u64>,
    /// `mem_info_gtt_used`, the kernel's own count. amdgpu publishes it; i915
    /// and xe do not, and there it is `None`.
    pub gtt_used: Option<u64>,
}

/// The header's memory and swap fields; every other field is left default.
pub(crate) fn read_ram() -> HostTree {
    let Ok(text) = fs::read_to_string(crate::root::path("/proc/meminfo")) else {
        return HostTree::default();
    };
    HostTree {
        zram_used_bytes: read_zram(),
        ..parse_meminfo(&text)
    }
}

/// `mem_used_total` summed over every zram device; a missing or unconfigured
/// device reads 0.
fn read_zram() -> u64 {
    let Ok(entries) = fs::read_dir(crate::root::path("/sys/block")) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("zram"))
        .filter_map(|e| fs::read_to_string(e.path().join("mm_stat")).ok())
        .filter_map(|s| zram_used(&s))
        .fold(0, u64::saturating_add)
}

/// The third `mm_stat` field, `mem_used_total`, in bytes: the compressed data
/// plus allocator overhead, which is what the device costs in RAM
/// (docs.kernel.org/admin-guide/blockdev/zram).
fn zram_used(mm_stat: &str) -> Option<u64> {
    mm_stat.split_whitespace().nth(2)?.parse().ok()
}

pub(crate) fn parse_meminfo(text: &str) -> HostTree {
    let mut total = 0u64;
    let mut avail = 0u64;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    let mut shmem = 0u64;
    let mut sreclaimable = 0u64;
    let mut anon = 0u64;
    let mut kernel = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;
    for line in text.lines() {
        if let Some(v) = field_u64(line, "MemTotal:") {
            total = v.saturating_mul(1024);
        } else if let Some(v) = field_u64(line, "MemAvailable:") {
            avail = v.saturating_mul(1024);
        } else if let Some(v) = field_u64(line, "Buffers:") {
            buffers = v.saturating_mul(1024);
        } else if let Some(v) = field_u64(line, "Cached:") {
            cached = v.saturating_mul(1024);
        } else if let Some(v) = field_u64(line, "Shmem:") {
            shmem = v.saturating_mul(1024);
        } else if let Some(v) = field_u64(line, "SReclaimable:") {
            sreclaimable = v.saturating_mul(1024);
        } else if let Some(v) = field_u64(line, "AnonPages:") {
            anon = v.saturating_mul(1024);
        } else if let Some(v) = ["SUnreclaim:", "PageTables:", "KernelStack:"]
            .iter()
            .find_map(|k| field_u64(line, k))
        {
            kernel = kernel.saturating_add(v.saturating_mul(1024));
        } else if let Some(v) = field_u64(line, "SwapTotal:") {
            swap_total = v.saturating_mul(1024);
        } else if let Some(v) = field_u64(line, "SwapFree:") {
            swap_free = v.saturating_mul(1024);
        }
    }
    HostTree {
        mem_used_bytes: total.saturating_sub(avail),
        mem_total_bytes: total,
        mem_buffers_bytes: buffers,
        mem_cached_bytes: cached,
        mem_shmem_bytes: shmem,
        mem_anon_bytes: anon,
        mem_kernel_bytes: kernel,
        mem_sreclaimable_bytes: sreclaimable,
        // `SwapCached:` is swapped-out pages that also still sit in RAM, so it
        // is neither free swap nor a separate tank: total - free is what is
        // actually out on disk.
        swap_used_bytes: swap_total.saturating_sub(swap_free),
        swap_total_bytes: swap_total,
        ..HostTree::default()
    }
}

pub(crate) fn read_gpu() -> GpuPool {
    let Ok(entries) = fs::read_dir(crate::root::path("/sys/class/drm")) else {
        return GpuPool::default();
    };
    let mut pool = GpuPool::default();
    for ent in entries.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dir = ent.path().join("device");
        if let Some(t) = read_u64(&dir.join("mem_info_vram_total"))
            && let Some(u) = read_u64(&dir.join("mem_info_vram_used"))
        {
            pool.vram_total = sum_opt(pool.vram_total, Some(t));
            pool.vram_used = sum_opt(pool.vram_used, Some(u));
        }
        pool.gtt_total = sum_opt(pool.gtt_total, read_u64(&dir.join("mem_info_gtt_total")));
        pool.gtt_used = sum_opt(pool.gtt_used, read_u64(&dir.join("mem_info_gtt_used")));
    }
    pool
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// APU / unified: sysfs VRAM is a carve-out of `MemTotal` (GTT covers most of RAM),
/// not a second device. False means discrete, which `ui::mem_header_line` renders
/// as its own tank against `vram_total` rather than folding it into `MemTotal`.
pub(crate) fn is_unified(mem_total: u64, gpu: &GpuPool) -> bool {
    let Some(vram) = gpu.vram_total.filter(|v| *v > 0) else {
        return false;
    };
    if mem_total == 0 || vram >= mem_total {
        return false;
    }
    if gpu.gtt_total.is_some_and(|g| g >= mem_total / 2) {
        return true;
    }
    vram.saturating_mul(8) < mem_total
}

/// Paint VRAM, GTT, zram, shmem, kernel and anon inside `used`, `other` as
/// whatever of `used` none of them itemise, then reclaimable cache, slab and
/// buffers beyond `used`. Each is clipped to the room the ones before it left,
/// so the sum never exceeds `total`.
///
/// Reclaimable cache is never inside `used`. `used` is `MemTotal -
/// MemAvailable`, and `MemAvailable` already counts reclaimable page cache as
/// free, so painting it inside `used` claimed room that was process memory:
/// measured on a 125 GiB host, 18.4 GiB of `Cached` drawn inside 52.7 GiB used
/// left anon at about 14 GiB against `AnonPages` 23.7 GiB. Shmem is the cache
/// `MemAvailable` cannot reclaim, so it is the slice that is actually there.
///
/// `inside` is vram, gtt, zram, shmem, kernel, anon; `beyond` is reclaimable
/// cache (`Cached` - `Shmem`), slab, buffers. The result is those ten with
/// `other` after anon, which is the order `ui::mem_key` draws them in.
pub(crate) fn clip_used(used: u64, total: u64, inside: [u64; 6], beyond: [u64; 3]) -> [u64; 10] {
    if total == 0 {
        return [0; 10];
    }
    let used = used.min(total);
    let ([vram, gtt, zram, shmem, kernel, anon], other) = fill(used, inside);
    let ([cache, slab, buffers], _) = fill(total - used, beyond);
    [
        vram, gtt, zram, shmem, kernel, anon, other, cache, slab, buffers,
    ]
}

/// Each want clipped to the room the ones before it left, and what is left.
fn fill<const N: usize>(room: u64, wants: [u64; N]) -> ([u64; N], u64) {
    let mut left = room;
    let got = wants.map(|n| {
        let t = n.min(left);
        left -= t;
        t
    });
    (got, left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_used() {
        let ram = parse_meminfo(
            "MemTotal: 1000 kB\nMemAvailable: 400 kB\nBuffers: 10 kB\nCached: 50 kB\nSwapCached: 7 kB\nShmem: 20 kB\nShmemHugePages: 9 kB\nSReclaimable: 5 kB\nAnonPages: 30 kB\nSUnreclaim: 3 kB\nPageTables: 2 kB\nSecPageTables: 99 kB\nKernelStack: 1 kB\nSwapTotal: 800 kB\nSwapFree: 300 kB\n",
        );
        assert_eq!(ram.mem_total_bytes, 1000 * 1024);
        assert_eq!(ram.mem_used_bytes, 600 * 1024);
        assert_eq!(ram.mem_buffers_bytes, 10 * 1024);
        assert_eq!(ram.mem_cached_bytes, 50 * 1024);
        assert_eq!(ram.mem_shmem_bytes, 20 * 1024);
        assert_eq!(ram.mem_sreclaimable_bytes, 5 * 1024);
        assert_eq!(ram.mem_anon_bytes, 30 * 1024);
        // SecPageTables is a different line that merely ends in the same name.
        assert_eq!(ram.mem_kernel_bytes, 6 * 1024);
        assert_eq!(ram.swap_total_bytes, 800 * 1024);
        assert_eq!(ram.swap_used_bytes, 500 * 1024);
    }

    /// The swapless machine this was written on: every swap key reads 0, and
    /// the header must then render as if swap did not exist at all.
    #[test]
    fn swapless_meminfo_is_zero_not_garbage() {
        let ram = parse_meminfo("MemTotal: 1000 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n");
        assert_eq!(ram.swap_total_bytes, 0);
        assert_eq!(ram.swap_used_bytes, 0);
    }

    #[test]
    fn unified_when_gtt_covers_ram() {
        let gpu = GpuPool {
            vram_total: Some(512 * 1024 * 1024),
            gtt_total: Some(120 * 1024 * 1024 * 1024),
            ..GpuPool::default()
        };
        assert!(is_unified(125 * 1024 * 1024 * 1024, &gpu));
    }

    #[test]
    fn discrete_vram_is_not_a_carve_out() {
        let gpu = GpuPool {
            vram_total: Some(12 * 1024 * 1024 * 1024),
            gtt_total: Some(4 * 1024 * 1024 * 1024),
            ..GpuPool::default()
        };
        assert!(!is_unified(32 * 1024 * 1024 * 1024, &gpu));
    }

    #[test]
    fn zram_mm_stat_third_field_is_ram_used() {
        assert_eq!(zram_used("4096 1024 8192 0 8192 0 0 0 0\n"), Some(8192));
        assert_eq!(zram_used("0 0 0 0 0 0 0 0 0"), Some(0));
        assert_eq!(zram_used("garbage"), None);
    }

    // `clip_used` results index as vram 0, gtt 1, zram 2, shmem 3, kernel 4,
    // anon 5, other 6, cache 7, slab 8, buffers 9.
    fn inside(s: &[u64; 10]) -> u64 {
        s[..7].iter().sum()
    }

    #[test]
    fn clip_gpu_when_vram_gtt_exceed_used() {
        let s = clip_used(100, 200, [80, 50, 40, 10, 5, 20], [30, 5, 1]);
        assert_eq!(s, [80, 20, 0, 0, 0, 0, 0, 30, 5, 1]);
        assert_eq!(inside(&s), 100);
    }

    #[test]
    fn clip_zram_shmem_kernel_anon_after_gpu() {
        let s = clip_used(100, 200, [10, 10, 60, 15, 50, 40], [0; 3]);
        assert_eq!(s[2..7], [60, 15, 5, 0, 0]);
    }

    /// Cache, slab and buffers are what `MemAvailable` counts as free, so they
    /// are drawn beyond `used` and only into the room `total` has left there.
    #[test]
    fn clip_never_exceeds_memtotal() {
        let s = clip_used(500, 100, [40, 30, 20, 10, 0, 0], [50, 0, 0]);
        assert_eq!(inside(&s), 100);
        assert_eq!((s[6], s[7]), (0, 0));
        let s = clip_used(60, 100, [0; 6], [30, 20, 5]);
        assert_eq!(s[6..], [60, 30, 10, 0]);
    }

    /// The reported host: 17.3 GiB used, 5.2 GiB of PSS, 10 GiB out on zram.
    /// Every figure the kernel itemises gets its own segment, and `other` is
    /// only what none of them name, rather than a remainder read as anon.
    #[test]
    fn clip_other_is_the_unitemised_rest() {
        let s = clip_used(100, 200, [5, 15, 20, 10, 8, 30], [0; 3]);
        assert_eq!((s[5], s[6]), (30, 12));
        assert_eq!(inside(&s), 100);
    }
}
