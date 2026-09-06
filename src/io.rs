use std::fs;

use crate::proc::field_u64;

pub fn read_io(pid: u32) -> (Option<u64>, Option<u64>) {
    match fs::read_to_string(format!("/proc/{pid}/io")) {
        Ok(text) => parse_io(&text),
        Err(_) => (None, None),
    }
}

pub fn parse_io(text: &str) -> (Option<u64>, Option<u64>) {
    let mut r = None;
    let mut w = None;
    for line in text.lines() {
        if let Some(v) = field_u64(line, "read_bytes:") {
            r = Some(v);
        } else if let Some(v) = field_u64(line, "write_bytes:") {
            w = Some(v);
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
    // `Pss:` only — smaps_rollup also carries Pss_Anon/Pss_File/Pss_Shmem.
    text.lines().find_map(|l| field_u64(l, "Pss:"))
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
