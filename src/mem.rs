use std::fs;
use std::path::Path;

pub fn read_ram() -> (u64, u64) {
    let Ok(text) = fs::read_to_string("/proc/meminfo") else {
        return (0, 0);
    };
    parse_meminfo(&text)
}

pub fn parse_meminfo(text: &str) -> (u64, u64) {
    let mut total = 0u64;
    let mut avail = 0u64;
    for line in text.lines() {
        if let Some(v) = kb_field(line, "MemTotal:") {
            total = v.saturating_mul(1024);
        } else if let Some(v) = kb_field(line, "MemAvailable:") {
            avail = v.saturating_mul(1024);
        }
    }
    (total.saturating_sub(avail), total)
}

fn kb_field(line: &str, key: &str) -> Option<u64> {
    let rest = line.strip_prefix(key)?.trim();
    rest.split_whitespace().next()?.parse().ok()
}

pub fn read_vram() -> (Option<u64>, Option<u64>) {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return (None, None);
    };
    let mut used = 0u64;
    let mut total = 0u64;
    let mut found = false;
    for ent in entries.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dir = ent.path().join("device");
        if let Some(t) = read_u64(&dir.join("mem_info_vram_total")) {
            if let Some(u) = read_u64(&dir.join("mem_info_vram_used")) {
                total = total.saturating_add(t);
                used = used.saturating_add(u);
                found = true;
            }
        }
    }
    if found {
        (Some(used), Some(total))
    } else {
        (None, None)
    }
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_used() {
        let (used, total) = parse_meminfo("MemTotal: 1000 kB\nMemAvailable: 400 kB\n");
        assert_eq!(total, 1000 * 1024);
        assert_eq!(used, 600 * 1024);
    }
}
