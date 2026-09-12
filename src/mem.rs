use std::fs;
use std::path::Path;

use crate::proc::field_u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RamInfo {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub buffers_bytes: u64,
    pub cached_bytes: u64,
    /// tmpfs and shared memory: the part of `Cached` that `MemAvailable` cannot
    /// reclaim, and so the only cache that is inside `used`.
    pub shmem_bytes: u64,
    /// `SUnreclaim` + `PageTables` + `KernelStack`: the kernel's own memory
    /// that `MemAvailable` cannot count as free, so it is inside `used`.
    pub kernel_bytes: u64,
    /// Reclaimable slab, which `MemAvailable` counts as free the way it counts
    /// page cache; drawn with the cache, beyond `used`.
    pub sreclaimable_bytes: u64,
    /// RAM held by every zram device's compressed store (`mm_stat`
    /// `mem_used_total`). Kernel memory that no process's PSS carries and no
    /// `/proc/meminfo` line names, so on a zram host it is most of the gap
    /// between `used` and anything the tree can account for.
    pub zram_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GpuPool {
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub gtt_total: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MemParts {
    pub used: u64,
    pub total: u64,
    pub vram: u64,
    pub gtt: u64,
    pub zram: u64,
    pub shmem: u64,
    pub kernel: u64,
    /// Reclaimable page cache, buffers and slab, which sit outside `used`.
    pub cache: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MemSegments {
    pub vram: u64,
    pub gtt: u64,
    pub zram: u64,
    pub shmem: u64,
    pub kernel: u64,
    pub anon: u64,
    pub cache: u64,
}

pub(crate) fn read_ram() -> RamInfo {
    let Ok(text) = fs::read_to_string(crate::root::path("/proc/meminfo")) else {
        return RamInfo::default();
    };
    RamInfo {
        zram_bytes: read_zram(),
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

pub(crate) fn parse_meminfo(text: &str) -> RamInfo {
    let mut total = 0u64;
    let mut avail = 0u64;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    let mut shmem = 0u64;
    let mut sreclaimable = 0u64;
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
    RamInfo {
        used_bytes: total.saturating_sub(avail),
        total_bytes: total,
        buffers_bytes: buffers,
        cached_bytes: cached,
        shmem_bytes: shmem,
        kernel_bytes: kernel,
        sreclaimable_bytes: sreclaimable,
        zram_bytes: 0,
        // `SwapCached:` is swapped-out pages that also still sit in RAM, so it
        // is neither free swap nor a separate tank: total - free is what is
        // actually out on disk.
        swap_used_bytes: swap_total.saturating_sub(swap_free),
        swap_total_bytes: swap_total,
    }
}

pub(crate) fn read_gpu() -> GpuPool {
    let Ok(entries) = fs::read_dir(crate::root::path("/sys/class/drm")) else {
        return GpuPool::default();
    };
    let mut vram_used = 0u64;
    let mut vram_total = 0u64;
    let mut gtt_total = 0u64;
    let mut vram_found = false;
    let mut gtt_found = false;
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
            vram_total = vram_total.saturating_add(t);
            vram_used = vram_used.saturating_add(u);
            vram_found = true;
        }
        if let Some(t) = read_u64(&dir.join("mem_info_gtt_total")) {
            gtt_total = gtt_total.saturating_add(t);
            gtt_found = true;
        }
    }
    GpuPool {
        vram_used: vram_found.then_some(vram_used),
        vram_total: vram_found.then_some(vram_total),
        gtt_total: gtt_found.then_some(gtt_total),
    }
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

/// Paint VRAM, GTT, zram, shmem and kernel inside `used`, `anon` as the rest
/// of it, then reclaimable `cache` beyond `used`. Each is clipped to the room
/// the ones before it left, so the sum never exceeds `total`.
///
/// Reclaimable cache is never inside `used`. `used` is `MemTotal -
/// MemAvailable`, and `MemAvailable` already counts reclaimable page cache as
/// free, so painting it inside `used` claimed room that was process memory:
/// measured on a 125 GiB host, 18.4 GiB of `Cached` drawn inside 52.7 GiB used
/// left anon at about 14 GiB against `AnonPages` 23.7 GiB. Shmem is the cache
/// `MemAvailable` cannot reclaim, so it is the slice that is actually there.
pub(crate) fn clip_used(p: MemParts) -> MemSegments {
    if p.total == 0 {
        return MemSegments::default();
    }
    let used = p.used.min(p.total);
    let mut left = used;
    let mut take = |n: u64| {
        let t = n.min(left);
        left -= t;
        t
    };
    let vram = take(p.vram);
    let gtt = take(p.gtt);
    let zram = take(p.zram);
    let shmem = take(p.shmem);
    let kernel = take(p.kernel);
    MemSegments {
        vram,
        gtt,
        zram,
        shmem,
        kernel,
        anon: left,
        cache: p.cache.min(p.total - used),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_used() {
        let ram = parse_meminfo(
            "MemTotal: 1000 kB\nMemAvailable: 400 kB\nBuffers: 10 kB\nCached: 50 kB\nSwapCached: 7 kB\nShmem: 20 kB\nShmemHugePages: 9 kB\nSReclaimable: 5 kB\nSUnreclaim: 3 kB\nPageTables: 2 kB\nSecPageTables: 99 kB\nKernelStack: 1 kB\nSwapTotal: 800 kB\nSwapFree: 300 kB\n",
        );
        assert_eq!(ram.total_bytes, 1000 * 1024);
        assert_eq!(ram.used_bytes, 600 * 1024);
        assert_eq!(ram.buffers_bytes, 10 * 1024);
        assert_eq!(ram.cached_bytes, 50 * 1024);
        assert_eq!(ram.shmem_bytes, 20 * 1024);
        assert_eq!(ram.sreclaimable_bytes, 5 * 1024);
        // SecPageTables is a different line that merely ends in the same name.
        assert_eq!(ram.kernel_bytes, 6 * 1024);
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

    fn inside(s: &MemSegments) -> u64 {
        s.vram + s.gtt + s.zram + s.shmem + s.kernel + s.anon
    }

    #[test]
    fn clip_gpu_when_vram_gtt_exceed_used() {
        let s = clip_used(MemParts {
            used: 100,
            total: 200,
            vram: 80,
            gtt: 50,
            zram: 40,
            shmem: 10,
            kernel: 5,
            cache: 30,
        });
        assert_eq!(
            s,
            MemSegments {
                vram: 80,
                gtt: 20,
                zram: 0,
                shmem: 0,
                kernel: 0,
                anon: 0,
                cache: 30,
            }
        );
        assert_eq!(inside(&s), 100);
    }

    #[test]
    fn clip_zram_shmem_kernel_after_gpu() {
        let s = clip_used(MemParts {
            used: 100,
            total: 200,
            vram: 10,
            gtt: 10,
            zram: 60,
            shmem: 15,
            kernel: 50,
            cache: 0,
        });
        assert_eq!((s.zram, s.shmem, s.kernel, s.anon), (60, 15, 5, 0));
    }

    /// Cache is what `MemAvailable` counts as free, so it is drawn beyond
    /// `used` and only into the room `total` has left there.
    #[test]
    fn clip_never_exceeds_memtotal() {
        let s = clip_used(MemParts {
            used: 500,
            total: 100,
            vram: 40,
            gtt: 30,
            zram: 20,
            shmem: 10,
            kernel: 0,
            cache: 50,
        });
        assert_eq!(inside(&s), 100);
        assert_eq!((s.anon, s.cache), (0, 0));
        let s = clip_used(MemParts {
            used: 60,
            total: 100,
            cache: 70,
            ..MemParts::default()
        });
        assert_eq!((s.anon, s.cache), (60, 40));
    }

    /// The reported host: 17.3 GiB used, 5.2 GiB of PSS, 10 GiB out on zram.
    /// The store is RAM inside `used`, so it is a segment of its own rather
    /// than more of the unlabelled remainder read as process memory.
    #[test]
    fn clip_anon_is_used_remainder() {
        let s = clip_used(MemParts {
            used: 100,
            total: 200,
            vram: 5,
            gtt: 15,
            zram: 20,
            shmem: 10,
            kernel: 8,
            cache: 0,
        });
        assert_eq!(s.anon, 42);
        assert_eq!(inside(&s), 100);
    }
}
