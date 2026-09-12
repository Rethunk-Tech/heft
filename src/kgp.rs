//! TREND drawn as a kitty-graphics-protocol image rather than block characters.
//!
//! Opt-in (`--trend kitty`), never detected. `TERM` names a terminal, not what
//! it implements, and the alternative is the DA1-style handshake heft has
//! deliberately never done: a write, a read, and a timeout before the first
//! frame. The flag asserts the terminal the same way `--glyphs unicode` asserts
//! the font.
//!
//! One image for the whole column, not one per row. The protocol's row
//! diacritics exist to index into an image, so a single transmission covers
//! every visible row: one shared-memory object and one escape per frame
//! instead of one per row, and no per-row image lifecycle to leak.
//!
//! Two transports, because one of them cannot work everywhere:
//!
//! * `t=s` writes the pixels to a POSIX shared memory object and sends only
//!   its name, so a frame costs tens of bytes whatever the image weighs. It
//!   requires the terminal to be on this machine.
//! * `t=d` sends the pixels inline, base64 in 4096-byte chunks. That is the
//!   only thing that can work when the terminal is at the other end of an ssh
//!   connection, and it costs roughly 4/3 of the raw image per frame.
//!
//! `SSH_CONNECTION` / `SSH_TTY` picks between them. A wrong guess is quiet
//! rather than wrong: every escape carries `q=2`, so a terminal that cannot
//! read the object reports nothing instead of printing an error into the table.

use std::collections::VecDeque;
use std::ffi::CString;
use std::io::{self, Write};

/// Image id, carried in the placeholder's foreground colour. `0x686566` is
/// "hef": ids are a global namespace shared with every other program drawing
/// on this terminal, and a low number is what everything else picks. The top
/// byte is zero, so the third "most significant byte" diacritic is never
/// needed.
const IMAGE_ID: u32 = 0x0068_6566;

/// The placeholder character every cell of the image is drawn with.
const PLACEHOLDER: char = '\u{10eeee}';

