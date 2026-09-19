//! Reconciliation against the kernel, not against heft.
//!
//! `live_proc.rs` proves the tree is self-consistent: a row is the fold of its
//! processes, the header bounds the tree. None of it would notice if every
//! number heft prints were uniformly wrong, because heft is the only oracle.
//! This file parses `/proc/stat`, `/proc/meminfo` and `smaps_rollup` itself
//! and holds heft to what it finds.
//!
//! Exact equality is right for two of these and wrong for the rest, and the
//! reasons are the ones HUMANS.md already documents: the Host row sums only
//! the PIDs heft can see, PSS leaves kernel and page-cache memory billed to
//! nobody, and heft's sampling window is a strict sub-interval of any window
//! a test can draw around the process. So the memory and CPU assertions are
//! containment arguments, each with a slack that has to hold on a busy desktop
//! under CPU and page-cache load.

use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

mod common;
use common::{arr, heft, idents, pids, procs_of};

/// heft's CPU window is this sleep and nothing else; the outer window a test
/// can draw around it adds a full `/proc` walk on either side. Every CPU bound
/// here is a containment argument whose tightness is the ratio between the
/// two, so the interval has to be long enough that the walks do not swamp it.
const INTERVAL: f64 = 1.0;

struct Sample {
    host: Value,
    /// Busy and total tick deltas over a window that strictly contains heft's:
    /// read before the fork and after the child was reaped.
    my_busy: u64,
    my_total: u64,
    /// Seconds that outer window spanned. `my_total / wall` is `CLK_TCK *
    /// nproc` measured rather than asked for, which is what lets heft's share
    /// of the window be derived without libc.
    wall: f64,
    nproc: u64,
    mem_total: u64,
    /// Low and high water of `MemTotal - MemAvailable` while heft ran.
    used_lo: u64,
    used_hi: u64,
    /// pid -> PSS bytes straight from `smaps_rollup`, read as soon as heft
    /// exited. `None` is a pid the kernel published no usable rollup for,
    /// which is every kernel thread.
    oracle: HashMap<u32, Option<u64>>,
    /// pids whose `stat` this test read both before and after the run. A pid
    /// in this set was alive across heft's entire walk, so heft had no excuse
    /// to miss it.
    common: HashSet<u32>,
}

/// One live run shared by every test here, as in `live_proc.rs`: each test
/// names one reconciliation, so a failure still says which.
fn sample() -> &'static Sample {
    static SAMPLE: OnceLock<Sample> = OnceLock::new();
    SAMPLE.get_or_init(|| {
        let before: HashSet<u32> = scan().into_keys().collect();

        // MemAvailable moves under heft's feet, so bracket it rather than
        // guess a slack. The poll period is far below the interval heft
        // spends between its two walks.
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let poll = std::thread::spawn(move || {
            let (total, mut lo, mut hi) = meminfo();
            while !flag.load(Ordering::Relaxed) {
                let (_, used, _) = meminfo();
                lo = lo.min(used);
                hi = hi.max(used);
                std::thread::sleep(Duration::from_millis(5));
            }
            (total, lo, hi)
        });

        let t0 = Instant::now();
        let cpu0 = host_cpu();
        let out = heft(&["--json", "--interval", &INTERVAL.to_string()])
            .stderr(Stdio::piped())
            .output()
            .expect("run heft --json");
        let cpu1 = host_cpu();
        let wall = t0.elapsed().as_secs_f64();
        stop.store(true, Ordering::Relaxed);
        let (mem_total, used_lo, used_hi) = poll.join().expect("meminfo poller");

        // The rollups heft just read, read again as close behind it as this
        // test can get: PSS is a level, and distance here is the whole
        // tolerance in `the_tree_carries_the_pss_the_kernel_published`.
        let oracle = scan();
        assert!(out.status.success(), "heft --json exited {}", out.status);
        let doc: Value = serde_json::from_slice(&out.stdout).expect("--json emits valid JSON");
        let alive: HashSet<u32> = oracle.keys().copied().collect();
        Sample {
            host: doc["host"].clone(),
            my_busy: cpu1.0.saturating_sub(cpu0.0),
            my_total: cpu1.1.saturating_sub(cpu0.1),
            wall,
            nproc: nproc(),
            mem_total,
            used_lo,
            used_hi,
            common: before.intersection(&alive).copied().collect(),
            oracle,
        }
    })
}

