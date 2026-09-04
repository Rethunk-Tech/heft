use std::fs;
use std::io;

pub fn read_io(pid: u32) -> (Option<u64>, Option<u64>) {
    match fs::read_to_string(format!("/proc/{pid}/io")) {
        Ok(text) => parse_io(&text),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => (None, None),
        Err(_) => (None, None),
    }
}

pub fn parse_io(text: &str) -> (Option<u64>, Option<u64>) {
    let mut r = None;
    let mut w = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("read_bytes:") {
            r = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("write_bytes:") {
            w = v.trim().parse().ok();
        }
    }
    (r, w)
}

pub fn read_pss_kb(pid: u32) -> Option<u64> {
    match fs::read_to_string(format!("/proc/{pid}/smaps_rollup")) {
        Ok(text) => parse_pss_kb(&text),
        Err(_) => None,
    }
}

pub fn parse_pss_kb(text: &str) -> Option<u64> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Pss:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_and_pss() {
        let (r, w) = parse_io("rchar: 1\nread_bytes: 10\nwrite_bytes: 20\n");
        assert_eq!(r, Some(10));
        assert_eq!(w, Some(20));
        assert_eq!(parse_pss_kb("Rss: 9 kB\nPss:  42 kB\n"), Some(42));
    }
}
