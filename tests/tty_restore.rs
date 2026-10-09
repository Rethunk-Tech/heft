//! HUMANS.md promises that on SIGTERM or a closed terminal heft restores the
//! terminal and exits `128 +` the signal. Only a real pty can observe that:
//! the restore is bytes written to the terminal from inside a signal handler.

use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const ENTER_ALT_SCREEN: &[u8] = b"\x1b[?1049h";
const RESTORE: &[u8] = b"\x1b[?25h\x1b[?1049l";

fn open_pty() -> (OwnedFd, OwnedFd) {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: both out-pointers are valid; the null name, termios and winsize
    // are all optional.
    let rc = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: openpty returned two fresh descriptors that nothing else owns.
    unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
}

fn exit_after(signal: libc::c_int) -> (i32, Vec<u8>) {
    let (master, slave) = open_pty();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_heft"));
    // No saved view or rules of the developer's may change what is drawn.
    cmd.args(["--trend", "chars"])
        .env(
            "XDG_CONFIG_HOME",
            std::env::temp_dir().join("heft-no-config"),
        )
        .env(
            "HEFT_RULES_PATH",
            std::env::temp_dir().join("heft-no-rules"),
        );
    let dup = |fd: &OwnedFd| Stdio::from(fd.try_clone().expect("dup slave"));
    cmd.stdin(dup(&slave))
        .stdout(dup(&slave))
        .stderr(dup(&slave));
    // SAFETY: setsid and ioctl are async-signal-safe, as pre_exec requires.
    // Making the slave the controlling terminal is what a real terminal does.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().expect("spawn heft");
    drop(slave);
    drop(cmd);

    let mut out = Vec::new();
    let mut file = std::fs::File::from(master);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut buf = [0u8; 4096];
    let mut sent = false;
    loop {
        assert!(Instant::now() < deadline, "heft never left the alt screen");
        // A blocking read ends when the child exits and the slave closes (EIO).
        match file.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
        }
        if !sent
            && out
                .windows(ENTER_ALT_SCREEN.len())
                .any(|w| w == ENTER_ALT_SCREEN)
        {
            // SAFETY: the pid is the child spawned above.
            let pid = libc::pid_t::try_from(child.id()).expect("pid fits pid_t");
            assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
            sent = true;
        }
    }
    let status = child.wait().expect("wait");
    assert!(sent, "heft exited before entering the alt screen");
    (
        status
            .code()
            .unwrap_or_else(|| -status.signal().unwrap_or(0)),
        out,
    )
}

#[test]
fn a_signal_restores_the_terminal_and_exits_128_plus_it() {
    for signal in [libc::SIGTERM, libc::SIGHUP] {
        let (code, out) = exit_after(signal);
        assert_eq!(code, 128 + signal, "exit status for signal {signal}");
        assert!(
            out.ends_with(RESTORE),
            "output must end by showing the cursor and leaving the alt screen: {:?}",
            String::from_utf8_lossy(&out[out.len().saturating_sub(40)..])
        );
    }
}
