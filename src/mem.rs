use std::fs;
use std::path::Path;

use crate::proc::field_u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RamInfo {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub buffers_bytes: u64,
    pub cached_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuPool {
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub gtt_total: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemParts {
    pub used: u64,
    pub total: u64,
    pub vram: u64,
    pub gtt: u64,
    pub cache: u64,
    pub buffers: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemSegments {
    pub vram: u64,
    pub gtt: u64,
    pub cache: u64,
    pub buffers: u64,
    pub anon: u64,
}

pub fn read_ram() -> RamInfo {
    let Ok(text) = fs::read_to_string("/proc/meminfo") else {
        return RamInfo::default();
    };
    parse_meminfo(&text)
}

pub fn parse_meminfo(text: &str) -> RamInfo {
    let mut total = 0u64;
    let mut avail = 0u64;
    let mut buffers = 0u64;
    let mut cached = 0u64;
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
        // `SwapCached:` is swapped-out pages that also still sit in RAM, so it
        // is neither free swap nor a separate tank: total - free is what is
        // actually out on disk.
        swap_used_bytes: swap_total.saturating_sub(swap_free),
        swap_total_bytes: swap_total,
    }
}

pub fn read_gpu() -> GpuPool {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
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
pub fn is_unified(mem_total: u64, gpu: &GpuPool) -> bool {
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

/// Paint VRAM, GTT, cache, buffers, then anon inside `used`. Clip GPU first when
/// vram+gtt exceed used; clip cache/buffers afterward. Sum never exceeds
/// `used.min(total)`.
pub fn clip_used(p: MemParts) -> MemSegments {
    if p.total == 0 {
        return MemSegments::default();
    }
    let used = p.used.min(p.total);
    let vram = p.vram.min(used);
    let gtt = p.gtt.min(used.saturating_sub(vram));
    let after_gpu = used.saturating_sub(vram).saturating_sub(gtt);
    let cache = p.cache.min(after_gpu);
    let buffers = p.buffers.min(after_gpu.saturating_sub(cache));
    let anon = after_gpu.saturating_sub(cache).saturating_sub(buffers);
    MemSegments {
        vram,
        gtt,
        cache,
        buffers,
        anon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_used() {
        let ram = parse_meminfo(
            "MemTotal: 1000 kB\nMemAvailable: 400 kB\nBuffers: 10 kB\nCached: 50 kB\nSwapCached: 7 kB\nSwapTotal: 800 kB\nSwapFree: 300 kB\n",
        );
        assert_eq!(ram.total_bytes, 1000 * 1024);
        assert_eq!(ram.used_bytes, 600 * 1024);
        assert_eq!(ram.buffers_bytes, 10 * 1024);
        assert_eq!(ram.cached_bytes, 50 * 1024);
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
    fn clip_gpu_when_vram_gtt_exceed_used() {
        let s = clip_used(MemParts {
            used: 100,
            total: 200,
            vram: 80,
            gtt: 50,
            cache: 40,
            buffers: 10,
        });
        assert_eq!(
            s,
            MemSegments {
                vram: 80,
                gtt: 20,
                cache: 0,
                buffers: 0,
                anon: 0,
            }
        );
        assert_eq!(s.vram + s.gtt + s.cache + s.buffers + s.anon, 100);
    }

    #[test]
    fn clip_cache_buffers_after_gpu() {
        let s = clip_used(MemParts {
            used: 100,
            total: 200,
            vram: 10,
            gtt: 10,
            cache: 200,
            buffers: 50,
        });
        assert_eq!(
            s,
            MemSegments {
                vram: 10,
                gtt: 10,
                cache: 80,
                buffers: 0,
                anon: 0,
            }
        );
    }

    #[test]
    fn clip_never_exceeds_memtotal() {
        let s = clip_used(MemParts {
            used: 500,
            total: 100,
            vram: 40,
            gtt: 30,
            cache: 20,
            buffers: 10,
        });
        let sum = s.vram + s.gtt + s.cache + s.buffers + s.anon;
        assert_eq!(sum, 100);
        assert_eq!(s.anon, 0);
    }

    #[test]
    fn clip_anon_is_used_remainder() {
        let s = clip_used(MemParts {
            used: 100,
            total: 200,
            vram: 5,
            gtt: 15,
            cache: 20,
            buffers: 10,
        });
        assert_eq!(s.anon, 50);
        assert_eq!(s.vram + s.gtt + s.cache + s.buffers + s.anon, 100);
    }
}
