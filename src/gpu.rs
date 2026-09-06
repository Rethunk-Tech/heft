use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::types::GpuCounters;

/// `full_scan` is the PSS / `--once` published pass. Prime and TUI catch-all
/// ticks pass false: dri/drm symlink names are enough, and a first sighting
/// must not walk every fdinfo. Clients whose fd names omit dri/drm show up
/// on the next full_scan (up to `--pss-interval`).
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
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return GpuCounters::default(),
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
        match k {
            "drm-driver" if v.contains("amdgpu") => driver_ok = true,
            "drm-client-id" => id = v.trim().parse().ok(),
            "drm-resident-vram" => vram = parse_size(v),
            "drm-resident-gtt" => gtt = parse_size(v),
            "drm-engine-gfx" => gfx_ns = parse_ns(v),
            "drm-engine-compute" => compute_ns = parse_ns(v),
            _ => {}
        }
    }
    if !driver_ok && !(text.contains("drm-client-id") && text.contains("drm-resident")) {
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
        out.vram_bytes = add(out.vram_bytes, c.vram);
        out.gtt_bytes = add(out.gtt_bytes, c.gtt);
        out.gfx_ns = add(out.gfx_ns, c.gfx_ns);
        out.compute_ns = add(out.compute_ns, c.compute_ns);
    }
    out
}

fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        (x, y) => Some(x.unwrap_or(0) + y.unwrap_or(0)),
    }
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
