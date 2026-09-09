use std::collections::HashMap;
use std::fs;
use std::io;
use std::num::NonZero;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::Overrides;
use crate::containers::{ContainerIndex, InspectCache};
use crate::cpu;
use crate::group;
use crate::types::{GpuCounters, HostHeader, HostTree, Process};
use crate::{gpu, io as pio, net, psi};

/// The walk is around seven small procfs reads per pid and no computation
/// worth the name, so nearly all of its wall clock is the kernel building
/// those files one at a time while this process waits. Splitting the pid list
/// across threads overlaps that wait.
///
/// Measured on a 777-pid host, `--once --interval 0.05` (a plain walk, then a
/// PSS walk), ten interleaved runs each: 1.02s serial to 0.41s parallel. The
/// two walks do not gain equally. A plain tick goes 120ms to 20ms, which is
/// what makes the documented 0.05s `--interval` floor reachable at all; the
/// PSS tick only goes 850ms to 340ms, because `smaps_rollup` makes the kernel
/// walk that process's page tables and that is memory-bound rather than
/// latency-bound. So a PSS tick still stretches its interval, exactly as
/// HUMANS.md says it does — this made the ordinary tick cheap, not that one.
///
/// Workers live on the Sampler (so `--once` / `--json` still pool their two
/// walks) and `sample_stream` drop joins them. A `thread::scope` per sample
/// was refused because spawn cost tens of microseconds; the RSS is why a
/// pool exists now. New glibc arenas each tick climbed ~15 MiB every 5s PSS
/// tick to ~488 MiB. `MALLOC_ARENA_MAX=2` plateaued at 39 MiB, so arenas
/// dominate the HostTree.
struct WalkPool {
    job_txs: Vec<mpsc::Sender<WalkJob>>,
    result_rx: mpsc::Receiver<WalkChunk>,
    handles: Vec<JoinHandle<()>>,
}

struct WalkJob {
    pids: Vec<u32>,
    want_pss: bool,
    want_swap: bool,
    prev: Option<Arc<HashMap<u32, Process>>>,
}

enum WalkChunk {
    Done(Vec<(u32, Process)>),
    Panicked,
}

impl WalkPool {
    fn new() -> Self {
        let n = thread::available_parallelism().map_or(1, NonZero::get);
        let (result_tx, result_rx) = mpsc::channel();
        let mut job_txs = Vec::with_capacity(n);
        let mut handles = Vec::with_capacity(n);
        for i in 0..n {
            let (job_tx, job_rx) = mpsc::channel();
            let result_tx = result_tx.clone();
            let handle = thread::Builder::new()
                .name(format!("heft-walk-{i}"))
                .spawn(move || walk_worker(job_rx, result_tx))
                .expect("walk worker");
            job_txs.push(job_tx);
            handles.push(handle);
        }
        Self {
            job_txs,
            result_rx,
            handles,
        }
    }

    fn collect(
        &mut self,
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
        let workers = self.job_txs.len().min(pids.len());
        let chunk = pids.len().div_ceil(workers);
        let n_jobs = pids.chunks(chunk).len();
        for (i, slice) in pids.chunks(chunk).enumerate() {
            self.job_txs[i]
                .send(WalkJob {
                    pids: slice.to_vec(),
                    want_pss,
                    want_swap,
                    prev: prev.cloned(),
                })
                .expect("a /proc walk thread exited");
        }
        let mut out = HashMap::with_capacity(pids.len());
        for _ in 0..n_jobs {
            match self.result_rx.recv().expect("a /proc walk thread exited") {
                WalkChunk::Done(v) => out.extend(v),
                // A panic here is a bug in a `/proc` parser, and swallowing it
                // would publish a tree quietly missing a chunk of the machine.
                WalkChunk::Panicked => panic!("a /proc walk thread panicked"),
            }
        }
        out
    }
}