/// Row and column numbers are spelled with these combining marks, in this
/// order, from kitty's `gen/rowcolumn-diacritics.txt`. The whole table is
/// carried rather than the first handful: the row index is a table row, so a
/// terminal taller than the list would draw an image for the rows it covered
/// and characters for the rest.
/// Row and column numbers are spelled with these combining marks, in this
/// order, from kitty's `gen/rowcolumn-diacritics.txt`. The whole table is
/// carried rather than the first handful: the row index is a table row, so a
/// terminal taller than the list would draw an image for the rows it covered
/// and characters for the rest.
///
/// One string rather than an array: it is a contiguous list read front to
/// back, and `MAX_BANDS` entries one per line was 2% of the crate.
const DIACRITICS: &str = "\u{0305}\u{030d}\u{030e}\u{0310}\u{0312}\u{033d}\u{033e}\u{033f}\u{0346}\
    \u{034a}\u{034b}\u{034c}\u{0350}\u{0351}\u{0352}\u{0357}\u{035b}\u{0363}\
    \u{0364}\u{0365}\u{0366}\u{0367}\u{0368}\u{0369}\u{036a}\u{036b}\u{036c}\
    \u{036d}\u{036e}\u{036f}\u{0483}\u{0484}\u{0485}\u{0486}\u{0487}\u{0592}\
    \u{0593}\u{0594}\u{0595}\u{0597}\u{0598}\u{0599}\u{059c}\u{059d}\u{059e}\
    \u{059f}\u{05a0}\u{05a1}\u{05a8}\u{05a9}\u{05ab}\u{05ac}\u{05af}\u{05c4}\
    \u{0610}\u{0611}\u{0612}\u{0613}\u{0614}\u{0615}\u{0616}\u{0617}\u{0657}\
    \u{0658}\u{0659}\u{065a}\u{065b}\u{065d}\u{065e}\u{06d6}\u{06d7}\u{06d8}\
    \u{06d9}\u{06da}\u{06db}\u{06dc}\u{06df}\u{06e0}\u{06e1}\u{06e2}\u{06e4}\
    \u{06e7}\u{06e8}\u{06eb}\u{06ec}\u{0730}\u{0732}\u{0733}\u{0735}\u{0736}\
    \u{073a}\u{073d}\u{073f}\u{0740}\u{0741}\u{0743}\u{0745}\u{0747}\u{0749}\
    \u{074a}\u{07eb}\u{07ec}\u{07ed}\u{07ee}\u{07ef}\u{07f0}\u{07f1}\u{07f3}\
    \u{0816}\u{0817}\u{0818}\u{0819}\u{081b}\u{081c}\u{081d}\u{081e}\u{081f}\
    \u{0820}\u{0821}\u{0822}\u{0823}\u{0825}\u{0826}\u{0827}\u{0829}\u{082a}\
    \u{082b}\u{082c}\u{082d}\u{0951}\u{0953}\u{0954}\u{0f82}\u{0f83}\u{0f86}\
    \u{0f87}\u{135d}\u{135e}\u{135f}\u{17dd}\u{193a}\u{1a17}\u{1a75}\u{1a76}\
    \u{1a77}\u{1a78}\u{1a79}\u{1a7a}\u{1a7b}\u{1a7c}\u{1b6b}\u{1b6d}\u{1b6e}\
    \u{1b6f}\u{1b70}\u{1b71}\u{1b72}\u{1b73}\u{1cd0}\u{1cd1}\u{1cd2}\u{1cda}\
    \u{1cdb}\u{1ce0}\u{1dc0}\u{1dc1}\u{1dc3}\u{1dc4}\u{1dc5}\u{1dc6}\u{1dc7}\
    \u{1dc8}\u{1dc9}\u{1dcb}\u{1dcc}\u{1dd1}\u{1dd2}\u{1dd3}\u{1dd4}\u{1dd5}\
    \u{1dd6}\u{1dd7}\u{1dd8}\u{1dd9}\u{1dda}\u{1ddb}\u{1ddc}\u{1ddd}\u{1dde}\
    \u{1ddf}\u{1de0}\u{1de1}\u{1de2}\u{1de3}\u{1de4}\u{1de5}\u{1de6}\u{1dfe}\
    \u{20d0}\u{20d1}\u{20d4}\u{20d5}\u{20d6}\u{20d7}\u{20db}\u{20dc}\u{20e1}\
    \u{20e7}\u{20e9}\u{20f0}\u{2cef}\u{2cf0}\u{2cf1}\u{2de0}\u{2de1}\u{2de2}\
    \u{2de3}\u{2de4}\u{2de5}\u{2de6}\u{2de7}\u{2de8}\u{2de9}\u{2dea}\u{2deb}\
    \u{2dec}\u{2ded}\u{2dee}\u{2def}\u{2df0}\u{2df1}\u{2df2}\u{2df3}\u{2df4}\
    \u{2df5}\u{2df6}\u{2df7}\u{2df8}\u{2df9}\u{2dfa}\u{2dfb}\u{2dfc}\u{2dfd}\
    \u{2dfe}\u{2dff}\u{a66f}\u{a67c}\u{a67d}\u{a6f0}\u{a6f1}\u{a8e0}\u{a8e1}\
    \u{a8e2}\u{a8e3}\u{a8e4}\u{a8e5}\u{a8e6}\u{a8e7}\u{a8e8}\u{a8e9}\u{a8ea}\
    \u{a8eb}\u{a8ec}\u{a8ed}\u{a8ee}\u{a8ef}\u{a8f0}\u{a8f1}\u{aab0}\u{aab2}\
    \u{aab3}\u{aab7}\u{aab8}\u{aabe}\u{aabf}\u{aac1}\u{fe20}\u{fe21}\u{fe22}\
    \u{fe23}\u{fe24}\u{fe25}\u{fe26}\u{10a0f}\u{10a38}\u{1d185}\u{1d186}\
    \u{1d187}\u{1d188}\u{1d189}\u{1d1aa}\u{1d1ab}\u{1d1ac}\u{1d1ad}\u{1d242}\
    \u{1d243}\u{1d244}";

