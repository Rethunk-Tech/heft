use std::collections::HashMap;
use std::ffi::CStr;
use std::io::{self, Write};
use std::os::fd::AsFd;

use rustix::fs::{Dir, Mode, OFlags};
use rustix::io::Errno;

use crate::proc::{atoi, read_at, read_link_at};
use crate::types::{GpuCounters, sum_opt};

/// Dri/drm symlink names select which fdinfo to read on every tick. When
/// that prefilter is empty, `full_scan` (PSS / `--once`) does not walk every
/// fdinfo, so a GPU client whose fd name omits dri/drm stays blank: that walk
/// measured ~318 ms of ~760 ms serial PSS-tick kernel work on 308 pids with no
/// dri/drm fd. A full walk still runs when the prefilter finds fds but they
/// yield no GPU metrics. `dir` is the process's `/proc/<pid>`.
///
/// `carried` is the prefilter from an earlier tick, used instead of reading
/// every `fd` symlink; the returned list is the prefilter to carry next. That
/// scan is a `readlinkat` per open fd, ~8.8k per tick and ~17 ms of kernel
/// time per pass here, while the fdinfo reads it selects stay per tick so
/// GFX/CMP rates keep their cadence. A carried fd number since reused for
/// something else fails the `drm-client-id` check in `push_drm_text`.
pub(crate) fn read_pid(
    dir: impl AsFd,
    full_scan: bool,
    carried: Option<&[u32]>,
    buf: &mut Vec<u8>,
) -> (GpuCounters, Vec<u32>) {
    let drm_fds = match carried.map_or_else(|| drm_fd_nums(&dir), |v| Ok(v.to_vec())) {
        Err(Errno::ACCESS) => return (GpuCounters::default(), Vec::new()),
        Err(_) => Vec::new(),
        Ok(v) => v,
    };
    let prefilter_empty = drm_fds.is_empty();
    let filtered = if prefilter_empty {
        GpuCounters::default()
    } else {
        read_fdinfo_files(&dir, &drm_fds, buf)
    };
    if !needs_full_fdinfo(full_scan, prefilter_empty, &filtered) {
        return (filtered, drm_fds);
    }
    (read_all_fdinfo(&dir, buf), drm_fds)
}

/// `/dev/dri/renderD128`, `/dev/dri/card1`, and other drm device nodes.
fn fd_target_looks_like_drm(target: &str) -> bool {
    target.contains("dri") || target.contains("drm")
}

const fn needs_full_fdinfo(full_scan: bool, prefilter_empty: bool, filtered: &GpuCounters) -> bool {
    full_scan
        && !prefilter_empty
        && filtered.vram_bytes.is_none()
        && filtered.gtt_bytes.is_none()
        && filtered.gfx_ns.is_none()
        && filtered.compute_ns.is_none()
}

fn subdir(dir: impl AsFd, name: &CStr) -> rustix::io::Result<Dir> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    Dir::new(rustix::fs::openat(dir, name, flags, Mode::empty())?)
}

/// `.` and `..` are the only non-numeric names in `fd` and `fdinfo`.
fn fd_num(name: &CStr) -> Option<u32> {
    u32::try_from(atoi(name.to_bytes())?).ok()
}

fn drm_fd_nums(dir: impl AsFd) -> rustix::io::Result<Vec<u32>> {
    let mut fds = subdir(dir, c"fd")?;
    let mut link = [0; 4096];
    let mut nums = Vec::new();
    while let Some(ent) = fds.read() {
        let ent = ent?;
        let Some(n) = fd_num(ent.file_name()) else {
            continue;
        };
        let Some(target) = read_link_at(fds.fd()?, ent.file_name(), &mut link) else {
            continue;
        };
        if fd_target_looks_like_drm(&String::from_utf8_lossy(target)) {
            nums.push(n);
        }
    }
    Ok(nums)
}

fn read_fdinfo_files(dir: impl AsFd, fds: &[u32], buf: &mut Vec<u8>) -> GpuCounters {
    let mut texts = Vec::new();
    // "fdinfo/" and a u32 fit, so the name needs no allocation.
    let mut name = [0u8; 17];
    for fd in fds {
        let mut w = io::Cursor::new(&mut name[..]);
        let _ = write!(w, "fdinfo/{fd}");
        let len = usize::try_from(w.position()).unwrap_or(0);
        push_drm_text(&mut texts, &dir, &name[..len], buf);
    }
    merge_fdinfo_texts(&texts)
}

