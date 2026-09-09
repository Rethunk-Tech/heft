use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::types::{GpuCounters, sum_opt};

/// Dri/drm symlink names select which fdinfo to read on every tick.
/// `full_scan` (PSS / `--once`) used to walk every fdinfo when that
/// prefilter was empty, so a GPU client whose fd name omitted dri/drm still
/// appeared. That walk was ~318 ms of ~760 ms serial PSS-tick kernel work
/// on 308 pids with no dri/drm fd; those clients now stay blank. A residual
/// full walk still runs when the prefilter found fds but they yielded no
/// GPU metrics.
pub(crate) fn read_pid(pid: u32, full_scan: bool) -> GpuCounters {
    let drm_fds = match drm_fd_nums(pid) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return GpuCounters::default(),
        Err(_) => Vec::new(),
        Ok(v) => v,
    };
    let prefilter_empty = drm_fds.is_empty();
    let filtered = if prefilter_empty {
        GpuCounters::default()
    } else {
        read_fdinfo_files(pid, &drm_fds)
    };
    if !needs_full_fdinfo(full_scan, prefilter_empty, &filtered) {
        return filtered;
    }
    read_all_fdinfo(pid)
}

/// `/dev/dri/renderD128`, `/dev/dri/card1`, and other drm device nodes.
fn fd_target_looks_like_drm(target: &str) -> bool {
    target.contains("dri") || target.contains("drm")
}

fn needs_full_fdinfo(full_scan: bool, prefilter_empty: bool, filtered: &GpuCounters) -> bool {
    full_scan
        && !prefilter_empty
        && filtered.vram_bytes.is_none()
        && filtered.gtt_bytes.is_none()
        && filtered.gfx_ns.is_none()
        && filtered.compute_ns.is_none()
}

fn drm_fd_nums(pid: u32) -> io::Result<Vec<u32>> {
    let mut nums = Vec::new();
    for ent in fs::read_dir(format!("{}/proc/{pid}/fd", crate::root::prefix()))? {
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
        push_drm_text(
            &mut texts,
            format!("{}/proc/{pid}/fdinfo/{fd}", crate::root::prefix()),
        );
    }
    merge_fdinfo_texts(&texts)
}

fn read_all_fdinfo(pid: u32) -> GpuCounters {
    let dir = format!("{}/proc/{pid}/fdinfo", crate::root::prefix());
    let Ok(entries) = fs::read_dir(&dir) else {
        return GpuCounters::default();
    };
    let mut texts = Vec::new();
    for ent in entries.flatten() {
        push_drm_text(&mut texts, ent.path());
    }
    merge_fdinfo_texts(&texts)
}

/// Observed drm fdinfo on this host: vivaldi max 7 KiB, cursor 14 KiB.
/// `localsearch-3` has a 16_038_344-byte `anon_inode:[fanotify]` fdinfo
/// with 0 `drm-client-id`. 64 KiB is well above real drm and well below
/// that dump. `/proc/<pid>/fdinfo/*` often reports `st_size` 0 (measured
/// 0 on `/proc/self/fdinfo/0`), so a metadata cap alone would not skip
/// the fanotify file; `Read::take` enforces the same bound after open.
const FDINFO_MAX_BYTES: u64 = 64 * 1024;

fn push_drm_text(texts: &mut Vec<String>, path: impl AsRef<Path>) {
    let Ok(file) = fs::File::open(path) else {
        return;
    };
    if file.metadata().is_ok_and(|m| m.len() > FDINFO_MAX_BYTES) {
        return;
    }
    let mut buf = String::new();
    let mut limited = file.take(FDINFO_MAX_BYTES + 1);
    match limited.read_to_string(&mut buf) {
        Ok(n) if n as u64 > FDINFO_MAX_BYTES => {}
        Ok(_) if buf.contains("drm-client-id") => texts.push(buf),
        _ => {}
    }
}