/// The combining mark that spells index `i`.
fn diacritic(i: usize) -> Option<char> {
    DIACRITICS.chars().nth(i)
}

/// How many bands one image can index, which is how many table rows a single
/// transmission can cover. The row is spelled with one diacritic, so the table
/// bounds it, not the protocol.
pub(crate) const MAX_BANDS: usize = 297;

/// Refuse to paint an image larger than this. A terminal reporting an absurd
/// cell size would otherwise have heft allocate against it every frame.
const MAX_PIXELS: usize = 4 * 1024 * 1024;

/// Chunk size for `t=d`, from the protocol: payloads above this are split,
/// and every chunk but the last must be a multiple of 4 so it lands on a
/// base64 group boundary.
const CHUNK: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Transport {
    /// The pixels go through `/dev/shm`; the escape carries only a name.
    Shm,
    /// The pixels go through the escape itself, base64 and chunked.
    Direct,
}

/// `t=s` needs the terminal to be able to open a file this process created,
/// which is exactly what an ssh session does not give it. Neither variable is
/// set by a local terminal, and both are set by sshd, so their presence is the
/// question being asked -- not a guess about the terminal's identity.
pub(crate) const fn transport_for_env(ssh_connection: bool, ssh_tty: bool) -> Transport {
    if ssh_connection || ssh_tty {
        Transport::Direct
    } else {
        Transport::Shm
    }
}

/// Whether the terminal is at the other end of a connection, which is what
/// decides both the kitty transport and, in `ui::resolve_trend`, whether sixel
/// is the cheaper protocol.
pub(crate) fn is_remote() -> bool {
    detect_transport() == Transport::Direct
}

fn detect_transport() -> Transport {
    transport_for_env(
        std::env::var_os("SSH_CONNECTION").is_some(),
        std::env::var_os("SSH_TTY").is_some(),
    )
}

/// One painted TREND column: RGBA, one band of `cell_h` pixels per table row.
pub(crate) struct Image {
    pub(crate) w: u32,
    pub(crate) h: u32,
    pub(crate) rgba: Vec<u8>,
}

/// The cell grid in pixels, from the same `TIOCGWINSZ` the terminal answers
/// for rows and columns. `ws_xpixel` is zero on a terminal that does not
/// report it, and an image cannot be sized without it, so that is a `None`
/// and the caller falls back to characters.
pub(crate) fn cell_px() -> Option<(u32, u32)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: `ws` is a live, correctly sized winsize for the duration.
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &raw mut ws) } != 0 {
        return None;
    }
    let (cols, rows) = (u32::from(ws.ws_col), u32::from(ws.ws_row));
    let (xp, yp) = (u32::from(ws.ws_xpixel), u32::from(ws.ws_ypixel));
    if cols == 0 || rows == 0 || xp == 0 || yp == 0 {
        return None;
    }
    Some((xp / cols, yp / rows))
}

