use std::fs;

use crate::proc::field_u64;

pub(crate) fn read_io(pid: u32) -> (Option<u64>, Option<u64>) {
    match fs::read_to_string(format!("/proc/{pid}/io")) {
        Ok(text) => parse_io(&text),
        Err(_) => (None, None),
    }
}

pub(crate) fn parse_io(text: &str) -> (Option<u64>, Option<u64>) {
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

/// PSS and swapped-out PSS from one `smaps_rollup` read. `want_swap` is the
/// host having swap at all: with `SwapTotal: 0` every process reports
/// `SwapPss: 0`, and a column of zeros claims a figure exists where none does.
pub(crate) fn read_rollup_kb(pid: u32, want_swap: bool) -> (Option<u64>, Option<u64>) {
    match fs::read_to_string(format!("/proc/{pid}/smaps_rollup")) {
        Ok(text) => (
            parse_pss_kb(&text),
            want_swap.then(|| parse_swap_pss_kb(&text)).flatten(),
        ),
        Err(_) => (None, None),
    }
}

pub(crate) fn parse_pss_kb(text: &str) -> Option<u64> {
    // `Pss:` only — smaps_rollup also carries Pss_Anon/Pss_File/Pss_Shmem.
    text.lines().find_map(|l| field_u64(l, "Pss:"))
}

pub(crate) fn parse_swap_pss_kb(text: &str) -> Option<u64> {
    // `SwapPss:`, not `Swap:` — see the `Metrics::swap_bytes` comment.
    text.lines().find_map(|l| field_u64(l, "SwapPss:"))
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

    /// `Swap:` sits directly above `SwapPss:` in the rollup and is the wrong
    /// one: prefix matching must not take it, and must not take `SwapPss` for
    /// `Pss` either.
    #[test]
    fn swap_pss_is_not_swap_and_not_pss() {
        let text = "Pss: 42 kB\nSwap: 900 kB\nSwapPss: 100 kB\n";
        assert_eq!(parse_swap_pss_kb(text), Some(100));
        assert_eq!(parse_pss_kb(text), Some(42));
        assert_eq!(parse_swap_pss_kb("Pss: 42 kB\n"), None);
    }
}