fn parse_fdinfo(text: &str) -> Option<(u64, GpuCounters)> {
    let mut driver_ok = false;
    let mut id = None;
    let mut vram = None;
    let mut gtt = None;
    let mut gfx_ns = None;
    let mut compute_ns = None;
    let mut gfx_cycles = None;
    let mut compute_cycles = None;
    let mut total_cycles = None;
    let mut gfx_capacity = 1;
    let mut compute_capacity = 1;
    for line in text.lines() {
        // A line that is not `key: value` is skipped, not fatal: `?` here
        // discarded every key already read and lost the whole client, where
        // everything else in heft renders what it could not read as blank.
        let Some((k, v)) = split_kv(line) else {
            continue;
        };
        // amdgpu, i915 and xe all implement the same DRM fdinfo interface
        // (docs.kernel.org/gpu/drm-usage-stats.html)
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
            "drm-engine-gfx" | "drm-engine-render" => gfx_ns = parse_count(v),
            "drm-engine-compute" => compute_ns = parse_count(v),
            // xe publishes engine busy only as drm-cycles-<rcs|ccs|...> against
            // drm-total-cycles-*, never ns, so it takes the ratio path in
            // `cpu::cycles_pct` instead of the ns-over-wall-clock rate.
            "drm-cycles-rcs" => gfx_cycles = parse_count(v),
            "drm-cycles-ccs" => compute_cycles = parse_count(v),
            // One `xe_hw_engine_read_timestamp` read is printed under every
            // class (xe/xe_drm_client.c `show_run_ticks`), so these are the
            // same number and the last one parsed is as good as the first.
            "drm-total-cycles-rcs" | "drm-total-cycles-ccs" => total_cycles = parse_count(v),
            "drm-engine-capacity-rcs" => gfx_capacity = parse_capacity(v),
            "drm-engine-capacity-ccs" => compute_capacity = parse_capacity(v),
            _ => {}
        }
    }
    if !(driver_ok || (text.contains("drm-client-id") && text.contains("drm-resident"))) {
        return None;
    }
    Some((
        id?,
        GpuCounters {
            vram_bytes: vram,
            gtt_bytes: gtt,
            gfx_ns,
            compute_ns,
            // xe sums run_ticks over every engine instance of the class while
            // drm-total-cycles is one clock, so capacity is the divisor
            // (docs.kernel.org/gpu/drm-usage-stats.html; xe/xe_drm_client.c `show_run_ticks` prints
            // it only when it exceeds one). Dividing here rather than carrying
            // capacity to the rate costs under one count in the ~1e10 that a
            // second of GPU timestamp spans.
            gfx_cycles: gfx_cycles.map(|c| c / gfx_capacity),
            compute_cycles: compute_cycles.map(|c| c / compute_capacity),
            total_cycles,
        },
    ))
}

