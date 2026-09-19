use std::borrow::Cow;
use std::ffi::CStr;
use std::fs;
use std::io;
use std::num::NonZero;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use rustix::buffer::spare_capacity;
use rustix::fs::{Dir, Mode, OFlags};
use rustix::io::Errno;
use rustix::path::DecInt;

use crate::containers::{ContainerIndex, InspectCache};
use crate::cpu;
use crate::group;
use crate::rules::Rules;
use crate::types::{GpuCounters, HostHeader, HostTree, PidMap, Process};
use crate::{gpu, io as pio, net, psi};

/// The walk is around seven small procfs reads per pid and no computation
/// worth the name, so nearly all of its wall clock is the kernel building
/// those files one at a time while this process waits. Splitting the pid list
/// across threads overlaps that wait.
///
/// Measured on a 723-pid, 32-CPU host, pinned with `taskset`, ten interleaved
/// runs each, median: `--once --interval 0.05` (a plain walk, then a PSS walk)
/// takes 618 ms on one CPU, 376 ms on four and 170 ms on 32; `--fixture` (one
/// plain walk) 37.4, 19.0 and 11.2 ms. The PSS walk gains less, because
/// `smaps_rollup` makes the kernel walk that process's page tables, which is
/// memory-bound rather than latency-bound, so a PSS tick still stretches its
/// interval, exactly as HUMANS.md says it does.
///
/// Parallel walkers cost kernel time, though: idle `--json --follow` pinned to
/// 1, 4, 8, 16 and 32 CPUs spent 3.96, 4.19, 4.27, 4.57 and 5.25-5.66 s of CPU
/// per 30 s. So the continuous modes, which pay that every tick forever, take
/// `FOLLOW_WALKERS`, while `--once`, one-shot `--json`, `--fixture` and
/// `--explain`, where latency is the point, take every CPU.
///
/// Workers live on the Sampler (so `--once` / `--json` still pool their two
/// walks) and `sample_stream` drop joins them. A `thread::scope` per sample
/// was refused because spawn cost tens of microseconds; the RSS is why a
/// pool exists now. New glibc arenas each tick climbed ~15 MiB every 5s PSS
/// tick to ~488 MiB. `MALLOC_ARENA_MAX=2` plateaued at 39 MiB, so arenas
/// dominate the `HostTree`.
///
/// Workers take equal-count slices. Pulling one pid at a time off a shared
/// index was measured and refused: on ~750 pids and 32 threads it shortened
/// the walk (PSS tick 98 to 82 ms, plain tick 8.7 to 6.5 ms) because one slow
/// `smaps_rollup` no longer stalls a slice, but it kept every worker in procfs
/// until the list drained, and idle `--json --follow` rose from 5.1 s to 6.6 s
/// of CPU per 30 s, nearly all system time. Idle cost outranks a shorter tick.
///
/// Each worker has its own result channel, so a worker that panics in a
/// debug build drops its sender and `collect` fails on that `recv` instead of
/// waiting forever; release is `panic = "abort"`. Dropping the pool drops the
/// job senders, which ends every worker.
struct WalkPool {
    workers: Vec<(mpsc::Sender<WalkJob>, mpsc::Receiver<Walked>)>,
}

/// One worker's slice of the walk.
type Walked = Vec<(u32, Process)>;

/// Walk workers for the TUI and `--follow`; see `WalkPool`.
const FOLLOW_WALKERS: usize = 4;

struct WalkJob {
    pids: Vec<u32>,
    want_pss: bool,
    want_swap: bool,
    prev: Option<Arc<PidMap<Process>>>,
}

impl WalkPool {
    /// At most `max` workers, and never more than the CPUs this process may run on.
    /// Fewer when a thread cannot start (`RLIMIT_NPROC`, a cgroup's `pids.max`),
    /// which is when a monitor is most wanted; with none, `collect` walks on the
    /// calling thread.
    fn new(max: usize) -> Self {
        let n = thread::available_parallelism()
            .map_or(1, NonZero::get)
            .min(max);
        let workers = (0..n)
            .map_while(|i| {
                let (job_tx, job_rx) = mpsc::channel();
                let (result_tx, result_rx) = mpsc::channel();
                thread::Builder::new()
                    .name(format!("heft-walk-{i}"))
                    .spawn(move || walk_worker(&job_rx, &result_tx))
                    .ok()?;
                Some((job_tx, result_rx))
            })
            .collect();
        Self { workers }
    }

    fn collect(
        &self,
        want_pss: bool,
        want_swap: bool,
        prev: Option<&Arc<PidMap<Process>>>,
    ) -> PidMap<Process> {
        let Some(mut dir) = proc_dir().and_then(|fd| Dir::read_from(fd).ok()) else {
            return PidMap::default();
        };
        let mut pids = Vec::new();
        while let Some(Ok(ent)) = dir.read() {
            if let Some(pid) = atoi(ent.file_name().to_bytes()).and_then(|n| u32::try_from(n).ok())
            {
                pids.push(pid);
            }
        }
        if pids.is_empty() {
            return PidMap::default();
        }
        if self.workers.is_empty() {
            return walk(
                &pids,
                want_pss,
                want_swap,
                prev.map(|m| &**m),
                &mut Vec::new(),
            )
            .into_iter()
            .collect();
        }
        let chunk = pids.len().div_ceil(self.workers.len());
        let busy = pids.chunks(chunk).len();
        for ((tx, _), slice) in self.workers.iter().zip(pids.chunks(chunk)) {
            tx.send(WalkJob {
                pids: slice.to_vec(),
                want_pss,
                want_swap,
                prev: prev.cloned(),
            })
            .expect("a /proc walk thread exited");
        }
        let mut out = PidMap::with_capacity_and_hasher(pids.len(), Default::default());
        for (_, rx) in &self.workers[..busy] {
            // A dead worker is a bug in a `/proc` parser; carrying on would
            // publish a tree quietly missing a chunk of the machine.
            out.extend(rx.recv().expect("a /proc walk thread exited"));
        }
        out
    }
}