/// Paint one band per row: a line tracing that row's %CORE across the samples,
/// on the same fixed 0-`full` scale `ui::spark` uses, so the two renderings of
/// TREND say the same thing.
///
/// A line and not a filled bar. Filled, every row that was doing any work at
/// all became a solid block of colour with the movement hidden inside it --
/// the shape is the whole point of the column, and ink under the shape is not
/// carrying any of it.
///
/// A row with no history is left transparent rather than painted flat: that
/// is heft's blank, and the cursor's reverse-video highlight has to show
/// through it, which is why the image is RGBA and not RGB.
#[expect(
    clippy::cast_possible_truncation,
    reason = "band is under MAX_BANDS and the column under the table width, so both fit u32 pixel coordinates"
)]
pub(crate) fn paint(
    rows: &[Option<&VecDeque<f64>>],
    cols: u32,
    (cell_w, cell_h): (u32, u32),
    colour: [u8; 3],
    full: f64,
) -> Option<Image> {
    let w = cols.checked_mul(cell_w)?;
    let h = u32::try_from(rows.len()).ok()?.checked_mul(cell_h)?;
    let px = usize::try_from(w)
        .ok()?
        .checked_mul(usize::try_from(h).ok()?)?;
    if w == 0 || h == 0 || px > MAX_PIXELS {
        return None;
    }
    let mut rgba = vec![0u8; px * 4];
    let mut put = |x: u32, y: u32| {
        if x < w && y < h {
            let o = ((y * w + x) * 4) as usize;
            rgba[o] = colour[0];
            rgba[o + 1] = colour[1];
            rgba[o + 2] = colour[2];
            rgba[o + 3] = 0xff;
        }
    };
    for (band, buf) in rows.iter().enumerate() {
        let Some(buf) = buf.filter(|b| !b.is_empty()) else {
            continue;
        };
        let top = band as u32 * cell_h;
        let mut prev: Option<u32> = None;
        for (i, v) in buf.iter().enumerate().take(cols as usize) {
            let y = sample_y(*v, full, cell_h) + top;
            let x0 = i as u32 * cell_w;
            for x in x0..(x0 + cell_w).min(w) {
                put(x, y);
            }
            // Join this sample to the last, so nine marks read as one line
            // moving rather than as nine unrelated dashes.
            if let Some(py) = prev {
                for y in py.min(y)..=py.max(y) {
                    put(x0, y);
                }
            }
            prev = Some(y);
        }
    }
    Some(Image { w, h, rgba })
}

/// The row of pixels a sample sits on, within a band `cell_h` tall: the bottom
/// row at zero and the top row at or above `full`. Above full it pins rather
/// than rescaling, so one row flat out does not flatten every row beside it.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "frac is 0..=1 of a u32 height, so the rounded product is inside that \
height and never negative"
)]
fn sample_y(v: f64, full: f64, cell_h: u32) -> u32 {
    let last = cell_h.saturating_sub(1);
    // Spelled out rather than negating a comparison: these are floats, so
    // "not greater than zero" and "less than or equal to zero" differ on NaN,
    // and a NaN reaching the subtraction below would wrap the row index.
    if v.is_nan() || v <= 0.0 || full.is_nan() || full <= 0.0 {
        return last;
    }
    let frac = (v / full).min(1.0);
    last - (frac * f64::from(last)).round() as u32
}

/// The cells a table row puts in its TREND column to show band `band` of the
/// image. The foreground colour carries the image id and is set by the caller
/// as a ratatui style, never as an escape inside the text: ratatui writes a
/// cell's symbol literally, so an escape smuggled into one corrupts the frame.
///
/// Only the first cell spells out its row and column. The protocol reads a
/// bare placeholder as "the cell to the left, one column on", provided the
/// colours match -- which they do, since the whole column is one span.
pub(crate) fn placeholder(band: usize, cols: usize) -> Option<String> {
    let row = diacritic(band)?;
    let mut s = String::with_capacity(cols * 4);
    s.push(PLACEHOLDER);
    s.push(row);
    s.push(diacritic(0)?);
    for _ in 1..cols {
        s.push(PLACEHOLDER);
    }
    Some(s)
}

/// The image id as the RGB the placeholder's foreground must carry.
pub(crate) const fn id_rgb() -> (u8, u8, u8) {
    (
        ((IMAGE_ID >> 16) & 0xff) as u8,
        ((IMAGE_ID >> 8) & 0xff) as u8,
        (IMAGE_ID & 0xff) as u8,
    )
}

pub(crate) struct Kgp {
    transport: Transport,
    /// Hash of the last image sent, so a redraw for a keypress re-sends
    /// nothing. The loop draws on every poll timeout, not once per sample, and
    /// on `t=d` a frame is most of a megabyte.
    last: Option<u64>,
    /// Bumped per transmission so a new object never collides with one the
    /// terminal has not finished reading.
    seq: u64,
    /// The last shm name written, unlinked on the way out for the case where
    /// the terminal never read it and so never unlinked it itself.
    live: Option<String>,
}

impl Kgp {
    pub(crate) fn new() -> Self {
        Self::with_transport(detect_transport())
    }