/// (busy, total) ticks from the aggregate `cpu ` line, parsed here rather than
/// by `cpu::parse_host_cpu`: an oracle that reuses heft's parser proves
/// nothing. Busy is everything the header bar fills — user+nice,
/// system+irq+softirq, iowait — and idle+steal is the unfilled remainder.
/// Guest time already sits inside user and nice, so the first eight fields are
/// the whole machine.
fn host_cpu() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/stat").expect("/proc/stat");
    let line = text
        .lines()
        .find(|l| l.starts_with("cpu "))
        .expect("an aggregate cpu line");
    let v: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(|t| t.parse().expect("a /proc/stat counter"))
        .collect();
    let total: u64 = v.iter().sum();
    let idle = v[3] + v.get(7).copied().unwrap_or(0);
    (total - idle, total)
}

fn nproc() -> u64 {
    std::fs::read_to_string("/proc/stat")
        .expect("/proc/stat")
        .lines()
        .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
        .count() as u64
}

/// (`MemTotal`, used, used) in bytes, where used is `MemTotal - MemAvailable` —
/// what `mem::parse_meminfo` publishes as `mem_used_bytes`, and deliberately
/// not `MemTotal - MemFree`, which would call the page cache used.
fn meminfo() -> (u64, u64, u64) {
    let text = std::fs::read_to_string("/proc/meminfo").expect("/proc/meminfo");
    let field = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
            .map_or_else(|| panic!("{k} in /proc/meminfo"), |kb| kb * 1024)
    };
    let (total, avail) = (field("MemTotal:"), field("MemAvailable:"));
    let used = total - avail;
    (total, used, used)
}

/// Every pid whose `stat` is readable, with its rollup PSS. `stat` is heft's
/// own admission test (`proc::read_pid` gives up there first), so this is the
/// population heft is answerable for.
fn scan() -> HashMap<u32, Option<u64>> {
    let mut out = HashMap::new();
    for pid in pids() {
        let pid = u32::try_from(pid).expect("a Linux pid is below pid_max, at most 2^22");
        if std::fs::read_to_string(format!("/proc/{pid}/stat")).is_err() {
            continue;
        }
        // `Pss:` only, as `io::parse_pss_kb` does: Pss_Anon and the rest are
        // slices of it and would double the total.
        let pss = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
            .ok()
            .and_then(|t| {
                t.lines().find_map(|l| {
                    l.strip_prefix("Pss:")?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
            })
            .map(|kb| kb * 1024);
        out.insert(pid, pss);
    }
    out
}

/// pid -> the PSS heft published for it. `None` is the blank cell HUMANS.md
/// promises for a metric heft could not read, which is why the value is an
/// `Option` and not a zero.
fn tree_pss(host: &Value) -> HashMap<u32, Option<u64>> {
    let mut out = HashMap::new();
    for (_, ident) in idents(host) {
        for inst in arr(ident, "instances") {
            for p in procs_of(inst) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a pid is a u32 in /proc and in the tree; the JSON widened it, this narrows it back"
                )]
                let pid = p["pid"].as_u64().expect("a process node carries its pid") as u32;
                out.insert(pid, p.get("pss_bytes").and_then(Value::as_u64));
            }
        }
    }
    out
}

#[test]
fn the_kernel_constants_are_copied_not_computed() {
    // MemTotal and the CPU count come out of the same two files this test
    // just read, unscaled, so anything but equality is a parse bug. Nothing
    // else in the header gets to be exact.
    let s = sample();
    assert_eq!(
        s.host["mem_total_bytes"].as_u64(),
        Some(s.mem_total),
        "mem_total_bytes is MemTotal in bytes"
    );
    assert_eq!(
        s.host["nproc"].as_u64(),
        Some(s.nproc),
        "nproc is the count of cpuN lines in /proc/stat"
    );
}

#[test]
fn mem_used_is_memtotal_minus_memavailable_while_heft_ran() {
    let s = sample();
    let used = s.host["mem_used_bytes"].as_u64().expect("mem_used_bytes");
    // The envelope is the tolerance: heft read /proc/meminfo at one instant
    // inside its own run, and the poller bracketed every instant in it.
    // The slack is only for a poller starved off-CPU, and stays well under
    // the signal a wrong field would give: MemFree in place of MemAvailable
    // reads further apart the more cache a host holds.
    let slack = 256 * 1024 * 1024;
    assert!(
        used + slack >= s.used_lo && used <= s.used_hi + slack,
        "mem_used {used} is outside the [{}, {}] MemTotal-MemAvailable seen while heft ran (slack {slack})",
        s.used_lo,
        s.used_hi
    );
}