fn walk_worker(job_rx: &mpsc::Receiver<WalkJob>, result_tx: &mpsc::Sender<Walked>) {
    let mut buf = Vec::new();
    while let Ok(job) = job_rx.recv() {
        let out = walk(
            &job.pids,
            job.want_pss,
            job.want_swap,
            job.prev.as_deref(),
            &mut buf,
        );
        if result_tx.send(out).is_err() {
            break;
        }
    }
}

fn walk(
    pids: &[u32],
    want_pss: bool,
    want_swap: bool,
    prev: Option<&PidMap<Process>>,
    buf: &mut Vec<u8>,
) -> Walked {
    pids.iter()
        .filter_map(|&pid| {
            let before = prev.and_then(|m| m.get(&pid));
            Some((pid, read_pid(pid, want_pss, want_swap, before, buf)?))
        })
        .collect()
}

/// `/proc/<pid>` opened once, so every file under it is an `openat` of one
/// name rather than a path the kernel resolves from `/` again. `O_PATH`
/// because the handle is only ever a base for those lookups.
///
/// Carrying the handle to the next tick on `Process` was measured and refused.
/// On 819 pids it saved an `open` and a `close` per pid per tick (7% of
/// syscalls) with idle CPU unchanged, 1.66 to 1.64 s per 30 s, but one-shot
/// `--fixture` went from 8.7 to 68 ms: holding one fd per pid grows the fd
/// table while the walk threads share it, and opens stalled 15 to 20 ms at
/// each growth.
fn open_pid(pid: u32) -> Option<OwnedFd> {
    let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
    rustix::fs::openat(proc_dir()?, DecInt::new(pid), flags, Mode::empty()).ok()
}

/// `/proc` under `--proc-root`, opened once for the run: the root cannot
/// change, and every pid is then a one-name lookup from here.
fn proc_dir() -> Option<&'static OwnedFd> {
    static DIR: OnceLock<Option<OwnedFd>> = OnceLock::new();
    DIR.get_or_init(|| {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        rustix::fs::open(crate::root::path("/proc").as_str(), flags, Mode::empty()).ok()
    })
    .as_ref()
}

/// `name` under `dir` into `buf`, replacing what it held, stopping once it
/// holds more than `cap` bytes so an oversized file shows as `len() > cap`
/// without being read whole. `None` when it cannot be opened or read.
///
/// Not `fs::read`: that `statx`es every file to size a buffer, and procfs
/// reports size 0, so the call bought nothing. With the `open_pid` handle and
/// a buffer reused across the walk, `--once` on ~745 pids went from 91.9k
/// syscalls to 66.8k (`read` 39.1k to 19.4k, `statx` 9.3k to 1.0k), and a
/// one-CPU plain walk from 41.3 ms [40.3-42.9] to 35.8 [34.8-37.0], median and
/// p10-p90 over ten interleaved runs. The 32-thread pool shows no wall change.
///
/// A short read is the end: every caller reads a procfs `seq_file` or
/// `cmdline`, and both fill the buffer whenever the file has more, so the read
/// that would only return 0 is skipped. That cut `read` calls from 63.2k to
/// 38.6k over 6 s of `--json --follow` on 819 pids. A file that can return a
/// short read before its end must not come through here.
pub(crate) fn read_at(
    dir: impl AsFd,
    name: impl rustix::path::Arg,
    buf: &mut Vec<u8>,
    cap: usize,
) -> Option<&[u8]> {
    buf.clear();
    let fd = rustix::fs::openat(dir, name, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty()).ok()?;
    while buf.len() <= cap {
        buf.reserve(4096);
        let spare = buf.capacity() - buf.len();
        match rustix::io::read(&fd, spare_capacity(buf)) {
            Ok(n) if n < spare => break,
            Ok(_) | Err(Errno::INTR) => {}
            Err(_) => return None,
        }
    }
    Some(buf)
}

/// `read_at` uncapped, as text. Invalid UTF-8 is replaced rather than `None`:
/// a process chooses its own name bytes, and `status` carries that name, so a
/// strict read would let one `PR_SET_NAME` reset its uid to 0.
pub(crate) fn read_str(
    dir: impl AsFd,
    name: impl rustix::path::Arg,
    buf: &mut Vec<u8>,
) -> Option<Cow<'_, str>> {
    Some(String::from_utf8_lossy(read_at(
        dir,
        name,
        buf,
        usize::MAX,
    )?))
}

/// A kernel symlink target never exceeds `PATH_MAX` (4096 with its NUL), so a
/// buffer this size is never truncated.
pub(crate) type LinkBuf = [u8; 4096];

pub(crate) fn read_link_at(
    dir: impl AsFd,
    name: impl rustix::path::Arg,
    out: &mut LinkBuf,
) -> Option<&[u8]> {
    let n = rustix::fs::readlinkat_raw(dir, name, &mut out[..]).ok()?;
    out.get(..n)
}