fn read_all_fdinfo(dir: impl AsFd, buf: &mut Vec<u8>) -> GpuCounters {
    let Ok(mut entries) = subdir(dir, c"fdinfo") else {
        return GpuCounters::default();
    };
    let mut texts = Vec::new();
    while let Some(Ok(ent)) = entries.read() {
        let Ok(fdinfo) = entries.fd() else {
            break;
        };
        if fd_num(ent.file_name()).is_some() {
            push_drm_text(&mut texts, fdinfo, ent.file_name(), buf);
        }
    }
    merge_fdinfo_texts(&texts)
}

/// Observed drm fdinfo on this host: vivaldi max 7 KiB, cursor 14 KiB.
/// `localsearch-3` has a 16_038_344-byte `anon_inode:[fanotify]` fdinfo
/// with 0 `drm-client-id`. 64 KiB is well above real drm and well below
/// that dump. `/proc/<pid>/fdinfo/*` reports `st_size` 0 (measured on
/// `/proc/self/fdinfo/0`), so the cap is on bytes read, never on metadata.
const FDINFO_MAX_BYTES: usize = 64 * 1024;

fn push_drm_text(
    texts: &mut Vec<String>,
    dir: impl AsFd,
    name: impl rustix::path::Arg,
    buf: &mut Vec<u8>,
) {
    let Some(bytes) = read_at(dir, name, buf, FDINFO_MAX_BYTES) else {
        return;
    };
    if bytes.len() <= FDINFO_MAX_BYTES
        && let Ok(text) = std::str::from_utf8(bytes)
        && text.contains("drm-client-id")
    {
        texts.push(text.to_owned());
    }
}

enum Region {
    Vram,
    Gtt,
}

/// Memory-stat prefixes in preference order. `drm-resident-*` is what the
/// region is actually holding; `drm-total-*` counts buffers that may be
/// evicted, so it is a fallback rather than a peer. `drm-memory-*` is
/// amdgpu's own pre-standard pair (`drivers/gpu/drm/amd/amdgpu/amdgpu_fdinfo.c)`:
/// it is the only memory key an amdgpu older than the `drm_show_memory_stats`
/// switch prints at all, and such a kernel still prints `drm-engine-gfx`, so
/// without this tier those hosts showed gfx%/compute% beside a blank VRAM and
/// GTT. Current kernels print all three.
const MEM_PREFIXES: [&str; 3] = ["drm-resident-", "drm-total-", "drm-memory-"];

/// amdgpu, i915 and xe all implement the same DRM fdinfo interface
/// (docs.kernel.org/gpu/drm-usage-stats.html) but name their regions
/// differently. i915 regions are `<class><instance>` -- "system0" is
/// GPU-visible system memory, "local0" is discrete VRAM
/// (`i915/intel_memory_region.c` `intel_memory_type_str`); xe uses "gtt" and
/// "vram0"/"vram1" (`xe/xe_bo.c` `xe_mem_type_to_name`). Summing lets a multi-tile
/// xe report both VRAM tiles. Every other region amdgpu prints -- cpu, gds,
/// gws, oa, doorbell, mmioremap -- is neither.
fn mem_key(k: &str) -> Option<(usize, Region)> {
    let (tier, region) = MEM_PREFIXES
        .iter()
        .enumerate()
        .find_map(|(i, p)| k.strip_prefix(p).map(|r| (i, r)))?;
    match region {
        "vram" | "vram0" | "vram1" | "local0" => Some((tier, Region::Vram)),
        "gtt" | "system0" => Some((tier, Region::Gtt)),
        _ => None,
    }
}

/// The first tier the client published, so a present `0` still beats a
/// less-exact tier's larger figure.
fn best_tier(tiers: [Option<u64>; MEM_PREFIXES.len()]) -> Option<u64> {
    tiers.into_iter().flatten().next()
}

