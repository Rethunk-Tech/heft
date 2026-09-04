use std::collections::HashMap;
use std::fs;
use std::io;

use crate::types::GpuCounters;

pub fn read_pid(pid: u32) -> GpuCounters {
    let dir = format!("/proc/{pid}/fdinfo");
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return GpuCounters::default(),
        Err(_) => return GpuCounters::default(),
    };
    let mut texts = Vec::new();
    for ent in entries.flatten() {
        match fs::read_to_string(ent.path()) {
            Ok(t) if t.contains("drm-client-id") => texts.push(t),
            Ok(_) => {}
            Err(_) => {}
        }
    }
    merge_fdinfo_texts(&texts)
}

struct Client {
    id: u64,
    vram: Option<u64>,
    gtt: Option<u64>,
    gfx_ns: Option<u64>,
    compute_ns: Option<u64>,
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

impl From<ClientView> for Client {
    fn from(c: ClientView) -> Self {
        Self {
            id: c.id,
            vram: c.vram,
            gtt: c.gtt,
            gfx_ns: c.gfx_ns,
            compute_ns: c.compute_ns,
        }
    }
}

fn merge_one(text: &str) -> Option<Client> {
    parse_fdinfo(text).map(Client::from)
}

pub fn merge_fdinfo_texts(texts: &[String]) -> GpuCounters {
    let mut by_client: HashMap<u64, Client> = HashMap::new();
    for text in texts {
        if let Some(c) = merge_one(text) {
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

    #[test]
    fn dedupe_client_id() {
        let sample = "drm-driver:\tamdgpu\ndrm-client-id:\t27\ndrm-resident-vram:\t48596 KiB\ndrm-resident-gtt:\t100 KiB\ndrm-engine-gfx:\t1000 ns\n";
        let texts = [sample.to_string(), sample.to_string()];
        let g = merge_fdinfo_texts(&texts);
        assert_eq!(g.vram_bytes, Some(48596 * 1024));
        assert_eq!(g.gfx_ns, Some(1000));
    }
}
