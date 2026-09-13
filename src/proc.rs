use std::collections::HashMap;
use std::ffi::CStr;
use std::fs;
use std::io;
use std::num::NonZero;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rustix::buffer::spare_capacity;
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use crate::containers::{ContainerIndex, InspectCache};
use crate::cpu;
use crate::group;
use crate::rules::Rules;
use crate::types::{GpuCounters, HostHeader, HostTree, Process};
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
    prev: Option<Arc<HashMap<u32, Process>>>,
}

impl WalkPool {
    /// At most `max` workers, and never more than the CPUs this process may run on.
    fn new(max: usize) -> Self {
        let n = thread::available_parallelism()
            .map_or(1, NonZero::get)
            .min(max);
        let workers = (0..n)
            .map(|i| {
                let (job_tx, job_rx) = mpsc::channel();
                let (result_tx, result_rx) = mpsc::channel();
                thread::Builder::new()
                    .name(format!("heft-walk-{i}"))
                    .spawn(move || walk_worker(&job_rx, &result_tx))
                    .expect("walk worker");
                (job_tx, result_rx)
            })
            .collect();
        Self { workers }
    }

    fn collect(
        &self,
        want_pss: bool,
        want_swap: bool,
        prev: Option<&Arc<HashMap<u32, Process>>>,
    ) -> HashMap<u32, Process> {
        let Ok(dir) = fs::read_dir(crate::root::path("/proc")) else {
            return HashMap::new();
        };
        let pids: Vec<u32> = dir
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
            .collect();
        if pids.is_empty() {
            return HashMap::new();
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
        let mut out = HashMap::with_capacity(pids.len());
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
        let mut out = Vec::new();
        for &pid in &job.pids {
            let prev = job.prev.as_ref().and_then(|m| m.get(&pid));
            if let Some(p) = read_pid(pid, job.want_pss, job.want_swap, prev, &mut buf) {
                out.push((pid, p));
            }
        }
        if result_tx.send(out).is_err() {
            break;
        }
    }
}

/// `/proc/<pid>` opened once, so every file under it is an `openat` of one
/// name rather than a path the kernel resolves from `/` again. `O_PATH`
/// because the handle is only ever a base for those lookups.
fn open_pid(pid: u32) -> Option<OwnedFd> {
    let path = format!("{}/proc/{pid}", crate::root::prefix());
    let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
    rustix::fs::open(path.as_str(), flags, Mode::empty()).ok()
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
        match rustix::io::read(&fd, spare_capacity(buf)) {
            Ok(0) => break,
            Ok(_) | Err(Errno::INTR) => {}
            Err(_) => return None,
        }
    }
    Some(buf)
}