#[test]
#[expect(
    clippy::cast_precision_loss,
    reason = "tick counters over one test interval are millions, not 2^53"
)]
fn host_cpu_claims_no_more_busy_ticks_than_the_kernel_counted() {
    let s = sample();
    let pct = s.host["cpu_pct"].as_f64().expect("cpu_pct");

    // heft's window is `--interval` of sleep between two `/proc/stat` reads,
    // and it opens and closes strictly inside the window this test measured,
    // so every busy tick heft billed is one of the ticks counted here. What
    // heft publishes is a percentage, so recovering its ticks needs its
    // window's capacity: at least INTERVAL seconds of it, at the tick rate
    // `my_total / wall` measures. That underestimates heft's real window,
    // which also covers a container-API poll, so the bound stays conservative.
    //
    // This is containment, not correlation: a busier machine only raises the
    // right-hand side, which is what makes it safe where "sum of per-process
    // CPU <= host CPU" was not. How much it catches depends on how much of the
    // outer window heft's own window covers: loose on a desktop where two
    // walks are slow, tight in a near-empty PID namespace, which is the shape
    // CI runs. It always catches the errors that move a figure by a factor: a
    // tick count read as seconds, or a per-core percentage published as a
    // per-machine one.
    let capacity = s.my_total as f64 * (INTERVAL / s.wall);
    let claimed = pct / 100.0 * capacity;
    // /proc/stat accrues in whole ticks per CPU, so the outer window can read
    // a tick per CPU short at each end. Twice that theoretical bound; on an
    // idle many-core box it is the dominant term.
    let slack = (4 * s.nproc) as f64;
    assert!(
        claimed <= s.my_busy as f64 + slack,
        "cpu_pct {pct:.2} over a >={INTERVAL}s window claims {claimed:.0} busy ticks; \
         the kernel counted {} busy of {} total across the {:.2}s that contains it",
        s.my_busy,
        s.my_total,
        s.wall
    );
}

#[test]
fn every_pid_alive_across_the_run_is_in_the_tree() {
    // The Host row sitting below the header is only legitimate when the
    // missing weight belongs to processes heft could not read. This is the
    // half of that claim a test can check: a pid whose `stat` this test read
    // both before and after the run was alive through heft's whole walk and
    // was readable by the same uid, so heft skipping it is heft dropping a row
    // it could have had, not the kernel withholding one.
    let s = sample();
    let tree = tree_pss(&s.host);
    let missing: Vec<u32> = s
        .common
        .iter()
        .copied()
        .filter(|p| !tree.contains_key(p))
        .collect();
    assert!(!s.common.is_empty(), "this test saw no processes at all");
    assert!(
        missing.is_empty(),
        "{} of {} pids readable across the whole run are in no row: {:?}",
        missing.len(),
        s.common.len(),
        &missing[..missing.len().min(20)]
    );
}

#[test]
fn the_tree_carries_the_pss_the_kernel_published() {
    let s = sample();
    let tree = tree_pss(&s.host);
    let mut blank = Vec::new();
    let (mut oracle, mut heft) = (0u64, 0u64);
    for pid in &s.common {
        // Restricting to pids both sides saw takes the population difference
        // out, leaving only the drift between heft's rollup read and this
        // test's. Reasons 1 and 3 (invisible processes, memory owned by no
        // process) are about the gap to `mem_used`, which this comparison
        // deliberately does not go near.
        let (Some(Some(mine)), Some(theirs)) = (s.oracle.get(pid), tree.get(pid)) else {
            continue;
        };
        match theirs {
            Some(v) => {
                oracle += mine;
                heft += v;
            }
            // A rollup this test can read is one heft could read a moment
            // earlier, and a kernel thread has none for either of us, so a
            // non-zero here is heft blanking a cell it had the number for.
            None => blank.push(*pid),
        }
    }
    assert!(
        blank.is_empty(),
        "{} pids have a readable smaps_rollup but a blank PSS in the tree: {:?}",
        blank.len(),
        &blank[..blank.len().min(20)]
    );
    assert!(oracle > 0, "no pid offered a rollup to reconcile against");

    // PSS is a level, and heft read it up to a walk earlier than this test
    // did, so a browser or a compiler moves the total under both of them.
    // 5% covers that drift and stays well below the smallest wrong-field
    // signal, summing RSS instead of PSS. The 64 MiB floor is for a
    // near-empty container, where a few processes make the relative term
    // meaninglessly small.
    let slack = (oracle / 20).max(64 * 1024 * 1024);
    assert!(
        heft.abs_diff(oracle) <= slack,
        "summed PSS: heft {heft}, kernel {oracle}, apart by {} (slack {slack})",
        heft.abs_diff(oracle)
    );
}