impl Drop for WalkPool {
    fn drop(&mut self) {
        self.job_txs.clear();
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

fn walk_worker(job_rx: mpsc::Receiver<WalkJob>, result_tx: mpsc::Sender<WalkChunk>) {
    while let Ok(job) = job_rx.recv() {
        let chunk = std::panic::catch_unwind(AssertUnwindSafe(|| {
            job.pids
                .iter()
                .filter_map(|&pid| {
                    let p = read_pid(
                        pid,
                        job.want_pss,
                        job.want_swap,
                        job.prev.as_ref().and_then(|m| m.get(&pid)),
                    )?;
                    Some((pid, p))
                })
                .collect::<Vec<_>>()
        }));
        let msg = match chunk {
            Ok(v) => WalkChunk::Done(v),
            Err(_) => WalkChunk::Panicked,
        };
        if result_tx.send(msg).is_err() {
            break;
        }
    }
}

fn read_pid(pid: u32, want_pss: bool, want_swap: bool, prev: Option<&Process>) -> Option<Process> {
    let base = format!("{}/proc/{pid}", crate::root::prefix());
    let stat = fs::read_to_string(format!("{base}/stat")).ok()?;
    let parsed = parse_stat(&stat)?;
    let uid = read_uid(&format!("{base}/status")).unwrap_or(0);
    let exe = read_exe(&format!("{base}/exe"));
    let cmdline = read_cmdline(&format!("{base}/cmdline"));
    let cgroup = fs::read_to_string(format!("{base}/cgroup"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let rss_pages = read_rss_pages(&format!("{base}/statm"));
    // PSS is a level, not a rate. Kernel threads have no rollup. Prime and
    // TUI ticks between `--pss-interval` reuse last (new PIDs stay blank).
    let (pss_kb, swap_pss_kb) = rollup_for(want_pss, want_swap, parsed.kthread, prev, pid);
    // PF_KTHREAD has no userspace /proc/pid/io or drm fdinfo.
    let (read_bytes, write_bytes, gpu) = if parsed.kthread {
        (None, None, GpuCounters::default())
    } else {
        let (r, w) = pio::read_io(pid);
        // want_pss is the residual GPU fdinfo walk (PSS / --once) when dri/drm
        // names were found but yielded no metrics; empty prefilter skips it.
        (r, w, gpu::read_pid(pid, want_pss))
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
        d_state: parsed.d_state,
        rss_pages,
        pss_kb,
        swap_pss_kb,
        read_bytes,
        write_bytes,
        gpu,
    })
}

fn rollup_for(
    want_pss: bool,
    want_swap: bool,
    kthread: bool,
    prev: Option<&Process>,
    pid: u32,
) -> (Option<u64>, Option<u64>) {
    if kthread {
        (None, None)
    } else if want_pss {
        pio::read_rollup_kb(pid, want_swap)
    } else {
        (
            prev.and_then(|p| p.pss_kb),
            prev.and_then(|p| p.swap_pss_kb),
        )
    }
}

/// The fastest catch-all sample heft will take. Below this a tick cannot
/// finish its `/proc` walk before the next one is due.
pub const MIN_INTERVAL: f64 = 0.05;

/// Floor `MIN_INTERVAL`; PSS cadence is at least the catch-all interval.
///
/// Still clamps rather than failing: `main` refuses a typed value below the
/// floor, so what reaches here is either already valid or came from a caller
/// of the library, which should get a working monitor rather than a panic.
/// `max` also absorbs a NaN, which `Duration::from_secs_f64` would panic on.
#[must_use]
pub fn clamp_intervals(interval_s: f64, pss_s: f64) -> (Duration, Duration) {
    let interval = Duration::from_secs_f64(interval_s.max(MIN_INTERVAL));
    let pss = Duration::from_secs_f64(pss_s.max(MIN_INTERVAL)).max(interval);
    (interval, pss)
}

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
    d_state: bool,
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
        // `D` is uninterruptible sleep. A missing field is not `D`: the state
        // is one character the kernel always writes, so absence means a
        // truncated line, and guessing `D` there would invent a stall.
        d_state: fields.first().is_some_and(|s| *s == "D"),
    })
}

/// One `/proc` `Key: value` line as a number. Takes the first whitespace token
/// only: meminfo and smaps_rollup append a ` kB` unit that parsing the whole
/// remainder would reject. `None` covers both a missing key and an unparsable
/// value; each caller decides whether that is a blank cell or a default.
pub(crate) fn field_u64(line: &str, key: &str) -> Option<u64> {
    line.strip_prefix(key)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn read_uid(status_path: &str) -> Option<u32> {
    // /proc/<pid> inode uid is euid; grouping uses ruid (Uid: field 1). They
    // diverge on setuid (e.g. fusermount3).
    let text = fs::read_to_string(status_path).ok()?;
    let uid = text.lines().find_map(|l| field_u64(l, "Uid:"))?;
    u32::try_from(uid).ok()
}

fn read_exe(path: &str) -> Option<String> {
    match fs::read_link(path) {
        Ok(p) => Some(strip_deleted(&p.to_string_lossy()).to_string()),
        Err(_) => None,
    }
}

fn strip_deleted(s: &str) -> &str {
    s.strip_suffix(" (deleted)").unwrap_or(s)
}

fn read_cmdline(path: &str) -> Vec<String> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn read_rss_pages(path: &str) -> Option<u64> {
    let text = fs::read_to_string(path).ok()?;
    text.split_whitespace().nth(1)?.parse().ok()
}
/// A uid straight through, otherwise the `/etc/passwd` name. Numeric first
/// because a uid is always meaningful and a passwd entry is not always there:
/// a container's uid appears in `/proc` with nothing in `/etc/passwd` to name
/// it, and `--user 1000` has to reach that branch anyway.
pub fn uid_for(who: &str) -> Option<u32> {
    if let Ok(uid) = who.parse::<u32>() {
        return Some(uid);
    }
    let text = fs::read_to_string("/etc/passwd").ok()?;
    text.lines().find_map(|line| {
        let mut it = line.split(':');
        (it.next()? == who).then_some(())?;
        let _ = it.next();
        it.next()?.parse().ok()
    })
}

pub(crate) fn username(uid: u32) -> String {
    if let Ok(text) = fs::read_to_string("/etc/passwd") {
        for line in text.lines() {
            let mut it = line.split(':');
            let name = it.next().unwrap_or("");
            let _ = it.next();
            if it.next().and_then(|s| s.parse::<u32>().ok()) == Some(uid) && !name.is_empty() {
                return name.to_string();
            }
        }
    }
    uid.to_string()
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
    let base = format!("{}/proc/{pid}", crate::root::prefix());
    let status = fs::read_to_string(format!("{base}/status")).unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .unwrap_or("")
            .trim()
            .to_string()
    };
    // `Uid:` is real/effective/saved/fs; the real uid is the one the tree bills.
    let uid = field("Uid:")
        .split_whitespace()
        .next()
        .and_then(|u| u.parse::<u32>().ok());
    let argv = read_cmdline(&format!("{base}/cmdline")).join(" ");
    vec![
        ("PID", pid.to_string()),
        ("PPID", field("PPid:")),
        ("STATE", field("State:")),
        (
            "UID",
            uid.map_or_else(String::new, |u| format!("{u} ({})", username(u))),
        ),
        ("EXE", read_exe(&format!("{base}/exe")).unwrap_or_default()),
        (
            "CGROUP",
            fs::read_to_string(format!("{base}/cgroup"))
                .unwrap_or_default()
                .trim()
                .to_string(),
        ),
        ("CMDLINE", truncate_chars(&argv, 240)),
    ]
}

