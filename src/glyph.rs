//! Which characters the bars, rules and markers are drawn with.
//!
//! Every one of them is a Unicode block, box-drawing or geometric character.
//! On a machine that has them they are the right choice — the shade ramp is
//! what carries a stacked bar's segments without relying on colour. On a bare
//! kernel console, an old terminal, or a container image stripped to a few
//! fonts, they render as tofu and the bar becomes unreadable.
//!
//! Resolved once at startup rather than threaded through every render site:
//! the answer cannot change while heft runs, and `ui` already passes `Columns`
//! down for the thing that can.
//!
//! Every ASCII substitute is one column wide, like the character it replaces.
//! That is the constraint, not the aesthetics: the header lines are built to
//! land on an exact width and the table truncates to an exact column count, so
//! a three-character `...` for `…` would silently overflow both.

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Set {
    Unicode,
    Ascii,
}

static SET: OnceLock<Set> = OnceLock::new();

/// Call once from `main`. `None` means detect. Defaults to Unicode when never
/// called, which is what keeps the render tests independent of the locale the
/// suite happens to run under.
pub fn init(choice: Option<Set>) {
    let _ = SET.set(choice.unwrap_or_else(detect));
}

fn set() -> Set {
    SET.get().copied().unwrap_or(Set::Unicode)
}

fn ascii() -> bool {
    set() == Set::Ascii
}

/// The POSIX variables in the order POSIX resolves them. No variable set at
/// all is the C locale, which is not UTF-8 — a container with no locale
/// configured is exactly the machine most likely to lack the fonts, so
/// defaulting it to ASCII fails in the safe direction. `--glyphs unicode`
/// is there for the terminal whose environment undersells it.
fn detect() -> Set {
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        match std::env::var(key) {
            Ok(v) if v.is_empty() => continue,
            Ok(v) => return charmap_set(&v),
            Err(_) => continue,
        }
    }
    Set::Ascii
}

/// Split out from `detect` so it can be tested: reading the environment is
/// process-wide, and the suite runs its tests in parallel threads.
fn charmap_set(locale: &str) -> Set {
    let v = locale.to_ascii_lowercase();
    if v.contains("utf-8") || v.contains("utf8") {
        Set::Unicode
    } else {
        Set::Ascii
    }
}

/// A filled bar segment, and the densest step of the ramp.
pub(crate) fn full() -> char {
    if ascii() { '#' } else { '█' }
}

pub(crate) fn dark() -> char {
    if ascii() { '=' } else { '▓' }
}

pub(crate) fn medium() -> char {
    if ascii() { '+' } else { '▒' }
}

/// The unfilled tail of a bar. Never a segment: it has to read as absence.
pub(crate) fn light() -> char {
    if ascii() { '.' } else { '░' }
}

/// Two segments that need to differ from the ramp rather than continue it.
pub(crate) fn quad_a() -> char {
    if ascii() { '%' } else { '▚' }
}

pub(crate) fn quad_b() -> char {
    if ascii() { '*' } else { '▙' }
}

pub(crate) fn rule() -> char {
    if ascii() { '-' } else { '─' }
}

/// One column, so the truncation width arithmetic is unchanged.
pub(crate) fn ellipsis() -> char {
    if ascii() { '~' } else { '…' }
}

pub(crate) fn expanded() -> &'static str {
    if ascii() { "v " } else { "▼ " }
}

pub(crate) fn collapsed() -> &'static str {
    if ascii() { "> " } else { "▶ " }
}

#[cfg(test)]
mod tests {
    use super::{Set, charmap_set};

    #[test]
    fn only_a_utf8_charmap_earns_the_block_characters() {
        for name in ["en_US.UTF-8", "en_US.utf8", "C.UTF-8", "en_GB.UTF-8@euro"] {
            assert_eq!(charmap_set(name), Set::Unicode, "{name}");
        }
        // The C locale is not UTF-8, and a container with nothing configured
        // lands here too: the machine most likely to lack the fonts.
        for name in ["C", "POSIX", "en_US", "en_US.ISO-8859-1", "ru_RU.KOI8-R"] {
            assert_eq!(charmap_set(name), Set::Ascii, "{name}");
        }
    }
}
