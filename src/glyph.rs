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
//! `Set::Legacy` sits between the two: a font that draws `█▓▒░`, `▀▄` and `▼►`
//! but not `▁▂▃▅▆▇`, which is common enough that asking such a
//! reader to drop to ASCII would cost them a header that was rendering fine.
//!
//! Every ASCII substitute is one column wide, like the character it replaces.
//! That is the constraint, not the aesthetics: the header lines are built to
//! land on an exact width and the table truncates to an exact column count, so
//! a three-character `...` for `…` would silently overflow both.

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Set {
    Unicode,
    /// Every Unicode character heft draws except the sparkline's eighth
    /// blocks. A font can carry the shade ramp, the half blocks and the
    /// CP437 triangles and still lack six of the ramp's eight steps -- every
    /// one but U+2584 and U+2588, which are half and full blocks too -- so
    /// TREND is the one tofu column on a machine whose bars and tree draw
    /// correctly.
    /// Never detected -- nothing heft can read says what the font covers --
    /// so it is only ever asked for.
    Legacy,
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
        if let Ok(v) = std::env::var(key)
            && !v.is_empty()
        {
            return charmap_set(&v);
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
/// Half blocks rather than the quadrants (`▚`, `▙`) they used to be: a font
/// carrying the shade ramp and the full block does not necessarily carry
/// U+2596-U+259F, and a reader whose bar drew `█▓▒` correctly still got tofu
/// where VRAM and GTT went. U+2580 and U+2584 sit in the same legacy
/// repertoire as the ramp itself, so a font that can draw one can draw these.
pub(crate) fn quad_a() -> char {
    if ascii() { '%' } else { '▀' }
}

pub(crate) fn quad_b() -> char {
    if ascii() { '*' } else { '▄' }
}

/// Eight rising steps for a sparkline, lowest first. Every one is a single
/// column in both sets, the same rule the bars and `ellipsis` keep, so a cell
/// of eight of them is eight columns wide whatever the terminal resolved.
pub(crate) fn spark_ramp() -> &'static [char; 8] {
    // The one place `Set::Legacy` parts company with `Set::Unicode`: the
    // eighth blocks are the only characters heft draws that a legacy font
    // reliably lacks while still carrying the bars.
    if set() != Set::Unicode {
        &['_', '.', ',', ':', '-', '=', '+', '#']
    } else {
        &[
            '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
            '\u{2588}',
        ]
    }
}

pub(crate) fn rule() -> char {
    if ascii() { '-' } else { '─' }
}

/// One column, so the truncation width arithmetic is unchanged.
pub(crate) fn ellipsis() -> char {
    if ascii() { '~' } else { '…' }
}

/// The four cursor keys, for the help overlay. One column each, so the
/// overlay's key column keeps its width under either set.
pub(crate) fn arrows() -> (char, char, char, char) {
    if ascii() {
        ('^', 'v', '<', '>')
    } else {
        ('\u{2191}', '\u{2193}', '\u{2190}', '\u{2192}')
    }
}

pub(crate) fn expanded() -> &'static str {
    if ascii() { "v " } else { "▼ " }
}

/// U+25BA rather than U+25B6, which looks identical in a font that has both.
/// U+25B6 is the base of the play-button emoji, so a terminal that resolves
/// emoji presentation can draw it double-width -- in a table whose every
/// column is exact, that shifts the row -- and a font without it falls through
/// to tofu beside the U+25BC that `expanded` draws fine. U+25BA carries no
/// emoji property and pairs with U+25BC in the same legacy repertoire.
pub(crate) fn collapsed() -> &'static str {
    if ascii() { "> " } else { "► " }
}

#[cfg(test)]
mod tests {
    use super::{Set, charmap_set};

    #[test]
    fn legacy_keeps_the_bars_and_drops_only_the_spark_ramp() {
        use super::{Set, full, quad_a, set, spark_ramp};
        // `set()` is process-wide and the suite runs in parallel threads, so
        // this asserts over the table rather than by initialising it.
        assert_eq!(set(), Set::Unicode, "the default the render tests rely on");
        assert_eq!(full(), '█');
        assert_eq!(quad_a(), '▀', "a half block, not a quadrant");
        assert_eq!(spark_ramp()[0], '▁');
        // Every glyph heft draws outside the sparkline is one a legacy font
        // carries, which is what makes Legacy one branch rather than a table.
        // U+2584 is not in that gap: it is the ramp's midpoint and a half
        // block, so `quad_b` may and does use it.
        let missing = ['▁', '▂', '▃', '▅', '▆', '▇'];
        for c in [full(), quad_a(), super::quad_b(), super::rule()] {
            assert!(!missing.contains(&c), "{c} is a step a legacy font lacks");
        }
        assert_eq!(
            spark_ramp().iter().filter(|c| missing.contains(c)).count(),
            6,
            "six of the eight steps are what Legacy exists for"
        );
    }

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
