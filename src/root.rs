//! Where `/proc` and `/sys` are read from.
//!
//! Resolved once at startup into a `OnceLock` rather than threaded through
//! every reader, the same reason `glyph` is: it cannot change while heft runs,
//! and the alternative is a parameter on every function that touches a file.
//!
//! Empty by default, so every path is the literal it always was and the
//! default build reads the running machine byte for byte.

use std::sync::OnceLock;

static ROOT: OnceLock<String> = OnceLock::new();

/// Set the prefix. Called once, before any sampling. A trailing slash is
/// dropped so `path()` never produces a doubled separator.
pub fn init(root: Option<&str>) {
    let _ = ROOT.set(root.map(trimmed).unwrap_or_default().to_string());
}

fn trimmed(given: &str) -> &str {
    given.trim_end_matches('/')
}

/// The prefix, empty when heft is reading the machine it runs on.
pub(crate) fn prefix() -> &'static str {
    ROOT.get().map_or("", String::as_str)
}

/// An absolute kernel path with the prefix applied. `path("/proc/stat")` is
/// `/proc/stat` by default and `<root>/proc/stat` under `--proc-root`. A path
/// that interpolates a pid or another runtime value is built with
/// `format!("{}/proc/{pid}/...", prefix())` instead, since `path(&format!(..))`
/// allocates twice for every pid on every tick.
pub(crate) fn path(abs: &str) -> String {
    format!("{}{abs}", prefix())
}

#[cfg(test)]
mod tests {
    /// A trailing slash would double the separator in every path built from
    /// the prefix, and `//proc/stat` is a different string in every error
    /// message even where the kernel accepts it.
    #[test]
    fn a_trailing_slash_never_doubles_the_separator() {
        for given in ["/mnt/tree", "/mnt/tree/", "/mnt/tree///"] {
            assert_eq!(
                format!("{}/proc/stat", super::trimmed(given)),
                "/mnt/tree/proc/stat"
            );
        }
        // The default is empty, so every path is the literal it always was.
        assert_eq!(format!("{}/proc/stat", super::trimmed("")), "/proc/stat");
    }
}