/// A Chromium helper's argv runs to thousands of characters — one
/// `--enable-features=` list alone fills a pane. The front is what identifies
/// the process (`--type=utility`, `--port`), so the tail is cut rather than
/// letting one field push every other fact off the screen. Chars, not bytes,
/// so a multi-byte argument cannot be split mid-character.
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
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
    overrides: Overrides,
    psi: psi::Sampler,
    net: net::Sampler,
}

impl Sampler {
    fn prime(pss_interval: Duration) -> Self {
        let overrides = crate::config::load_overrides();
        let mut inspect_cache = InspectCache::default();
        let mut net = net::Sampler::default();
        let mut pool = WalkPool::new();
        let prev = Arc::new(pool.collect(false, false, None));
        // Netns counters are levels, so the first published tick needs a
        // baseline here or `--once` and `--json` would always print a blank
        // rate. The inspect cache makes the tick's own load a no-op.
        net.tick(
            &ContainerIndex::load(&mut inspect_cache, &overrides),
            &prev,
            1.0,
        );
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
            overrides,
            net,
            psi,
        }
    }

    fn tick(&mut self, force_pss: bool) -> HostTree {
        let containers = ContainerIndex::load(&mut self.inspect_cache, &self.overrides);
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
            &self.overrides,
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
    let mut sampler = Sampler::prime(pss_interval);
    let mut first = true;
    loop {
        thread::sleep(interval);
        let tree = sampler.tick(first);
        first = false;
        emit(&tree)?;
    }
}

pub(crate) fn sample_world(interval: Duration) -> HostTree {
    let mut sampler = Sampler::prime(interval);
    thread::sleep(interval);
    sampler.tick(true)
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
            let mut sampler = Sampler::prime(pss_interval);
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

    /// State is the field before ppid, so reading the wrong offset would
    /// still parse and quietly call every process stalled or none of them.
    #[test]
    fn d_state_comes_from_the_state_field_not_a_neighbour() {
        let stat = |state: &str| {
            let mut t = format!("7 (some proc) {state} 1 0 0 0 0 0 0 0 0 0 1 2");
            for _ in 0..20 {
                t.push_str(" 0");
            }
            t
        };
        assert!(parse_stat(&stat("D")).unwrap().d_state);
        for other in ["R", "S", "Z", "I", "T"] {
            assert!(!parse_stat(&stat(other)).unwrap().d_state, "{other}");
        }
        // A comm containing the letter must not be mistaken for the state.
        assert!(
            !parse_stat("7 (D) S 1 0 0 0 0 0 0 0 0 0 1 2")
                .unwrap()
                .d_state
        );
    }

    #[test]
    fn clamp_floors_and_pss_at_least_interval() {
        let (i, p) = clamp_intervals(1.0, 5.0);
        assert_eq!(i, Duration::from_secs(1));
        assert_eq!(p, Duration::from_secs(5));
        let (i, p) = clamp_intervals(0.01, 0.01);
        assert_eq!(i, Duration::from_millis(50));
        assert_eq!(p, Duration::from_millis(50));
        let (i, p) = clamp_intervals(10.0, 5.0);
        assert_eq!(i, Duration::from_secs(10));
        assert_eq!(p, Duration::from_secs(10));
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
            ..Process::default()
        };
        assert_eq!(
            rollup_for(false, true, true, Some(&carried), 1),
            (None, None)
        );
        assert_eq!(
            rollup_for(false, true, false, Some(&carried), 1),
            (Some(12), Some(3))
        );
        assert_eq!(rollup_for(false, true, false, None, 1), (None, None));
    }
}
