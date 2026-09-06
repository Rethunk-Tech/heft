use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::types::{GpuCounters, sum_opt};

/// `full_scan` is the PSS / `--once` published pass. Prime and TUI catch-all
/// ticks pass false: dri/drm symlink names are enough, and a first sighting
/// must not walk every fdinfo. Clients whose fd names omit dri/drm show up
/// on the next `full_scan` (up to `--pss-interval`).
pub fn read_pid(pid: u32, full_scan: bool) -> GpuCounters {
    let drm_fds = match drm_fd_nums(pid) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return GpuCounters::default(),
        Err(_) => Vec::new(),
        Ok(v) => v,
    };
    let filtered = if drm_fds.is_empty() {
        GpuCounters::default()
    } else {
        read_fdinfo_files(pid, &drm_fds)
    };
    if !needs_full_fdinfo(full_scan, &filtered) {
        return filtered;
    }
    read_all_fdinfo(pid)
}

/// `/dev/dri/renderD128`, `/dev/dri/card1`, and other drm device nodes.
fn fd_target_looks_like_drm(target: &str) -> bool {
    target.contains("dri") || target.contains("drm")
}

fn has_counters(g: &GpuCounters) -> bool {
    g.vram_bytes.is_some() || g.gtt_bytes.is_some() || g.gfx_ns.is_some() || g.compute_ns.is_some()
}

fn needs_full_fdinfo(full_scan: bool, filtered: &GpuCounters) -> bool {
    full_scan && !has_counters(filtered)
}

fn drm_fd_nums(pid: u32) -> io::Result<Vec<u32>> {
    let mut nums = Vec::new();
    for ent in fs::read_dir(format!("/proc/{pid}/fd"))? {
        let ent = ent?;
        let Ok(link) = fs::read_link(ent.path()) else {
            continue;
        };
        if !fd_target_looks_like_drm(&link.to_string_lossy()) {
            continue;
        }
        let Some(n) = ent.file_name().to_str().and_then(|s| s.parse().ok()) else {
            continue;
        };
        nums.push(n);
    }
    Ok(nums)
}

fn read_fdinfo_files(pid: u32, fds: &[u32]) -> GpuCounters {
    let mut texts = Vec::new();
    for fd in fds {
        push_drm_text(&mut texts, format!("/proc/{pid}/fdinfo/{fd}"));
    }
    merge_fdinfo_texts(&texts)
}

fn read_all_fdinfo(pid: u32) -> GpuCounters {
    let dir = format!("/proc/{pid}/fdinfo");
    let Ok(entries) = fs::read_dir(&dir) else {
        return GpuCounters::default();
    };
    let mut texts = Vec::new();
    for ent in entries.flatten() {
        push_drm_text(&mut texts, ent.path());
    }
    merge_fdinfo_texts(&texts)
}

fn push_drm_text(texts: &mut Vec<String>, path: impl AsRef<Path>) {
    match fs::read_to_string(path) {
        Ok(t) if t.contains("drm-client-id") => texts.push(t),
        _ => {}
    }
}

pub fn parse_fdinfo(text: &str) -> Option<ClientView> {
    let mut driver_ok = false;
    let mut id = None;
    let mut vram = None;
    let mut gtt = None;
    let mut gfx_ns = None;
    let mut compute_ns = None;
    for line in text.lines() {
        let (k, v) = split_kv(line)?;
        // amdgpu, i915 and xe all implement Documentation/gpu/drm-usage-stats.rst
        // but name their regions and engines differently. i915 regions are
        // `<class><instance>` -- "system0" is GPU-visible system memory, "local0"
        // is discrete VRAM (i915/intel_memory_region.c intel_memory_type_str);
        // xe uses "gtt" and "vram0"/"vram1" (xe/xe_bo.c xe_mem_type_to_name).
        // Summing lets a multi-tile xe report both VRAM tiles.
        match k {
            "drm-driver" if matches!(v, "amdgpu" | "i915" | "xe") => driver_ok = true,
            "drm-client-id" => id = v.trim().parse().ok(),
            "drm-resident-vram"
            | "drm-resident-vram0"
            | "drm-resident-vram1"
            | "drm-resident-local0" => vram = sum_opt(vram, parse_size(v)),
            "drm-resident-gtt" | "drm-resident-system0" => gtt = sum_opt(gtt, parse_size(v)),
            // xe publishes engine busy only as drm-cycles-<rcs|ccs|...> against
            // drm-total-cycles-*, never ns, so it has no arm here: feeding cycles
            // to the ns-over-wall-clock rate would render a confidently wrong
            // percentage, which is worse than the blank column it gets instead.
            "drm-engine-gfx" | "drm-engine-render" => gfx_ns = parse_ns(v),
            "drm-engine-compute" => compute_ns = parse_ns(v),
            _ => {}
        }
    }
    if !(driver_ok || (text.contains("drm-client-id") && text.contains("drm-resident"))) {
        return None;
    }
    Some(ClientView {
        id: id?,
        vram,
        gtt,
        gfx_ns,
        compute_ns,
    })
}

pub struct ClientView {
    pub id: u64,
    pub vram: Option<u64>,
    pub gtt: Option<u64>,
    pub gfx_ns: Option<u64>,
    pub compute_ns: Option<u64>,
}

