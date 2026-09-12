//! TREND as a sixel image, for the terminals the kitty protocol does not
//! reach: xterm (on by default since patch #359), foot, wezterm, konsole
//! 22.04+, iTerm2, and Windows Terminal 1.22+.
//!
//! The picture is the same one `kgp` sends -- `kgp::paint` draws it, against
//! the scale `ui::trend_scale` gives the frame -- so the two only differ in
//! how the pixels reach the terminal.
//!
//! Sixel has no equivalent of kitty's Unicode placeholders: an image is
//! painted wherever the cursor is, with nothing tying it to the cell grid. So
//! this one has to be positioned, and the position cannot be computed --
//! the name column is a `Constraint::Min` and ratatui's layout solver decides
//! what it absorbs. Instead the TREND cells are rendered as spaces carrying a
//! marker colour, and `ui` reads the rectangle back out of the frame buffer
//! afterwards. Re-deriving the solver's arithmetic here would be a second
//! implementation of it, wrong the first time the layout changed.
//!
//! It is also emitted after ratatui flushes, not before. ratatui writes only
//! the cells that changed, and a cell it rewrites erases the pixels over it,
//! so the image is repainted every frame rather than hashed and skipped the
//! way the kitty one is. That is affordable here for a reason it would not be
//! there: a sparkline is mostly empty, and sixel run-length-encodes the empty
//! part, so a frame is a couple of kilobytes rather than the whole bitmap.

use std::fmt::Write;

use crate::kgp::Image;

/// Sixel data characters run `?` (0x3F, six blank pixels) to `~` (0x7E, six
/// set), one character per column of six vertical pixels.
const SIXEL_ZERO: u8 = b'?';

/// `!n<char>` costs three characters plus the digits, so a run shorter than
/// this is cheaper written out.
const RUN_MIN: usize = 4;

/// Sixel colour components are percentages, not bytes.
fn percent(v: u8) -> u32 {
    (u32::from(v) * 100 + 127) / 255
}

fn push_run(out: &mut String, ch: u8, n: usize) {
    if n == 0 {
        return;
    }
    if n >= RUN_MIN {
        out.push('!');
        out.push_str(&n.to_string());
        out.push(ch as char);
    } else {
        for _ in 0..n {
            out.push(ch as char);
        }
    }
}

/// Encode the image's alpha channel as a one-colour sixel.
///
/// The trend is a line in a single colour over transparency, so the alpha
/// byte is the whole picture and the RGB is a constant -- which is what lets
/// this be one colour register and one pass rather than a quantiser.
///
/// `P2=1` in the introducer is what makes the zero pixels transparent instead
/// of painting them the background colour; without it the image would be an
/// opaque block over the row, including over the cursor highlight.
pub(crate) fn encode(img: &Image, colour: [u8; 3]) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("\x1bP0;1;0q");
    // Written into the buffer rather than formatted into a temporary and
    // copied: `encode` runs once a frame.
    let _ = write!(out, "\"1;1;{};{}", img.w, img.h);
    let _ = write!(
        out,
        "#0;2;{};{};{}",
        percent(colour[0]),
        percent(colour[1]),
        percent(colour[2])
    );
    let lit = |x: u32, y: u32| y < img.h && img.rgba[((y * img.w + x) * 4 + 3) as usize] != 0;
    let bands = img.h.div_ceil(6);
    for band in 0..bands {
        out.push_str("#0");
        let top = band * 6;
        let (mut run_ch, mut run_len) = (0u8, 0usize);
        for x in 0..img.w {
            let mut bits = 0u8;
            for k in 0..6 {
                if lit(x, top + k) {
                    bits |= 1 << k;
                }
            }
            let ch = SIXEL_ZERO + bits;
            if ch == run_ch {
                run_len += 1;
            } else {
                push_run(&mut out, run_ch, run_len);
                run_ch = ch;
                run_len = 1;
            }
        }
        push_run(&mut out, run_ch, run_len);
        // Between bands only: a trailing one would advance the cursor past
        // the image and scroll the row under it.
        if band + 1 < bands {
            out.push('-');
        }
    }
    out.push_str("\x1b\\");
    out
}

/// Park the cursor, paint, put it back. `\x1b7` / `\x1b8` rather than a saved
/// row and column, because heft does not track where ratatui left it.
pub(crate) fn at(row: u16, col: u16, body: &str) -> String {
    format!("\x1b7\x1b[{};{}H{body}\x1b8", row + 1, col + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32, lit: &[(u32, u32)]) -> Image {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for (x, y) in lit {
            rgba[((y * w + x) * 4 + 3) as usize] = 0xff;
        }
        Image { w, h, rgba }
    }

    #[test]
    fn a_pixel_becomes_the_bit_that_names_its_row() {
        // One lit pixel at the top of a six-tall band is bit 0, so `?` + 1.
        let s = encode(&img(2, 6, &[(0, 0)]), [255, 0, 0]);
        assert!(s.starts_with("\x1bP0;1;0q\"1;1;2;6"), "{s}");
        assert!(
            s.contains("#0;2;100;0;0"),
            "components are percentages: {s}"
        );
        assert!(s.ends_with("\x1b\\"));
        assert!(s.contains("#0@?"), "bit 0 then an empty column: {s}");
        // Bottom of the band is bit 5, so `?` + 32.
        let s = encode(&img(1, 6, &[(0, 5)]), [0, 0, 0]);
        assert!(
            s.contains(&format!("#0{}", (SIXEL_ZERO + 32) as char)),
            "{s}"
        );
    }

    #[test]
    fn empty_columns_are_run_length_encoded() {
        // A line is mostly nothing, which is what makes repainting every
        // frame affordable where the kitty transport had to be hashed.
        let s = encode(&img(400, 6, &[(0, 0)]), [0, 0, 0]);
        assert!(s.contains("!399?"), "399 blank columns in one run: {s}");
        assert!(
            s.len() < 60,
            "a near-empty band stays tiny, got {}",
            s.len()
        );
        // Below the threshold it is cheaper to write the columns out.
        let s = encode(&img(3, 6, &[]), [0, 0, 0]);
        assert!(s.contains("#0???"), "{s}");
    }

    #[test]
    fn bands_are_separated_but_the_last_one_is_not() {
        let s = encode(&img(1, 18, &[]), [0, 0, 0]);
        assert_eq!(s.matches('-').count(), 2, "three bands, two separators");
        let s = encode(&img(1, 6, &[]), [0, 0, 0]);
        assert_eq!(s.matches('-').count(), 0);
        // A height that is not a multiple of six still gets a whole band, and
        // the rows past the end read as empty rather than out of bounds.
        let s = encode(&img(1, 7, &[(0, 6)]), [0, 0, 0]);
        assert_eq!(s.matches('-').count(), 1);
        assert!(
            s.contains(&format!("-#0{}", (SIXEL_ZERO + 1) as char)),
            "{s}"
        );
    }

    #[test]
    fn the_cursor_goes_back_where_it_was() {
        let s = at(4, 9, "X");
        assert_eq!(s, "\x1b7\x1b[5;10HX\x1b8", "one-based screen coordinates");
    }
}