    pub(crate) const fn with_transport(transport: Transport) -> Self {
        Self {
            transport,
            last: None,
            seq: 0,
            live: None,
        }
    }

    /// Transmit `img` and create its virtual placement, unless the identical
    /// image is already on screen. `cols`/`rows` are the cell rectangle the
    /// placeholders will cover.
    pub(crate) fn send(
        &mut self,
        w: &mut impl Write,
        img: &Image,
        cols: u32,
        rows: u32,
    ) -> io::Result<()> {
        let h = hash(&img.rgba);
        if self.last == Some(h) {
            return Ok(());
        }
        // Cleared before the write, not after: a failed transmission that
        // left `last` set would never be retried.
        self.last = None;
        // `a=T` transmits and places in one escape, `U=1` makes the placement
        // virtual (invisible until a placeholder cell references it), `f=32`
        // is RGBA, `q=2` silences failures so a terminal that cannot do this
        // does not print its complaint into the table.
        let head = format!(
            "a=T,U=1,i={IMAGE_ID},f=32,s={},v={},c={cols},r={rows},q=2",
            img.w, img.h
        );
        match self.transport {
            Transport::Shm => {
                let name = self.shm_name();
                shm_write(&name, &img.rgba)?;
                // Registered before the previous is dropped, so a signal
                // landing between the two always finds the live one.
                crate::tty::hold_shm(&name);
                // The terminal unlinks the object once it has read it. The
                // previous name is dropped only now, so it outlives the frame
                // that referenced it.
                if let Some(old) = self.live.replace(name.clone()) {
                    shm_unlink(&old);
                }
                let mut payload = String::new();
                b64(name.as_bytes(), &mut payload);
                write!(w, "\x1b_G{head},t=s;{payload}\x1b\\")?;
            }
            Transport::Direct => {
                let mut payload = String::with_capacity(img.rgba.len() * 4 / 3 + 4);
                b64(&img.rgba, &mut payload);
                write_chunked(w, &head, &payload)?;
            }
        }
        w.flush()?;
        self.last = Some(h);
        Ok(())
    }

    fn shm_name(&mut self) -> String {
        self.seq += 1;
        // A POSIX shm name is one leading slash and no others.
        format!("/heft-{}-{}", std::process::id(), self.seq)
    }