pub fn merge_fdinfo_texts(texts: &[String]) -> GpuCounters {
    let mut by_client: HashMap<u64, ClientView> = HashMap::new();
    for text in texts {
        if let Some(c) = parse_fdinfo(text) {
            by_client.entry(c.id).or_insert(c);
        }
    }
    let mut out = GpuCounters::default();
    for c in by_client.into_values() {
        out.vram_bytes = sum_opt(out.vram_bytes, c.vram);
        out.gtt_bytes = sum_opt(out.gtt_bytes, c.gtt);
        out.gfx_ns = sum_opt(out.gfx_ns, c.gfx_ns);
        out.compute_ns = sum_opt(out.compute_ns, c.compute_ns);
    }
    out
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once(':')?;
    Some((k.trim(), v.trim()))
}

fn parse_size(v: &str) -> Option<u64> {
    let mut parts = v.split_whitespace();
    let n: u64 = parts.next()?.parse().ok()?;
    let unit = parts.next().unwrap_or("B");
    let mul = match unit {
        "KiB" | "kiB" | "kB" | "KB" => 1024,
        "MiB" | "MB" => 1024 * 1024,
        "GiB" | "GB" => 1024 * 1024 * 1024,
        "B" => 1,
        _ => 1024,
    };
    Some(n.saturating_mul(mul))
}

fn parse_ns(v: &str) -> Option<u64> {
    v.split_whitespace().next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "drm-driver:\tamdgpu\ndrm-client-id:\t27\ndrm-resident-vram:\t48596 KiB\ndrm-resident-gtt:\t100 KiB\ndrm-engine-gfx:\t1000 ns\n";

    /// Integrated Intel: shmem-backed "system0" only, no discrete "local0".
    const I915_SAMPLE: &str = "drm-driver:\ti915\ndrm-client-id:\t14\ndrm-pdev:\t0000:00:02.0\ndrm-total-system0:\t8192 KiB\ndrm-shared-system0:\t0\ndrm-resident-system0:\t6144 KiB\ndrm-engine-render:\t2000 ns\ndrm-engine-capacity-render:\t1\ndrm-engine-compute:\t500 ns\n";

    /// Shape taken from the example in drivers/gpu/drm/xe/xe_drm_client.c.
    const XE_SAMPLE: &str = "drm-driver:\txe\ndrm-client-id:\t3\ndrm-pdev:\t0000:03:00.0\ndrm-total-gtt:\t192 KiB\ndrm-resident-gtt:\t192 KiB\ndrm-total-vram0:\t23992 KiB\ndrm-resident-vram0:\t23992 KiB\ndrm-cycles-rcs:\t28257900\ndrm-total-cycles-rcs:\t7655183225\n";

    #[test]
    fn i915_keys_map_onto_the_amdgpu_shape() {
        let g = merge_fdinfo_texts(&[I915_SAMPLE.to_string()]);
        assert_eq!(g.gtt_bytes, Some(6144 * 1024));
        assert_eq!(g.vram_bytes, None, "integrated Intel has no discrete VRAM");
        assert_eq!(g.gfx_ns, Some(2000));
        assert_eq!(g.compute_ns, Some(500));
    }

    #[test]
    fn xe_maps_memory_but_has_no_ns_engine_counter() {
        let g = merge_fdinfo_texts(&[XE_SAMPLE.to_string()]);
        assert_eq!(g.vram_bytes, Some(23992 * 1024));
        assert_eq!(g.gtt_bytes, Some(192 * 1024));
        assert_eq!(g.gfx_ns, None, "drm-cycles-rcs is not nanoseconds");
        assert_eq!(g.compute_ns, None);
    }

    #[test]
    fn dedupe_client_id() {
        let texts = [SAMPLE.to_string(), SAMPLE.to_string()];
        let g = merge_fdinfo_texts(&texts);
        assert_eq!(g.vram_bytes, Some(48596 * 1024));
        assert_eq!(g.gfx_ns, Some(1000));
    }

    #[test]
    fn dri_symlink_prefilter_still_dedupes_client_id() {
        assert!(fd_target_looks_like_drm("/dev/dri/renderD128"));
        assert!(fd_target_looks_like_drm("/dev/dri/card1"));
        assert!(!fd_target_looks_like_drm("/dev/null"));
        assert!(!fd_target_looks_like_drm("socket:[1]"));
        let texts: Vec<String> = ["/dev/dri/renderD128", "/dev/null", "/dev/dri/card1"]
            .into_iter()
            .filter(|t| fd_target_looks_like_drm(t))
            .map(|_| SAMPLE.to_string())
            .collect();
        assert_eq!(texts.len(), 2);
        let g = merge_fdinfo_texts(&texts);
        assert_eq!(g.vram_bytes, Some(48596 * 1024));
        assert_eq!(g.gfx_ns, Some(1000));
    }

    #[test]
    fn full_fdinfo_only_when_asked_and_prefilter_empty() {
        let empty = GpuCounters::default();
        let live = GpuCounters {
            vram_bytes: Some(1),
            ..GpuCounters::default()
        };
        assert!(!needs_full_fdinfo(false, &empty));
        assert!(!needs_full_fdinfo(false, &live));
        assert!(needs_full_fdinfo(true, &empty));
        assert!(!needs_full_fdinfo(true, &live));
    }
}