pub(crate) fn read_pid(
    pid: u32,
    want_pss: bool,
    want_swap: bool,
    prev: Option<&Process>,
    buf: &mut Vec<u8>,
) -> Option<Process> {
    let dir = open_pid(pid)?;
    let parsed = parse_stat(read_at(&dir, c"stat", buf, usize::MAX)?)?;
    // A kernel thread's other files say only uid 0, no exe, no argv, no memory,
    // and it is System on the flag alone, so they are not read: over half the
    // pids on a desktop, and five files each, every tick.
    let walks = prev
        .filter(|p| p.starttime_ticks.is_some() && p.starttime_ticks == parsed.starttime_ticks)
        .map_or(0, |p| p.walks.wrapping_add(1));
    let (uid, exe, cmdline, cgroup, rss_pages) = if parsed.kthread {
        (0, None, Vec::new(), String::new(), Some(0))
    } else {
        let exe = read_exe(&dir);
        let (uid, cmdline, cgroup) = match prev {
            Some(p) if carries_identity(p, walks, &parsed.comm, exe.as_deref()) => {
                (p.uid, p.cmdline.clone(), p.cgroup.clone())
            }
            _ => (
                read_at(&dir, c"status", buf, usize::MAX)
                    .and_then(parse_uid)
                    .unwrap_or(0),
                read_cmdline(&dir, buf),
                read_str(&dir, c"cgroup", buf)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            ),
        };
        (
            uid,
            exe,
            cmdline,
            cgroup,
            // Not `stat` field 24: `do_task_stat` prints `get_mm_rss`, the
            // `percpu_counter_read_positive` estimate, where `task_statm`
            // prints `get_mm_counter_sum`. On 7.1, 240 of 716 pids disagreed
            // (avahi-daemon 0 pages against 468), so dropping this read was
            // refused.
            read_str(&dir, c"statm", buf).and_then(|s| parse_rss_pages(&s)),
        )
    };
    // PSS is a level, not a rate. Kernel threads have no rollup. Prime and
    // TUI ticks between `--pss-interval` reuse last (new PIDs stay blank).
    let rollup = rollup_for(
        want_pss,
        want_swap,
        parsed.kthread,
        prev,
        (parsed.starttime_ticks, rss_pages),
        || pio::read_rollup_kb(&dir, want_swap, buf),
    );
    // PF_KTHREAD has no userspace /proc/pid/io or drm fdinfo.
    let (read_bytes, write_bytes, (gpu, drm_fds)) = if parsed.kthread {
        (None, None, (GpuCounters::default(), Vec::new()))
    } else {
        let (r, w) = pio::read_io(&dir, buf);
        // want_pss is the residual GPU fdinfo walk (PSS / --once) when dri/drm
        // names were found but yielded no metrics; empty prefilter skips it.
        // It is also when the fd table is rescanned: between PSS ticks the
        // same process keeps its last drm fd list.
        let carried = prev
            .filter(|p| !want_pss && p.starttime_ticks.is_some())
            .filter(|p| p.starttime_ticks == parsed.starttime_ticks)
            .map(|p| p.drm_fds.as_slice());
        (r, w, gpu::read_pid(&dir, want_pss, carried, buf))
    };
    Some(Process {
        pid,
        ppid: parsed.ppid,
        pgrp: parsed.pgrp,
        uid,
        kthread: parsed.kthread,
        comm: parsed.comm,
        exe,
        cmdline,
        cgroup,
        utime: parsed.utime,
        stime: parsed.stime,
        threads: parsed.threads,
        starttime_ticks: parsed.starttime_ticks,
        rss_pages,
        pss_kb: rollup.pss_kb,
        swap_pss_kb: rollup.swap_pss_kb,
        rollup_rss_pages: rollup.rss_pages,
        rollup_periods: rollup.periods,
        walks,
        read_bytes,
        write_bytes,
        gpu,
        drm_fds,
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Rollup {
    pss_kb: Option<u64>,
    swap_pss_kb: Option<u64>,
    rss_pages: Option<u64>,
    periods: u32,
}

/// `now` is this sample's `(starttime_ticks, rss_pages)`.
fn rollup_for(
    want_pss: bool,
    want_swap: bool,
    kthread: bool,
    prev: Option<&Process>,
    now: (Option<u64>, Option<u64>),
    read: impl FnOnce() -> (Option<u64>, Option<u64>),
) -> Rollup {
    let carry = |p: &Process, periods| Rollup {
        pss_kb: p.pss_kb,
        swap_pss_kb: p.swap_pss_kb,
        rss_pages: p.rollup_rss_pages,
        periods,
    };
    match prev {
        _ if kthread => Rollup::default(),
        // A pid the kernel handed to a new process is blank until its first
        // read, never the figures of the process that held the pid before.
        Some(p) if !want_pss && p.starttime_ticks.is_some() && p.starttime_ticks == now.0 => {
            carry(p, p.rollup_periods)
        }
        _ if !want_pss => Rollup::default(),
        Some(p) if rollup_holds(p, now.0, now.1, want_swap, cpu::page_size()) => {
            carry(p, p.rollup_periods + 1)
        }
        _ => {
            let (pss_kb, swap_pss_kb) = read();
            Rollup {
                pss_kb,
                swap_pss_kb,
                rss_pages: now.1,
                periods: 0,
            }
        }
    }
}

/// PSS periods a carried rollup may age before it is read regardless, per
/// `ROLLUP_SCALE_BYTES` of RSS at that read, and at most `ROLLUP_AGE_CAP`.
const ROLLUP_MAX_PERIODS: u32 = 6;
const ROLLUP_SCALE_BYTES: u64 = 512 << 20;
const ROLLUP_AGE_CAP: u32 = 60;

/// A `smaps_rollup` read walks the page tables, so it costs in proportion to
/// RSS: on one desktop an 8.4 GiB and a 5.2 GiB process were 54% of a full
/// pass. A large process therefore ages longer between reads.
fn rollup_age_bound(rss_bytes: u64) -> u32 {
    let units = u32::try_from(rss_bytes / ROLLUP_SCALE_BYTES).unwrap_or(u32::MAX);
    ROLLUP_MAX_PERIODS
        .saturating_mul(units.max(1))
        .min(ROLLUP_AGE_CAP)
}

/// Whether a PSS tick may keep `prev`'s PSS and `SwapPss` instead of reading
/// `smaps_rollup`: the same process (`starttime` unchanged), RSS within 1% or
/// 1 MiB of what it was at the last real read, whichever is larger, and that
/// read fewer than `rollup_age_bound` PSS periods ago. Replayed over 10
/// minutes of a desktop (289 processes, every rollup read every 5 s), a flat
/// six periods read 112 ms of rollup CPU per PSS tick; the RSS-scaled bound
/// read 77 ms. Summed PSS was off by 0.023% mean and 0.079% at worst against
/// 0.015% and 0.070% flat, and a carried row by 1.03% at p99 against 1.02%.
/// A 256 MiB scale read 67 ms but summed PSS reached 0.122%.
///
/// `want_swap` must also match what the last read saw, so a `swapon` or
/// `swapoff` forces one read rather than carrying a blank or a figure from a
/// host that no longer has swap. A rollup that could not be read (PSS blank)
/// carries regardless, or an unreadable pid reopens it every PSS tick.
fn rollup_holds(
    prev: &Process,
    starttime_ticks: Option<u64>,
    rss_pages: Option<u64>,
    want_swap: bool,
    page_size: u64,
) -> bool {
    let (Some(then), Some(now)) = (prev.rollup_rss_pages, rss_pages) else {
        return false;
    };
    let floor = (1 << 20) / page_size.max(1);
    prev.starttime_ticks.is_some()
        && prev.starttime_ticks == starttime_ticks
        && prev.rollup_periods + 1 < rollup_age_bound(then.saturating_mul(page_size))
        && (prev.pss_kb.is_none() || prev.swap_pss_kb.is_some() == want_swap)
        && now.abs_diff(then) <= (then / 100).max(floor)
}

/// Every `IDENTITY_EVERY`th walk of a process rereads its uid, argv and
/// cgroup; the rest carry them.
const IDENTITY_EVERY: u32 = 5;

/// Whether `prev`'s uid, argv and cgroup still stand without rereading
/// `status`, `cmdline` and `cgroup`. `walks` restarts at 0 for a pid first
/// seen or reused (its `starttime` changed), and a process reads in full for
/// its first `IDENTITY_EVERY` walks, because that is when a launcher moves it
/// into its own scope and a server retitles its argv. An `exec` changes `exe`
/// or `comm`, both read every tick, so it rereads at once. What no fresh
/// field betrays later -- `setuid`, a cgroup move, an argv rewrite -- shows
/// within `IDENTITY_EVERY` walks; adding `pid` staggers those rereads so no
/// tick takes them all.
///
/// Idle `--json --follow` on 4 CPUs and ~700 pids, three interleaved 60 s
/// runs: 1.71-1.78 s of CPU to 1.61-1.64.
fn carries_identity(prev: &Process, walks: u32, comm: &str, exe: Option<&str>) -> bool {
    walks >= IDENTITY_EVERY
        && !walks.wrapping_add(prev.pid).is_multiple_of(IDENTITY_EVERY)
        && prev.comm == comm
        && prev.exe.as_deref() == exe
}

/// The fastest catch-all sample heft will take. Below this a tick cannot
/// finish its `/proc` walk before the next one is due.
pub const MIN_INTERVAL: f64 = 0.05;

fn pss_due(last: Option<Instant>, now: Instant, interval: Duration) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= interval)
}

struct StatFields {
    comm: String,
    ppid: u32,
    pgrp: i32,
    utime: u64,
    stime: u64,
    threads: Option<u64>,
    starttime_ticks: Option<u64>,
    kthread: bool,
}

/// Bytes, because the name between the parentheses is whatever the process
/// set and need not be UTF-8; only the name is converted, lossily.
fn parse_stat(stat: &[u8]) -> Option<StatFields> {
    let open = stat.iter().position(|&b| b == b'(')?;
    let close = stat.iter().rposition(|&b| b == b')')?;
    if close <= open {
        return None;
    }
    let comm = String::from_utf8_lossy(&stat[open + 1..close]).into_owned();
    let mut fields = stat[close + 1..]
        .split(u8::is_ascii_whitespace)
        .filter(|f| !f.is_empty())
        .map(atoi);
    // after comm: state ppid pgrp ... flags ... utime stime ...
    // num_threads ... starttime (0-based: 0,1,2,6,11,12,17,19), taken in
    // order, so each `nth` skips the fields between.
    let ppid = u32::try_from(fields.nth(1)??).ok()?;
    let pgrp = i32::try_from(fields.next()??).ok()?;
    // PF_KTHREAD in include/linux/sched.h — no userspace smaps/io/fdinfo.
    let flags = fields.nth(3).flatten().unwrap_or(0);
    let utime = fields.nth(4)??;
    let stime = fields.next()??;
    // Optional, unlike the fields above: a truncated tail costs two columns,
    // not the whole process, and a missing one is the blank cell either way.
    let threads = fields.nth(4).flatten();
    let starttime_ticks = fields.nth(1).flatten();
    Some(StatFields {
        comm,
        ppid,
        pgrp,
        utime,
        stime,
        threads,
        starttime_ticks,
        kthread: flags & 0x0020_0000 != 0,
    })
}

/// One `/proc` `Key: value` line as a number. Takes the first whitespace token
/// only: meminfo and `smaps_rollup` append a ` kB` unit that parsing the whole
/// remainder would reject. `None` covers both a missing key and an unparsable
/// value; each caller decides whether that is a blank cell or a default.
pub(crate) fn field_u64(line: impl AsRef<[u8]>, key: &str) -> Option<u64> {
    line.as_ref()
        .strip_prefix(key.as_bytes())?
        .split(u8::is_ascii_whitespace)
        .find(|t| !t.is_empty())
        .and_then(atoi)
}

/// An unsigned decimal token: digits only, `None` on anything else or on
/// overflow. Procfs numbers are ASCII, so there is no text to validate first.
pub(crate) fn atoi(token: &[u8]) -> Option<u64> {
    if token.is_empty() {
        return None;
    }
    token.iter().try_fold(0u64, |n, &b| {
        b.is_ascii_digit().then_some(())?;
        n.checked_mul(10)?.checked_add(u64::from(b - b'0'))
    })
}

/// Bytes, so only the `Uid:` line is converted: the file also carries the
/// process's own name, which need not be UTF-8. Validating all of `status`
/// for one line took UTF-8 checks from 2.0% to 4.7% of heft's user time.
fn parse_uid(status: &[u8]) -> Option<u32> {
    // /proc/<pid> inode uid is euid; grouping uses ruid (Uid: field 1). They
    // diverge on setuid (e.g. fusermount3).
    let line = status
        .split(|&b| b == b'\n')
        .find(|l| l.starts_with(b"Uid:"))?;
    let uid = field_u64(line, "Uid:")?;
    u32::try_from(uid).ok()
}

fn read_exe(dir: &OwnedFd) -> Option<String> {
    let mut link = [0; 4096];
    let target = read_link_at(dir, c"exe", &mut link)?;
    Some(strip_deleted(&String::from_utf8_lossy(target)).to_string())
}

fn strip_deleted(s: &str) -> &str {
    s.strip_suffix(" (deleted)").unwrap_or(s)
}

fn read_cmdline(dir: &OwnedFd, buf: &mut Vec<u8>) -> Vec<String> {
    read_at(dir, c"cmdline", buf, usize::MAX).map_or_else(Vec::new, |bytes| {
        bytes
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    })
}

fn parse_rss_pages(statm: &str) -> Option<u64> {
    statm.split_whitespace().nth(1)?.parse().ok()
}
/// A uid straight through, otherwise the `/etc/passwd` name. Numeric first
/// because a uid is always meaningful and a passwd entry is not always there:
/// a container's uid appears in `/proc` with nothing in `/etc/passwd` to name
/// it, and `--user 1000` has to reach that branch anyway.
#[must_use]
pub fn uid_for(who: &str) -> Option<u32> {
    who.parse().ok().or_else(|| Passwd::read().uid(who))
}

/// `/etc/passwd`, read once for every lookup one tree or one command makes.
/// Not cached for the run, so a user created while heft runs still gets a name.
pub(crate) struct Passwd(String);

impl Passwd {
    pub(crate) fn read() -> Self {
        Self(fs::read_to_string("/etc/passwd").unwrap_or_default())
    }

    /// `(name, uid)` per line. A line with an empty name is no entry, so
    /// neither lookup can match it.
    fn entries(&self) -> impl Iterator<Item = (&str, u32)> {
        self.0.lines().filter_map(|line| {
            let mut it = line.split(':');
            let name = it.next().filter(|n| !n.is_empty())?;
            Some((name, it.nth(1)?.parse().ok()?))
        })
    }

    fn uid(&self, name: &str) -> Option<u32> {
        self.entries()
            .find_map(|(n, uid)| (n == name).then_some(uid))
    }

    /// The login name, else the uid as text.
    pub(crate) fn name(&self, uid: u32) -> String {
        self.entries()
            .find(|&(_, u)| u == uid)
            .map_or_else(|| uid.to_string(), |(n, _)| n.to_string())
    }
}

/// The kernel's own thread count: field 4 of `/proc/loadavg` is
/// `running/total`, and that total is a global counter rather than a walk of
/// `/proc`. So it still answers on a `hidepid` mount, inside a PID namespace,
/// or wherever a pid directory is not readable -- which is exactly where
/// heft's own walk goes blind, and why it is worth comparing the two.
pub(crate) fn kernel_threads() -> Option<u64> {
    let text = fs::read_to_string(crate::root::path("/proc/loadavg")).ok()?;
    text.split_whitespace()
        .nth(3)?
        .split_once('/')?
        .1
        .parse()
        .ok()
}

/// What a process *is*, as opposed to what the columns say it currently costs.
/// Read when the detail overlay opens rather than carried on every `ProcNode`:
/// five more strings per process per tick would be paid on every tick to serve
/// one row of one keystroke. A field heft cannot read (EACCES on another
/// user's `exe`, or a pid that exited between the keypress and the read) comes
/// back empty, the same blank contract the columns keep.
pub(crate) fn detail(pid: u32) -> Vec<(&'static str, String)> {
    let dir = open_pid(pid);
    let mut buf = Vec::new();
    let mut text = |name: &CStr| {
        dir.as_ref()
            .and_then(|d| read_str(d, name, &mut buf))
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let status = text(c"status");
    let cgroup = text(c"cgroup");
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .unwrap_or("")
            .trim()
            .to_string()
    };
    // `Uid:` is real/effective/saved/fs; the real uid is the one the tree bills.
    let uid = parse_uid(status.as_bytes());
    let argv = dir
        .as_ref()
        .map(|d| read_cmdline(d, &mut buf).join(" "))
        .unwrap_or_default();
    vec![
        ("PID", pid.to_string()),
        ("PPID", field("PPid:")),
        ("STATE", field("State:")),
        (
            "UID",
            uid.map_or_else(String::new, |u| format!("{u} ({})", Passwd::read().name(u))),
        ),
        ("EXE", dir.as_ref().and_then(read_exe).unwrap_or_default()),
        ("CGROUP", cgroup),
        ("CMDLINE", crate::once::trunc(&argv, 240)),
    ]
}

/// Header totals from world-readable files only — no per-PID `/proc` walk.
pub(crate) fn placeholder_tree() -> HostTree {
    let cpu = cpu::HostCpu::default();
    cpu::header_from(&cpu::host_consts(), &cpu, &cpu)
}

struct Sampler {
    pool: WalkPool,
    prev: Arc<PidMap<Process>>,
    cpu0: cpu::HostCpu,
    t0: Instant,
    last_pss: Option<Instant>,
    pss_interval: Duration,
    consts: HostHeader,
    inspect_cache: InspectCache,
    rules: &'static Rules,
    psi: psi::Sampler,
    net: net::Sampler,
}

impl Sampler {
    fn prime(pss_interval: Duration, walkers: usize) -> Self {
        let rules = crate::rules::Rules::load();
        let mut inspect_cache = InspectCache::default();
        let mut net = net::Sampler::default();
        let pool = WalkPool::new(walkers);
        let prev = Arc::new(pool.collect(false, false, None));
        // Netns counters are levels, so the first published tick needs a
        // baseline here or `--once` and `--json` would always print a blank
        // rate. The inspect cache makes the tick's own load a no-op.
        net.tick(&ContainerIndex::load(&mut inspect_cache, rules), &prev, 1.0);
        // Pressure totals are levels too, for the same reason: without a
        // baseline here the first published tick has nothing to subtract and
        // every stall column would be blank.
        let mut psi = psi::Sampler::default();
        psi.tick(&prev, 1.0);
        let t0 = Instant::now();
        Self {
            consts: cpu::host_consts(),
            cpu0: cpu::read_host(),
            pool,
            prev,
            t0,
            last_pss: None,
            pss_interval,
            inspect_cache,
            rules,
            net,
            psi,
        }
    }

    fn tick(&mut self, force_pss: bool) -> HostTree {
        let containers = ContainerIndex::load(&mut self.inspect_cache, self.rules);
        let t1 = Instant::now();
        let cpu1 = cpu::read_host();
        let want_pss = force_pss || pss_due(self.last_pss, t1, self.pss_interval);
        // Built before the walk so the walk knows whether this machine has swap
        // at all; it reads only world-readable host files, so the order is free.
        let mut header = cpu::header_from(&self.consts, &self.cpu0, &cpu1);
        psi::host_avg10(&mut header);
        let curr = self
            .pool
            .collect(want_pss, header.swap_total_bytes > 0, Some(&self.prev));
        if want_pss {
            self.last_pss = Some(t1);
        }
        let elapsed = t1.duration_since(self.t0);
        let secs = elapsed.as_secs_f64().max(1e-6);
        let net = self.net.tick(&containers, &curr, secs);
        let stalls = self.psi.tick(&curr, secs);
        let mut tree = group::build_tree(
            &self.prev,
            &curr,
            elapsed,
            &self.consts,
            header,
            &containers,
            self.rules,
        );
        net.apply(&mut tree);
        // Applied after the tree exists, because a row's cgroup is only
        // knowable from the processes the grouping put under it.
        stalls.apply(&mut tree, &curr);
        self.prev = Arc::new(curr);
        self.cpu0 = cpu1;
        self.t0 = t1;
        tree
    }
}
/// Primes once, then hands `emit` a tree every `interval` until the reader
/// goes away or `emit` fails.
///
/// Unlike `sample_world`, this honours `--pss-interval`: a stream is a
/// continuous mode, and reading `smaps_rollup` for every process once a second
/// forever is the exact cost `--pss-interval` exists to avoid. The first
/// published sample forces PSS anyway, so the stream never opens with a blank
/// memory column.
///
/// # Errors
///
/// Returns whatever `emit` returns. A closed reader surfaces as `EPIPE`, which
/// `main` recognises and exits quietly on.
pub(crate) fn sample_stream(
    interval: Duration,
    pss_interval: Duration,
    mut emit: impl FnMut(&HostTree) -> Result<(), crate::types::Error>,
) -> Result<(), crate::types::Error> {
    let mut sampler = Sampler::prime(pss_interval, FOLLOW_WALKERS);
    let mut first = true;
    loop {
        thread::sleep(interval);
        let tree = sampler.tick(first);
        first = false;
        emit(&tree)?;
    }
}

pub(crate) fn sample_world(interval: Duration) -> HostTree {
    let mut sampler = Sampler::prime(interval, usize::MAX);
    thread::sleep(interval);
    sampler.tick(true)
}

/// `--fixture`: the fields grouping reads, in the shape `tests/grouping.rs`
/// loads, so a wrong row reported from a desktop heft has never run on becomes
/// a regression test verbatim. A screenshot or `--json` carries none of exe,
/// cgroup or ppid. One walk and no metrics, since grouping uses none. Kernel
/// threads are left out because `PF_KTHREAD` alone places them, and `$HOME/`
/// becomes `~/` so every path does not carry the login name.
///
/// # Errors
///
/// Returns an error if stdout cannot be written.
pub fn print_fixture() -> Result<(), crate::types::Error> {
    use std::io::Write;
    let consts = cpu::host_consts();
    let procs = WalkPool::new(usize::MAX).collect(false, false, None);
    let home = std::env::var("HOME")
        .ok()
        .filter(|h| h.len() > 1)
        .map(|h| format!("{}/", h.trim_end_matches('/')));
    let tilde = |s: &str| {
        home.as_deref()
            .map_or_else(|| s.to_string(), |h| s.replace(h, "~/"))
    };
    let mut rows: Vec<&Process> = procs
        .values()
        .filter(|p| !crate::identity::is_kernel(p))
        .collect();
    rows.sort_by_key(|p| p.pid);
    let processes: Vec<serde_json::Value> = rows
        .iter()
        .map(|p| {
            serde_json::json!({
                "pid": p.pid,
                "ppid": p.ppid,
                "pgrp": p.pgrp,
                "uid": p.uid,
                "comm": p.comm,
                "exe": p.exe.as_deref().map(tilde),
                "cmdline": p.cmdline.iter().map(|a| tilde(a)).collect::<Vec<_>>(),
                "cgroup": p.cgroup,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "nproc": consts.nproc,
        "clk_tck": consts.clk_tck,
        "page_size": consts.page_size,
        "processes": processes,
    });
    writeln!(io::stdout(), "{}", serde_json::to_string_pretty(&doc)?)?;
    eprintln!(
        "heft: command lines are included and can hold tokens or paths you would not publish; read this before attaching it."
    );
    Ok(())
}

/// Latest complete tree. The UI takes; the sampler only publishes.
///
/// # Errors
///
/// Returns an error if the sampler thread cannot be spawned.
pub(crate) fn spawn_sampler(
    interval: Duration,
    pss_interval: Duration,
    slot: Arc<Mutex<Option<HostTree>>>,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("heft-sample".into())
        .spawn(move || {
            let mut sampler = Sampler::prime(pss_interval, FOLLOW_WALKERS);
            loop {
                let start = Instant::now();
                let tree = sampler.tick(false);
                *slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tree);
                thread::sleep(interval.saturating_sub(start.elapsed()));
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_rereads_while_young_on_an_exec_and_every_fifth_walk() {
        let prev = Process {
            pid: 10,
            comm: "bash".into(),
            exe: Some("/usr/bin/bash".into()),
            ..Default::default()
        };
        let bash = |walks| carries_identity(&prev, walks, "bash", Some("/usr/bin/bash"));
        assert!((0..IDENTITY_EVERY).all(|w| !bash(w)));
        let carried = (5..15).filter(|&w| bash(w)).count();
        assert_eq!(carried, 8);
        assert!(!carries_identity(&prev, 6, "bash", Some("/usr/bin/sleep")));
        assert!(!carries_identity(&prev, 6, "sleep", Some("/usr/bin/bash")));
    }

    #[test]
    fn stat_with_spaces_in_comm() {
        let mut tail = String::from("10 (my app) S 1 10 10 0 0 0 0 0 0 0 30 40");
        for _ in 0..20 {
            tail.push_str(" 0");
        }
        let p = parse_stat(tail.as_bytes()).unwrap();
        assert_eq!(p.comm, "my app");
        assert_eq!(p.ppid, 1);
        assert_eq!(p.pgrp, 10);
        assert_eq!(p.utime, 30);
        assert_eq!(p.stime, 40);
        assert!(!p.kthread);
    }

    /// `num_threads` and `starttime` are fields 20 and 22, five and seven
    /// tokens past `stime`; an off-by-one here silently prints another
    /// process's counter as a thread count.
    #[test]
    fn threads_and_starttime_come_from_their_own_fields() {
        let stat =
            "10 (bash) S 1 10 10 0 -1 4194304 91 0 0 0 30 40 0 0 25 5 17 0 221093059 236335104 474";
        let p = parse_stat(stat.as_bytes()).unwrap();
        assert_eq!(p.utime, 30);
        assert_eq!(p.threads, Some(17));
        assert_eq!(p.starttime_ticks, Some(221_093_059));
        // A truncated tail costs those two columns, not the process.
        let short = parse_stat(b"10 (bash) S 1 10 10 0 -1 0 0 0 0 0 30 40").unwrap();
        assert_eq!(short.utime, 30);
        assert_eq!(short.threads, None);
        assert_eq!(short.starttime_ticks, None);
    }

    #[test]
    fn kthread_flag_from_stat() {
        let mut tail = String::from("10 (kworker/0:0) S 2 0 0 0 0 2097152 0 0 0 0 1 2");
        for _ in 0..20 {
            tail.push_str(" 0");
        }
        let p = parse_stat(tail.as_bytes()).unwrap();
        assert!(p.kthread);
        assert_eq!(p.ppid, 2);
    }

    /// Any process can give itself a name that is not UTF-8 with one
    /// `PR_SET_NAME`, and must not vanish from the walk for it. A thread is
    /// enough: `/proc/<tid>` resolves by lookup though readdir never lists it.
    #[test]
    fn a_name_that_is_not_utf8_keeps_the_process() {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let named = thread::spawn(move || {
            // SAFETY: PR_SET_NAME copies at most 16 bytes from a NUL-terminated
            // buffer that outlives the call, and renames only this thread.
            unsafe { libc::prctl(libc::PR_SET_NAME, c"\xff\xfehidden".as_ptr()) };
            // SAFETY: gettid has no side effects.
            tx.send(unsafe { libc::gettid() }).unwrap();
            let _ = done_rx.recv();
        });
        let tid = u32::try_from(rx.recv().unwrap()).unwrap();
        let p = read_pid(tid, false, false, None, &mut Vec::new());
        drop(done_tx);
        named.join().unwrap();
        let p = p.expect("the process is kept");
        assert_eq!(p.comm, "\u{fffd}\u{fffd}hidden");
        assert_eq!(p.uid, cpu::euid());
        assert!(!p.cgroup.is_empty());
    }

    /// A pool whose threads could not start (`RLIMIT_NPROC`, a cgroup's
    /// `pids.max`) walks on the calling thread and sees the same machine.
    #[test]
    fn a_pool_with_no_worker_walks_on_the_calling_thread() {
        let before = WalkPool::new(2).collect(false, false, None);
        let inline = WalkPool {
            workers: Vec::new(),
        }
        .collect(false, false, None);
        let after = WalkPool::new(2).collect(false, false, None);
        assert!(inline.contains_key(&std::process::id()));
        for pid in before.keys().filter(|p| after.contains_key(p)) {
            assert!(inline.contains_key(pid), "pid {pid} ran throughout");
        }
    }

    #[test]
    fn pss_due_first_then_interval() {
        let start = Instant::now();
        let five = Duration::from_secs(5);
        assert!(pss_due(None, start, five));
        assert!(!pss_due(Some(start), start, five));
        assert!(!pss_due(Some(start), start + Duration::from_secs(4), five));
        assert!(pss_due(Some(start), start + five, five));
    }

    #[test]
    fn passwd_never_matches_an_empty_name() {
        let pw = Passwd(String::from(
            "::7:7::/:/bin/false\nroot:x:0:0:root:/root:/bin/sh\n",
        ));
        assert_eq!(pw.uid(""), None);
        assert_eq!(pw.name(7), "7");
        assert_eq!(pw.uid("root"), Some(0));
        assert_eq!(pw.name(0), "root");
    }

    #[test]
    fn atoi_takes_digits_only_and_blanks_on_overflow() {
        assert_eq!(atoi(b"0"), Some(0));
        assert_eq!(atoi(b"18446744073709551615"), Some(u64::MAX));
        assert_eq!(atoi(b"18446744073709551616"), None);
        assert_eq!(atoi(b""), None);
        assert_eq!(atoi(b"-1"), None);
        assert_eq!(atoi(b"12kB"), None);
    }

    #[test]
    fn field_u64_takes_first_token_and_blanks_on_failure() {
        assert_eq!(field_u64("MemTotal:  1000 kB", "MemTotal:"), Some(1000));
        assert_eq!(field_u64("read_bytes: 10", "read_bytes:"), Some(10));
        assert_eq!(
            field_u64("Uid:\t1000\t1000\t1000\t1000", "Uid:"),
            Some(1000)
        );
        // An unreadable metric must stay blank, never fall back to 0.
        assert_eq!(field_u64("Pss: kB", "Pss:"), None);
        assert_eq!(field_u64("Pss_Anon: 4 kB", "Pss:"), None);
        assert_eq!(field_u64("Rss: 9 kB", "Pss:"), None);
    }

    #[test]
    fn carried_pss_skips_kthread_and_new_pids() {
        let carried = Process {
            pss_kb: Some(12),
            swap_pss_kb: Some(3),
            starttime_ticks: Some(7),
            ..Process::default()
        };
        let unread = || unreachable!("a carried tick reads no rollup");
        let now = (Some(7), None);
        assert_eq!(
            rollup_for(false, false, true, Some(&carried), now, unread),
            Rollup::default()
        );
        let kept = rollup_for(false, false, false, Some(&carried), now, unread);
        assert_eq!((kept.pss_kb, kept.swap_pss_kb), (Some(12), Some(3)));
        assert_eq!(
            rollup_for(false, false, false, Some(&carried), (Some(8), None), unread),
            Rollup::default(),
            "a reused pid"
        );
        assert_eq!(
            rollup_for(false, false, false, None, now, unread),
            Rollup::default()
        );
    }

    #[test]
    fn rollup_holds_within_one_percent_or_a_mebibyte_for_six_periods() {
        let prev = Process {
            starttime_ticks: Some(7),
            rollup_rss_pages: Some(100_000),
            ..Process::default()
        };
        let holds = |p: &Process, start, rss| rollup_holds(p, start, Some(rss), false, 4096);
        assert!(holds(&prev, Some(7), 101_000));
        assert!(holds(&prev, Some(7), 99_000));
        assert!(!holds(&prev, Some(7), 101_001));
        assert!(!holds(&prev, Some(8), 100_000), "a new process");
        let swapped = Process {
            pss_kb: Some(12),
            swap_pss_kb: Some(3),
            ..prev.clone()
        };
        assert!(rollup_holds(&swapped, Some(7), Some(100_000), true, 4096));
        assert!(
            !rollup_holds(&swapped, Some(7), Some(100_000), false, 4096),
            "swapoff"
        );
        let read = Process {
            pss_kb: Some(12),
            ..prev.clone()
        };
        assert!(
            !rollup_holds(&read, Some(7), Some(100_000), true, 4096),
            "swapon"
        );
        assert!(
            rollup_holds(&prev, Some(7), Some(100_000), true, 4096),
            "an unreadable rollup carries whatever swap is"
        );
        let small = Process {
            rollup_rss_pages: Some(1_000),
            ..prev.clone()
        };
        assert!(holds(&small, Some(7), 1_256), "1 MiB is 256 pages");
        assert!(!holds(&small, Some(7), 1_257));
        let fifth = Process {
            rollup_periods: 4,
            ..prev.clone()
        };
        assert!(holds(&fifth, Some(7), 100_000));
        let sixth = Process {
            rollup_periods: 5,
            ..prev.clone()
        };
        assert!(
            !holds(&sixth, Some(7), 100_000),
            "six periods since the read"
        );
        // 4 GiB is eight scale units, so 48 periods; 64 GiB hits the cap.
        let big = |pages, periods| Process {
            rollup_rss_pages: Some(pages),
            rollup_periods: periods,
            ..prev.clone()
        };
        assert!(holds(&big(1 << 20, 46), Some(7), 1 << 20));
        assert!(!holds(&big(1 << 20, 47), Some(7), 1 << 20));
        assert!(holds(&big(1 << 24, 58), Some(7), 1 << 24));
        assert!(!holds(&big(1 << 24, 59), Some(7), 1 << 24));
        let reused = rollup_for(
            true,
            false,
            false,
            Some(&fifth),
            (Some(7), Some(100_000)),
            || unreachable!("RSS held"),
        );
        assert_eq!(reused.periods, 5);
        let read = rollup_for(
            true,
            false,
            false,
            Some(&sixth),
            (Some(7), Some(100_500)),
            || (Some(1), None),
        );
        assert_eq!(
            read,
            Rollup {
                pss_kb: Some(1),
                swap_pss_kb: None,
                rss_pages: Some(100_500),
                periods: 0
            }
        );
    }
}
