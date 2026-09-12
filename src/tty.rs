//! Leaving the terminal usable when heft does not exit through `ui::run`.
//!
//! `ui::run` restores on both the normal and the error path, which covers
//! every exit heft chooses. It does not cover the two it does not choose. The
//! release profile is `panic = abort`, so a panic never unwinds and no
//! destructor or `?` ever runs; and SIGTERM, SIGHUP and SIGQUIT terminate
//! outright. Either way the shell is left in raw mode inside the alternate
//! screen — no echo, no line editing — until the user knows to type `reset`.
//!
//! The signal handler restores directly rather than setting a flag for the
//! event loop to notice: a flag only works while the loop is still turning,
//! and the case worth surviving is the one where it is not. That means
//! everything it calls must be async-signal-safe, which `tcsetattr`, `write`
//! and `_exit` are, and `println!`, allocation and `Drop` are not.

use std::io::Write;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cursor back on, alternate screen off. Written as bytes because the handler
/// cannot allocate; `ui` uses crossterm for the same thing on its own path.
const RESTORE: &[u8] = b"\x1b[?25h\x1b[?1049l";

static ARMED: AtomicBool = AtomicBool::new(false);
/// A shared memory object `kgp` has handed to the terminal but not yet seen
/// reclaimed. The handler unlinks it: a signal otherwise leaves up to an
/// image's worth of tmpfs behind, and heft's contract is that the only thing
/// it writes is its config directory.
static SHM_HELD: AtomicBool = AtomicBool::new(false);
/// The path, NUL-terminated, built by `hold_shm` so the handler never has to
/// allocate one. `/dev/shm` is where Linux puts POSIX shared memory, and heft
/// is Linux only.
static mut SHM_PATH: [u8; 128] = [0; 128];
/// The terminal settings from before raw mode, kept for the handler. A signal
/// handler cannot ask crossterm for them, and it cannot allocate to store them.
static mut SAVED: MaybeUninit<libc::termios> = MaybeUninit::uninit();

/// Captures the current terminal settings, installs a panic hook, and takes
/// over the signals that would otherwise kill heft mid-screen.
///
/// Call once, before raw mode. Safe to call when stdout is not a terminal:
/// `tcgetattr` fails, nothing is armed, and the hooks stay out of the way.
pub(crate) fn guard() {
    // SAFETY: written once here before ARMED is set, and read only by the
    // handler, which cannot run until ARMED is true.
    let ok = unsafe { libc::tcgetattr(libc::STDIN_FILENO, (&raw mut SAVED).cast()) } == 0;
    if !ok {
        return;
    }
    ARMED.store(true, Ordering::SeqCst);

    // Runs even under `panic = abort`: the hook fires, then the process
    // aborts. Restoring first is what lets the message be read at all — the
    // alternate screen is discarded on exit, taking the panic with it.
    let next = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        next(info);
    }));

    for sig in [libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT, libc::SIGINT] {
        // Through a fn pointer, not straight to an integer: casting a
        // function item to usize is what clippy flags, and the pointer is
        // what `signal` actually wants.
        let handler = on_signal as extern "C" fn(libc::c_int);
        // SAFETY: `on_signal` calls only async-signal-safe functions.
        unsafe { libc::signal(sig, handler as libc::sighandler_t) };
    }
}

/// 128 + signal is what a shell reports for a signalled child, so `$?` says
/// which signal even though heft exited on its own terms.
extern "C" fn on_signal(sig: libc::c_int) {
    restore();
    drop_shm();
    // Not `process::exit`: that runs atexit handlers and destructors, neither
    // of which is safe here.
    unsafe { libc::_exit(128 + sig) }
}

/// Idempotent, and a no-op when `guard` never armed.
fn restore() {
    if !ARMED.swap(false, Ordering::SeqCst) {
        return;
    }
    // SAFETY: SAVED was initialised by `guard` before ARMED became true, and
    // the swap above means only one caller ever reaches this.
    unsafe {
        libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, (&raw const SAVED).cast());
        libc::write(libc::STDOUT_FILENO, RESTORE.as_ptr().cast(), RESTORE.len());
    }
}

/// Remember one shared memory object to reclaim if heft is killed. Replaces
/// whatever was held: `kgp` keeps exactly one alive at a time, and unlinks
/// the previous itself on the ordinary path.
///
/// A name too long for the buffer holds nothing rather than a truncated path,
/// which would unlink something else.
pub(crate) fn hold_shm(name: &str) {
    const DIR: &[u8] = b"/dev/shm/";
    let leaf = name.trim_start_matches('/').as_bytes();
    if leaf.is_empty() || DIR.len() + leaf.len() >= 128 {
        return;
    }
    SHM_HELD.store(false, Ordering::SeqCst);
    // SAFETY: the handler reads this only while SHM_HELD is true, and it is
    // false for the whole of this write.
    // Written through the raw pointer rather than as a slice: taking a
    // reference to a static mut the handler also reads is what the aliasing
    // rules forbid.
    unsafe {
        let p = (&raw mut SHM_PATH).cast::<u8>();
        std::ptr::copy_nonoverlapping(DIR.as_ptr(), p, DIR.len());
        std::ptr::copy_nonoverlapping(leaf.as_ptr(), p.add(DIR.len()), leaf.len());
        p.add(DIR.len() + leaf.len()).write(0);
    }
    SHM_HELD.store(true, Ordering::SeqCst);
}

/// The object came back by the ordinary route, so the handler must not touch
/// that path again -- by then it may name a different object.
pub(crate) fn release_shm() {
    SHM_HELD.store(false, Ordering::SeqCst);
}

/// `unlink` is on the async-signal-safe list; `shm_unlink` is not, and on
/// Linux it is this call under `/dev/shm` anyway.
fn drop_shm() {
    if !SHM_HELD.swap(false, Ordering::SeqCst) {
        return;
    }
    // SAFETY: NUL-terminated by `hold_shm`, which completed before SHM_HELD
    // became true, and the swap means only one caller reaches this.
    unsafe { libc::unlink((&raw const SHM_PATH).cast()) };
}

/// Hands the terminal back on the ordinary path and disarms the handler, so a
/// later signal cannot write escape codes over a shell prompt.
pub(crate) fn released() {
    ARMED.store(false, Ordering::SeqCst);
    let _ = std::io::stdout().flush();
}
