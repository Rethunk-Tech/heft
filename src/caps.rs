//! Asking the terminal what it can draw, for `--trend auto`.
//!
//! heft resolves everything else it renders from things it can read for free —
//! `glyph` from the locale, `kgp` from `SSH_CONNECTION`. This is the one
//! question with no such answer: `TERM` names a terminal, not what it
//! implements, a terminal multiplexer or an ssh hop can take a capability away
//! from underneath it, and a wrong guess is not a cosmetic error but a screen
//! of escape codes.
//!
//! So it is asked, once, before the first frame. Two queries go out together:
//!
//! * the kitty graphics protocol's own query (`a=q`), answered `;OK` by a
//!   terminal that has it and ignored by one that does not, since an
//!   unrecognised APC string is discarded rather than printed;
//! * a primary device attributes request (`CSI c`), whose reply lists `4`
//!   where sixel is available.
//!
//! DA1 is the sentinel that makes the read terminable. Every terminal answers
//! it, and answers in order, so its reply arriving is proof the kitty query
//! has already been answered or ignored. Without it the only way to learn that
//! a terminal has no graphics support would be to wait out the full timeout on
//! every start.

use std::io::Write;
use std::time::Duration;

/// What the terminal said it can do.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Caps {
    pub(crate) kitty: bool,
    pub(crate) sixel: bool,
}

/// A one-pixel RGB image, queried rather than stored. `i=` is required for the
/// terminal to answer at all.
const QUERY: &str = "\x1b_Gi=4919,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";

/// Long enough for an ssh round trip, short enough not to read as a hang. Only
/// ever waited out in full by a terminal that answers neither query, which is
/// rare enough that `--trend chars` is the answer for it anyway.
const TIMEOUT: Duration = Duration::from_millis(400);

/// Read the two replies out of whatever the terminal sent back.
///
/// Split from the I/O so the wire formats can be tested without a terminal:
/// the shapes below are transcribed from real replies, and a parser that
/// drifts from them silently turns the feature off.
pub(crate) fn parse_reply(s: &str) -> Caps {
    // `\x1b_Gi=4919;OK\x1b\` on success, `;ENOTSUPPORTED` and friends on
    // failure, so the `OK` is what is being looked for and not the echo.
    let kitty = s
        .split("\x1b_G")
        .skip(1)
        .any(|r| r.split_once(';').is_some_and(|(_, v)| v.starts_with("OK")));
    // `\x1b[?62;4;6;22c` -- the attributes are a `;` list and sixel is 4.
    let sixel = s.split("\x1b[?").skip(1).any(|r| {
        r.split_once('c')
            .is_some_and(|(attrs, _)| attrs.split(';').any(|a| a == "4"))
    });
    Caps { kitty, sixel }
}

/// True once the reply holds a complete DA1 answer, which every terminal sends
/// and sends last.
fn done(s: &str) -> bool {
    s.split("\x1b[?").skip(1).any(|r| r.contains('c'))
}

/// Ask, and wait for the answer. Raw mode must already be on, or the reply is
/// line-buffered and echoed into the screen.
///
/// Returns nothing found on any failure — a terminal that cannot be asked is
/// one heft draws characters for.
#[expect(
    clippy::cast_sign_loss,
    reason = "the read above returns on n <= 0, so n is a positive byte count"
)]
pub(crate) fn probe() -> Caps {
    let mut out = std::io::stdout();
    if out.write_all(QUERY.as_bytes()).is_err() || out.flush().is_err() {
        return Caps::default();
    }
    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut buf = String::new();
    let mut chunk = [0u8; 256];
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() || !readable(left) {
            break;
        }
        // SAFETY: a plain read into a live buffer of the length given.
        let n = unsafe {
            libc::read(
                libc::STDIN_FILENO,
                chunk.as_mut_ptr().cast(),
                chunk.len() as libc::size_t,
            )
        };
        if n <= 0 {
            break;
        }
        buf.push_str(&String::from_utf8_lossy(&chunk[..n as usize]));
        if done(&buf) {
            break;
        }
    }
    parse_reply(&buf)
}

/// `poll` rather than a non-blocking read spun in a loop: the reply arrives
/// whenever the terminal gets to it, and busy-waiting for it would show up as
/// a spike before heft has drawn anything.
fn readable(within: Duration) -> bool {
    let mut fds = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = i32::try_from(within.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: one live pollfd for the duration of the call.
    unsafe { libc::poll(&raw mut fds, 1, ms) > 0 }
}

/// How long to keep swallowing bytes after the probe. A device-attributes
/// reply ends in `c`, and `c` cycles the sort column, so a reply that arrived
/// a moment after the timeout would otherwise land on the keyboard.
const DRAIN_IDLE: Duration = Duration::from_millis(50);

/// Read the trailing bytes of a reply that arrived after the timeout, so they
/// are not taken for keypresses. Bounded: it stops as soon as the terminal has
/// been quiet for `DRAIN_IDLE`.
pub(crate) fn drain() {
    let mut chunk = [0u8; 256];
    while readable(DRAIN_IDLE) {
        // SAFETY: as above.
        let n = unsafe {
            libc::read(
                libc::STDIN_FILENO,
                chunk.as_mut_ptr().cast(),
                chunk.len() as libc::size_t,
            )
        };
        if n <= 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kitty_terminal_answers_ok() {
        let c = parse_reply("\x1b_Gi=4919;OK\x1b\\\x1b[?62;22c");
        assert!(c.kitty);
        assert!(!c.sixel, "62 and 22 are not sixel");
    }

    #[test]
    fn sixel_is_attribute_four() {
        // xterm with sixel built in.
        let c = parse_reply("\x1b[?62;4;6;9;22c");
        assert!(c.sixel);
        assert!(!c.kitty);
        // A terminal with both answers both.
        let c = parse_reply("\x1b_Gi=4919;OK\x1b\\\x1b[?62;4;22c");
        assert!(c.kitty && c.sixel);
        // 14 and 44 contain a 4 but are not one.
        let c = parse_reply("\x1b[?62;14;44;22c");
        assert!(
            !c.sixel,
            "the attribute is the whole field, not a substring"
        );
    }

    #[test]
    fn a_refusal_is_not_a_capability() {
        // kitty answers failures on the same channel, so the OK is what counts.
        let c = parse_reply("\x1b_Gi=4919;ENOTSUPPORTED:no graphics\x1b\\\x1b[?62;22c");
        assert!(!c.kitty);
        assert_eq!(parse_reply(""), Caps::default(), "no answer is no support");
        assert_eq!(parse_reply("\x1b[?62;22c"), Caps::default());
    }

    /// DA1 is what makes the read terminable: it always comes, and it comes
    /// last, so it is proof the kitty query has been dealt with.
    #[test]
    fn the_read_ends_on_the_device_attributes_reply() {
        assert!(!done(""));
        assert!(
            !done("\x1b_Gi=4919;OK\x1b\\"),
            "graphics reply alone is not the end"
        );
        assert!(
            !done("\x1b[?62;4"),
            "a partial attribute list is not the end"
        );
        assert!(done("\x1b[?62;4;22c"));
        assert!(done("\x1b_Gi=4919;OK\x1b\\\x1b[?62;22c"));
    }
}