/// `read_at` uncapped, as text. Invalid UTF-8 is `None`, as `fs::read_to_string` has it.
pub(crate) fn read_str(
    dir: impl AsFd,
    name: impl rustix::path::Arg,
    buf: &mut Vec<u8>,
) -> Option<&str> {
    std::str::from_utf8(read_at(dir, name, buf, usize::MAX)?).ok()
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
    let parsed = parse_stat(read_str(&dir, c"stat", buf)?)?;
    let uid = read_str(&dir, c"status", buf)
        .and_then(parse_uid)
        .unwrap_or(0);
    let exe = read_exe(&dir);
    let cmdline = read_cmdline(&dir, buf);
    let cgroup = read_str(&dir, c"cgroup", buf)
        .unwrap_or_default()
        .trim()
        .to_string();
    let rss_pages = read_str(&dir, c"statm", buf).and_then(parse_rss_pages);
    // PSS is a level, not a rate. Kernel threads have no rollup. Prime and
    // TUI ticks between `--pss-interval` reuse last (new PIDs stay blank).
    let rollup = rollup_for(
        want_pss,
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
        Some(p) if rollup_holds(p, now.0, now.1, cpu::page_size()) => {
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

/// PSS periods a carried rollup may age before it is read regardless.
const ROLLUP_MAX_PERIODS: u32 = 6;

/// Whether a PSS tick may keep `prev`'s PSS and `SwapPss` instead of reading
/// `smaps_rollup`: the same process (`starttime` unchanged), RSS within 1% or
/// 1 MiB of what it was at the last real read, whichever is larger, and that
/// read fewer than `ROLLUP_MAX_PERIODS` PSS periods ago. Replayed over a
/// recorded `--follow` session this kept 24% of the rollup CPU; summed PSS
/// was off by 0.073%, and a carried row by 2.2% at p99 and 4.3% at worst. With
/// no age bound and a 0.1% threshold the worst row was 5.8%.
fn rollup_holds(
    prev: &Process,
    starttime_ticks: Option<u64>,
    rss_pages: Option<u64>,
    page_size: u64,
) -> bool {
    let (Some(then), Some(now)) = (prev.rollup_rss_pages, rss_pages) else {
        return false;
    };
    let floor = (1 << 20) / page_size.max(1);
    prev.starttime_ticks.is_some()
        && prev.starttime_ticks == starttime_ticks
        && prev.rollup_periods + 1 < ROLLUP_MAX_PERIODS
        && now.abs_diff(then) <= (then / 100).max(floor)
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

fn parse_stat(stat: &str) -> Option<StatFields> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    if close <= open {
        return None;
    }
    let comm = stat[open + 1..close].to_string();
    let rest = stat[close + 1..].split_whitespace();
    let fields: Vec<&str> = rest.collect();
    // after comm: state ppid pgrp ... flags ... utime stime ...
    // num_threads ... starttime (0-based: 0,1,2,6,11,12,17,19)
    let ppid = fields.get(1)?.parse().ok()?;
    let pgrp = fields.get(2)?.parse().ok()?;
    // PF_KTHREAD in include/linux/sched.h — no userspace smaps/io/fdinfo.
    let flags: u32 = fields.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);
    let utime = fields.get(11)?.parse().ok()?;
    let stime = fields.get(12)?.parse().ok()?;
    // Optional, unlike the fields above: a truncated tail costs two columns,
    // not the whole process, and a missing one is the blank cell either way.
    Some(StatFields {
        comm,
        ppid,
        pgrp,
        utime,
        stime,
        threads: fields.get(17).and_then(|s| s.parse().ok()),
        starttime_ticks: fields.get(19).and_then(|s| s.parse().ok()),
        kthread: flags & 0x0020_0000 != 0,
    })
}

/// One `/proc` `Key: value` line as a number. Takes the first whitespace token
/// only: meminfo and `smaps_rollup` append a ` kB` unit that parsing the whole
/// remainder would reject. `None` covers both a missing key and an unparsable
/// value; each caller decides whether that is a blank cell or a default.
pub(crate) fn field_u64(line: &str, key: &str) -> Option<u64> {
    line.strip_prefix(key)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn parse_uid(status: &str) -> Option<u32> {
    // /proc/<pid> inode uid is euid; grouping uses ruid (Uid: field 1). They
    // diverge on setuid (e.g. fusermount3).
    let uid = status.lines().find_map(|l| field_u64(l, "Uid:"))?;
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
    let uid = parse_uid(&status);
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
    prev: Arc<HashMap<u32, Process>>,
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
    let mut rows: Vec<&Process> = procs.values().filter(|p| !p.kthread).collect();
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
    fn stat_with_spaces_in_comm() {
        let mut tail = String::from("10 (my app) S 1 10 10 0 0 0 0 0 0 0 30 40");
        for _ in 0..20 {
            tail.push_str(" 0");
        }
        let p = parse_stat(&tail).unwrap();
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
        let p = parse_stat(stat).unwrap();
        assert_eq!(p.utime, 30);
        assert_eq!(p.threads, Some(17));
        assert_eq!(p.starttime_ticks, Some(221_093_059));
        // A truncated tail costs those two columns, not the process.
        let short = parse_stat("10 (bash) S 1 10 10 0 -1 0 0 0 0 0 30 40").unwrap();
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
        let p = parse_stat(&tail).unwrap();
        assert!(p.kthread);
        assert_eq!(p.ppid, 2);
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
            rollup_for(false, true, Some(&carried), now, unread),
            Rollup::default()
        );
        let kept = rollup_for(false, false, Some(&carried), now, unread);
        assert_eq!((kept.pss_kb, kept.swap_pss_kb), (Some(12), Some(3)));
        assert_eq!(
            rollup_for(false, false, Some(&carried), (Some(8), None), unread),
            Rollup::default(),
            "a reused pid"
        );
        assert_eq!(
            rollup_for(false, false, None, now, unread),
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
        let holds = |p: &Process, start, rss| rollup_holds(p, start, Some(rss), 4096);
        assert!(holds(&prev, Some(7), 101_000));
        assert!(holds(&prev, Some(7), 99_000));
        assert!(!holds(&prev, Some(7), 101_001));
        assert!(!holds(&prev, Some(8), 100_000), "a new process");
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
            ..prev
        };
        assert!(
            !holds(&sixth, Some(7), 100_000),
            "six periods since the read"
        );
        let reused = rollup_for(true, false, Some(&fifth), (Some(7), Some(100_000)), || {
            unreachable!("RSS held")
        });
        assert_eq!(reused.periods, 5);
        let read = rollup_for(true, false, Some(&sixth), (Some(7), Some(100_500)), || {
            (Some(1), None)
        });
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