fn parse_fdinfo(text: &str) -> Option<(u64, GpuCounters)> {
    let mut driver_ok = false;
    let mut id = None;
    // One slot per tier of MEM_PREFIXES, highest-preference first.
    let mut vram = [None; MEM_PREFIXES.len()];
    let mut gtt = [None; MEM_PREFIXES.len()];
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
        // Regions: `mem_key`. Engines are named per driver too.
        match k {
            "drm-driver" if matches!(v, "amdgpu" | "i915" | "xe") => driver_ok = true,
            "drm-client-id" => id = v.trim().parse().ok(),
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
            _ => match mem_key(k) {
                Some((tier, Region::Vram)) => vram[tier] = sum_opt(vram[tier], parse_size(v)),
                Some((tier, Region::Gtt)) => gtt[tier] = sum_opt(gtt[tier], parse_size(v)),
                None => {}
            },
        }
    }
    if !(driver_ok || (text.contains("drm-client-id") && text.contains("drm-resident"))) {
        return None;
    }
    Some((
        id?,
        GpuCounters {
            vram_bytes: best_tier(vram),
            gtt_bytes: best_tier(gtt),
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
        "MiB" | "MB" => 1024 * 1024,
        "GiB" | "GB" => 1024 * 1024 * 1024,
        "B" => 1,
        // KiB (also written kiB, kB, KB) and any unit this list does not name.
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

    /// Shape taken from the example in `drivers/gpu/drm/xe/xe_drm_client.c`.
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

    /// amdgpu before it adopted `drm_show_memory_stats`: engine counters, and
    /// `drm-memory-*` as the only memory keys.
    const AMDGPU_LEGACY: &str = "drm-driver:\tamdgpu\ndrm-client-id:\t550\ndrm-memory-vram:\t23048 KiB\ndrm-memory-gtt: \t80128 KiB\ndrm-memory-cpu: \t0 KiB\namd-evicted-vram:\t59648 KiB\namd-requested-vram:\t82696 KiB\ndrm-shared-vram:\t8 KiB\ndrm-shared-gtt:\t5632 KiB\ndrm-engine-gfx:\t301893603834 ns\ndrm-engine-compute:\t31641274970 ns\n";

    #[test]
    fn legacy_amdgpu_memory_keys_are_not_blank() {
        let g = merge_fdinfo_texts(&[AMDGPU_LEGACY.to_string()]);
        // drm-shared-* is a subset of the region, never a tier of its own.
        assert_eq!(g.vram_bytes, Some(23048 * 1024));
        assert_eq!(g.gtt_bytes, Some(80128 * 1024));
        assert_eq!(g.gfx_ns, Some(301_893_603_834));
        assert_eq!(g.compute_ns, Some(31_641_274_970));
    }

    #[test]
    fn resident_beats_total_beats_memory() {
        // Shape of a current amdgpu client: all three tiers, and a resident
        // figure far below total because most of it is evicted.
        let text = "drm-driver:\tamdgpu\ndrm-client-id:\t7\n\
             drm-total-vram:\t117088 KiB\ndrm-resident-vram:\t180 KiB\n\
             drm-memory-vram:\t999 KiB\ndrm-total-gtt:\t22572 KiB\n\
             drm-memory-gtt: \t139480 KiB\ndrm-total-cpu:\t5 KiB\n";
        let g = merge_fdinfo_texts(&[text.to_string()]);
        assert_eq!(g.vram_bytes, Some(180 * 1024), "resident wins");
        assert_eq!(
            g.gtt_bytes,
            Some(22572 * 1024),
            "no resident-gtt, total wins"
        );
        // A zero resident figure is a figure, not a reason to fall through.
        let zero = "drm-driver:\tamdgpu\ndrm-client-id:\t8\n\
             drm-resident-vram:\t0\ndrm-total-vram:\t4096 KiB\n";
        assert_eq!(merge_fdinfo_texts(&[zero.to_string()]).vram_bytes, Some(0));
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
        let dir = std::env::temp_dir().join(format!("heft-fdinfo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Real client text past the cap, so only the cap can reject it.
        let huge = SAMPLE.repeat(FDINFO_MAX_BYTES / SAMPLE.len() + 1);
        std::fs::write(dir.join("huge"), huge).unwrap();
        std::fs::write(dir.join("small"), SAMPLE).unwrap();
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let fd = rustix::fs::open(dir.as_path(), flags, Mode::empty()).unwrap();
        let (mut texts, mut buf) = (Vec::new(), Vec::new());
        push_drm_text(&mut texts, &fd, c"huge", &mut buf);
        assert!(texts.is_empty(), "oversize is skipped even as a drm client");
        push_drm_text(&mut texts, &fd, c"small", &mut buf);
        assert_eq!(texts.len(), 1);
        let g = merge_fdinfo_texts(&texts);
        assert_eq!(g.vram_bytes, Some(48596 * 1024));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