    /// Delete the image and free its data, and take back any shared memory
    /// the terminal never claimed. Called on the way out of the TUI, where
    /// `tty` restores the screen.
    pub(crate) fn teardown(&mut self, w: &mut impl Write) {
        let _ = write!(w, "\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\");
        let _ = w.flush();
        if let Some(name) = self.live.take() {
            crate::tty::release_shm();
            shm_unlink(&name);
        }
    }
}

/// Full control data on the first chunk only, `m=1` until the last, which is
/// `m=0` and empty when the payload divided evenly.
fn write_chunked(w: &mut impl Write, head: &str, payload: &str) -> io::Result<()> {
    let mut first = true;
    let mut rest = payload;
    while !rest.is_empty() {
        let take = CHUNK.min(rest.len());
        let (chunk, tail) = rest.split_at(take);
        let more = u8::from(!tail.is_empty());
        if first {
            write!(w, "\x1b_G{head},t=d,m={more};{chunk}\x1b\\")?;
            first = false;
        } else {
            write!(w, "\x1b_Gm={more};{chunk}\x1b\\")?;
        }
        rest = tail;
    }
    Ok(())
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding. Hand-rolled for the same reason the `/proc`
/// parsers are: it is a dozen lines and the alternative is a dependency.
fn b64(data: &[u8], out: &mut String) {
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
}

/// FNV-1a. Only ever compared against itself, so the bar is "does not collide
/// between two frames", not cryptographic.
fn hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

fn shm_unlink(name: &str) {
    if let Ok(c) = CString::new(name) {
        // SAFETY: a valid C string; ENOENT when the terminal already took it,
        // which is the ordinary case and not an error.
        unsafe { libc::shm_unlink(c.as_ptr()) };
    }
}

/// Create the object, fill it, and close: the terminal opens it by name,
/// reads it, then unlinks and closes it itself.
#[expect(
    clippy::cast_possible_wrap,
    reason = "the image is capped at MAX_PIXELS, four bytes each, far below off_t's positive range"
)]
fn shm_write(name: &str, bytes: &[u8]) -> io::Result<()> {
    let c = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let len = bytes.len();
    // SAFETY: every pointer below is checked before use, the mapping is the
    // length just ftruncate'd, and both the fd and the mapping are released
    // on every path out.
    unsafe {
        let fd = libc::shm_open(
            c.as_ptr(),
            libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
            0o600,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let close = |fd: i32, e: io::Error| {
            libc::close(fd);
            libc::shm_unlink(c.as_ptr());
            Err(e)
        };
        if libc::ftruncate(fd, len as libc::off_t) != 0 {
            return close(fd, io::Error::last_os_error());
        }
        let p = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        );
        if p == libc::MAP_FAILED {
            return close(fd, io::Error::last_os_error());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), len);
        libc::munmap(p, len);
        libc::close(fd);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(v: &[f64]) -> VecDeque<f64> {
        v.iter().copied().collect()
    }

    #[test]
    fn ssh_forces_the_inline_transport() {
        // Shared memory the terminal cannot open is not a fallback, it is a
        // blank column, so the question is asked of the session and not of
        // `TERM`.
        assert_eq!(transport_for_env(false, false), Transport::Shm);
        assert_eq!(transport_for_env(true, false), Transport::Direct);
        assert_eq!(transport_for_env(false, true), Transport::Direct);
    }

    #[test]
    fn base64_matches_the_worked_examples() {
        let mut s = String::new();
        b64(b"", &mut s);
        assert_eq!(s, "");
        for (input, want) in [
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            let mut s = String::new();
            b64(input.as_bytes(), &mut s);
            assert_eq!(s, want, "{input}");
        }
    }

    #[test]
    fn only_the_first_chunk_carries_control_data() {
        let mut out = Vec::new();
        let payload = "A".repeat(CHUNK + 8);
        write_chunked(&mut out, "a=T,f=32", &payload).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1b_Ga=T,f=32,t=d,m=1;AAA"), "{}", &s[..40]);
        // Every chunk but the last is a multiple of four, so a base64 group is
        // never split across two escapes.
        assert_eq!(CHUNK % 4, 0);
        assert!(
            s.contains("\x1b\\\x1b_Gm=0;"),
            "last chunk ends the sequence"
        );
        assert_eq!(s.matches("\x1b_G").count(), 2);
    }

    #[test]
    fn a_short_payload_is_one_chunk_that_closes_itself() {
        let mut out = Vec::new();
        write_chunked(&mut out, "a=T", "QUJD").unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s, "\x1b_Ga=T,t=d,m=0;QUJD\x1b\\");
    }

    #[test]
    fn the_first_cell_names_its_band_and_the_rest_run_on() {
        let p = placeholder(2, 9).unwrap();
        let chars: Vec<char> = p.chars().collect();
        assert_eq!(chars[0], PLACEHOLDER);
        assert_eq!(chars[1], diacritic(2).unwrap(), "row 2");
        assert_eq!(chars[2], diacritic(0).unwrap(), "column 0");
        // Nine cells: one spelled out, eight inheriting from the left.
        assert_eq!(chars.iter().filter(|c| **c == PLACEHOLDER).count(), 9);
        assert_eq!(chars.len(), 9 + 2);
        // A band past the table's own diacritics draws nothing rather than
        // drawing the wrong row.
        assert!(placeholder(MAX_BANDS, 9).is_none());
        assert_eq!(
            DIACRITICS.chars().count(),
            MAX_BANDS,
            "MAX_BANDS is the table length"
        );
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "a base64 index is 0..64 and the shifted groups are masked to one byte"
    )]
    fn b64_decode(s: &str) -> Vec<u8> {
        let idx = |c: u8| B64.iter().position(|b| *b == c).unwrap() as u32;
        let raw: Vec<u8> = s.bytes().filter(|b| *b != b'=').collect();
        let pad = s.bytes().filter(|b| *b == b'=').count();
        let mut out = Vec::new();
        for c in raw.chunks(4) {
            let mut n = 0u32;
            for (i, b) in c.iter().enumerate() {
                n |= idx(*b) << (18 - 6 * i);
            }
            out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]);
        }
        out.truncate(out.len() - pad);
        out
    }

    /// The bytes that reach the terminal, not the intent: control data,
    /// chunking and payload are asserted against what the protocol asks for
    /// and the pixels are decoded back out.
    #[test]
    fn the_inline_transport_round_trips_the_pixels() {
        let mut k = Kgp::with_transport(Transport::Direct);
        let img = paint(&[Some(&buf(&[1.0, 2.0]))], 2, (4, 8), [10, 20, 30], 100.0).unwrap();
        let mut out = Vec::new();
        k.send(&mut out, &img, 9, 1).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(
            s.starts_with(&format!(
                "\x1b_Ga=T,U=1,i={IMAGE_ID},f=32,s={},v={},c=9,r=1,q=2,t=d,m=",
                img.w, img.h
            )),
            "{}",
            &s[..70.min(s.len())]
        );
        assert!(s.ends_with("\x1b\\"));
        let payload: String = s
            .split("\x1b_G")
            .filter(|p| !p.is_empty())
            .map(|p| {
                let body = p.split_once(';').unwrap().1;
                body.trim_end_matches("\x1b\\").to_string()
            })
            .collect();
        assert_eq!(
            b64_decode(&payload),
            img.rgba,
            "the pixels survive the wire"
        );
    }

    /// The loop draws on every poll timeout, not once per sample. On the
    /// inline transport an unchanged frame re-sent is most of a megabyte.
    #[test]
    fn an_unchanged_image_is_not_sent_twice() {
        let mut k = Kgp::with_transport(Transport::Direct);
        let img = paint(&[Some(&buf(&[1.0]))], 1, (4, 8), [1, 2, 3], 100.0).unwrap();
        let mut first = Vec::new();
        k.send(&mut first, &img, 9, 1).unwrap();
        assert!(!first.is_empty());
        let mut again = Vec::new();
        k.send(&mut again, &img, 9, 1).unwrap();
        assert!(again.is_empty(), "same pixels, nothing on the wire");
        // A changed row sends again.
        let other = paint(&[Some(&buf(&[1.0, 9.0]))], 2, (4, 8), [1, 2, 3], 100.0).unwrap();
        let mut third = Vec::new();
        k.send(&mut third, &other, 9, 1).unwrap();
        assert!(!third.is_empty());
    }

    /// The shared-memory path is syscalls, so it is tested against the real
    /// object: the terminal opens it by name from `/dev/shm`, which is where
    /// this reads it back from.
    #[test]
    fn shared_memory_carries_the_pixels_and_can_be_reclaimed() {
        let name = format!("/heft-test-{}", std::process::id());
        let pixels: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        shm_write(&name, &pixels).unwrap();
        let path = format!("/dev/shm/{}", name.trim_start_matches('/'));
        assert_eq!(std::fs::read(&path).unwrap(), pixels, "the terminal's view");
        // A second create on a live name is refused rather than silently
        // handing the terminal a half-written object.
        assert!(shm_write(&name, &pixels).is_err());
        shm_unlink(&name);
        assert!(!std::path::Path::new(&path).exists(), "reclaimed");
        // Unlinking what the terminal already took is the ordinary case.
        shm_unlink(&name);
    }

    #[test]
    fn teardown_frees_the_image_data() {
        let mut k = Kgp::with_transport(Transport::Direct);
        let mut out = Vec::new();
        k.teardown(&mut out);
        let s = String::from_utf8(out).unwrap();
        // `D=I` uppercase: delete the placements *and* free the data, so the
        // image does not outlive heft in the terminal's store.
        assert_eq!(s, format!("\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\"));
    }

    #[test]
    fn the_id_rides_in_the_foreground_colour() {
        let (r, g, b) = id_rgb();
        assert_eq!(
            (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
            IMAGE_ID
        );
        assert_eq!(IMAGE_ID >> 24, 0, "no third diacritic needed");
    }

    /// The scale rule, which is the whole reason the column is readable: the
    /// floor at zero, the ceiling at `full`, and anything above `full` pinned
    /// rather than rescaling every row beside it.
    #[test]
    fn a_sample_sits_where_the_fixed_scale_puts_it() {
        assert_eq!(sample_y(0.0, 100.0, 8), 7, "zero is the floor");
        assert_eq!(sample_y(100.0, 100.0, 8), 0, "full scale is the ceiling");
        assert_eq!(
            sample_y(400.0, 100.0, 8),
            0,
            "above full pins, never rescales"
        );
        // Eight pixel rows span 0..=7, so half scale is 3.5 steps up from
        // the floor and lands on row 3.
        assert_eq!(sample_y(50.0, 100.0, 8), 3);
        // Nothing on screen has a figure yet: everything is on the floor
        // rather than dividing by zero.
        assert_eq!(sample_y(5.0, 0.0, 8), 7);
    }

    #[test]
    fn a_line_joins_the_samples_and_leaves_the_rest_clear() {
        let cell = (4, 8);
        let rows = [Some(&buf(&[0.0, 100.0])), None];
        let img = paint(&rows, 2, cell, [1, 2, 3], 100.0).unwrap();
        assert_eq!((img.w, img.h), (8, 16));
        let at = |x: u32, y: u32| img.rgba[((y * img.w + x) * 4 + 3) as usize];
        // Sample 0 is zero: a mark on the floor, and nothing above it.
        assert_eq!(at(0, 7), 0xff);
        assert_eq!(at(0, 6), 0, "a line, not a bar filled up from the floor");
        // Sample 1 is full scale: a mark on the ceiling, and nothing below it
        // except the riser joining it to the sample before.
        assert_eq!(at(7, 0), 0xff);
        assert_eq!(at(7, 7), 0, "no fill under the mark");
        // The riser sits on the new sample's first column.
        assert!((0..8).all(|y| at(4, y) == 0xff), "samples are joined");
        // A row with no history is transparent, not flat: heft's blank, and
        // the cursor highlight has to show through it.
        assert!(
            img.rgba[(img.w * cell.1 * 4) as usize..]
                .iter()
                .all(|b| *b == 0)
        );
    }

    /// The reported defect: against its own peak a row sitting flat at 2% drew
    /// every sample at full height, so the column was a solid block and the
    /// movement it exists to show was not in it.
    #[test]
    fn a_flat_busy_row_is_a_flat_line_near_the_floor() {
        let rows = [Some(&buf(&[2.0, 2.0, 2.0]))];
        let img = paint(&rows, 3, (4, 16), [9, 9, 9], 100.0).unwrap();
        let lit: Vec<u32> = (0..img.h)
            .filter(|y| (0..img.w).any(|x| img.rgba[((y * img.w + x) * 4 + 3) as usize] != 0))
            .collect();
        assert_eq!(lit.len(), 1, "a flat row is one row of pixels, not a block");
        assert_eq!(lit[0], 15, "2 of 100 rounds onto the floor");
        // Flat at zero draws too: a history that is flat is still a history.
        let zero = [Some(&buf(&[0.0, 0.0]))];
        let img = paint(&zero, 2, (2, 4), [9, 9, 9], 100.0).unwrap();
        assert!(img.rgba.iter().any(|b| *b != 0));
    }

    #[test]
    fn an_absurd_cell_or_an_empty_table_is_refused() {
        let rows = [Some(&buf(&[0.0, 0.0]))];
        assert!(paint(&rows, 2, (100_000, 100_000), [0, 0, 0], 100.0).is_none());
        assert!(paint(&[], 9, (8, 16), [0, 0, 0], 100.0).is_none());
    }
}