fn merge_fdinfo_texts(texts: &[String]) -> GpuCounters {
    let mut by_client: HashMap<u64, GpuCounters> = HashMap::new();
    for text in texts {
        if let Some((id, c)) = parse_fdinfo(text) {
            by_client.entry(id).or_insert(c);
        }
    }
    let mut out = GpuCounters::default();
    for c in by_client.into_values() {
        out.vram_bytes = sum_opt(out.vram_bytes, c.vram_bytes);
        out.gtt_bytes = sum_opt(out.gtt_bytes, c.gtt_bytes);
        out.gfx_ns = sum_opt(out.gfx_ns, c.gfx_ns);
        out.compute_ns = sum_opt(out.compute_ns, c.compute_ns);
        out.gfx_cycles = sum_opt(out.gfx_cycles, c.gfx_cycles);
        out.compute_cycles = sum_opt(out.compute_cycles, c.compute_cycles);
        // A device clock, not a quantity: summing it would divide the busy
        // cycles by one clock per open client.
        out.total_cycles = out.total_cycles.max(c.total_cycles);
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

fn parse_count(v: &str) -> Option<u64> {
    v.split_whitespace().next()?.parse().ok()
}

/// The DRM fdinfo spec (docs.kernel.org/gpu/drm-usage-stats.html) forbids
/// a zero capacity and says an absent tag means
/// one, so an unparsable value falls back the same way rather than dividing
/// the busy cycles away.
fn parse_capacity(v: &str) -> u64 {
    parse_count(v).unwrap_or(1).max(1)
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
    fn a_line_without_a_colon_does_not_lose_the_client() {
        let mut text = SAMPLE.to_string();
        text.insert_str(0, "not a key value line\n");
        let g = merge_fdinfo_texts(&[text]);
        assert_eq!(g.vram_bytes, Some(48596 * 1024));
        assert_eq!(g.gtt_bytes, Some(100 * 1024));
        assert_eq!(g.gfx_ns, Some(1000));
    }

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

    /// Two rcs-and-ccs samples of one xe client. ccs capacity 4 is the shape
    /// the kernel doc's worked example shows on an Arc part; rcs prints no
    /// capacity line, which means one.
    fn xe_tick(rcs: u64, ccs: u64, stamp: u64) -> String {
        format!(
            "drm-driver:\txe\ndrm-client-id:\t3\ndrm-resident-gtt:\t192 KiB\n\
             drm-cycles-rcs:\t{rcs}\ndrm-total-cycles-rcs:\t{stamp}\n\
             drm-cycles-ccs:\t{ccs}\ndrm-total-cycles-ccs:\t{stamp}\n\
             drm-engine-capacity-ccs:\t4\n"
        )
    }

    #[test]
    fn xe_cycles_divide_by_capacity_and_keep_one_device_clock() {
        let g = merge_fdinfo_texts(&[xe_tick(400, 4000, 9_000)]);
        assert_eq!(g.gfx_cycles, Some(400), "rcs capacity is 1");
        assert_eq!(g.compute_cycles, Some(1000), "4 ccs instances, one clock");
        assert_eq!(g.total_cycles, Some(9_000));
        // Two clients on one device: busy cycles add, the clock does not.
        let mut two = xe_tick(400, 4000, 9_000);
        two.push_str("drm-client-id:\t4\n");
        let g = merge_fdinfo_texts(&[xe_tick(400, 4000, 9_000), two]);
        assert_eq!(g.gfx_cycles, Some(800));
        assert_eq!(g.total_cycles, Some(9_000));
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
        assert!(!needs_full_fdinfo(false, true, &empty));
        assert!(!needs_full_fdinfo(false, false, &live));
        // Empty dri/drm prefilter never full-walks, including PSS / --once.
        assert!(!needs_full_fdinfo(true, true, &empty));
        assert!(!needs_full_fdinfo(true, true, &live));
        assert!(!needs_full_fdinfo(true, false, &live));
        // Residual: names were found but they yielded no GPU metrics.
        assert!(needs_full_fdinfo(true, false, &empty));
    }

    #[test]
    fn oversized_fdinfo_is_not_slurped() {
        let dir = std::env::temp_dir();
        let tag = std::process::id();
        let huge = dir.join(format!("heft-fdinfo-huge-{tag}"));
        let small = dir.join(format!("heft-fdinfo-small-{tag}"));
        fs::write(&huge, vec![b'x'; (FDINFO_MAX_BYTES as usize) + 1]).unwrap();
        fs::write(&small, SAMPLE).unwrap();
        let mut texts = Vec::new();
        push_drm_text(&mut texts, &huge);
        assert!(
            texts.is_empty(),
            "oversize without drm-client-id is skipped"
        );
        push_drm_text(&mut texts, &small);
        assert_eq!(texts.len(), 1);
        let g = merge_fdinfo_texts(&texts);
        assert_eq!(g.vram_bytes, Some(48596 * 1024));
        let _ = fs::remove_file(&huge);
        let _ = fs::remove_file(&small);
    }
}
