//! Invariants over a real `/proc`, not a fixture.
//!
//! `tests/fixtures` pins hard desktop shapes (launcher chains, helper folding,
//! container ownership) that a CI container never grows. This file pins the
//! other layer: what must hold on *any* Linux, so nothing here may name an
//! application, a GPU, a container runtime, or a uid. Every assertion is
//! either structural (the tree heft just printed is self-consistent) or true
//! of the kernel itself (a `PF_KTHREAD` is a kernel thread everywhere).
//!
//! Anything that needs a specific process to exist is a load-dependent
//! failure waiting for CI, so the only process asserted present is the heft
//! run itself.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_heft");
/// The floor `clamp_intervals` allows: the two `/proc` walks this far apart.
const FAST: &str = "0.05";

/// Keys `Metrics` skips when the value is `None`. A missing key is the blank
/// cell HUMANS.md promises; `0` would be a lie about a metric heft could not
/// read, so these are only ever compared against each other, never to a
/// number.
const COUNTERS: [&str; 6] = [
    "rss_bytes",
    "pss_bytes",
    "swap_bytes",
    "threads",
    "vram_bytes",
    "gtt_bytes",
];
const RATES: [&str; 6] = [
    "cpu_core_pct",
    "cpu_machine_pct",
    "disk_r_bps",
    "disk_w_bps",
    "gfx_pct",
    "compute_pct",
];

struct Sample {
    host: Value,
    /// heft's own pid. It is alive across both walks by construction and is
    /// the one process whose `/proc` files it is always allowed to read, so
    /// it is the only presence assertion that cannot go stale.
    self_pid: u64,
    /// `PF_KTHREAD` pids seen both before and after the run. Intersecting the
    /// two scans drops the kworker that was born or reaped mid-walk, which is
    /// the only way this set could disagree with what heft saw.
    kthreads: Vec<u64>,
    /// Every pid seen both before and after, kernel and userspace alike. A
    /// process that was in `/proc` at both ends was in `/proc` throughout, so
    /// heft had no excuse for missing it.
    alive: Vec<u64>,
}

/// One live walk shared by every test here; each test names one invariant, so
/// a failure still says which. Sampling twice would only double the runtime.
fn sample() -> &'static Sample {
    static SAMPLE: OnceLock<Sample> = OnceLock::new();
    SAMPLE.get_or_init(|| {
        let before = kthread_pids();
        let alive_before = all_pids();
        let child = heft(&["--json", "--interval", FAST])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn heft --json");
        let self_pid = u64::from(child.id());
        let out = child.wait_with_output().expect("wait for heft --json");
        let after = kthread_pids();
        let alive_after = all_pids();
        assert!(out.status.success(), "heft --json exited {}", out.status);
        assert!(
            out.stderr.is_empty(),
            "heft --json wrote to stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc: Value = serde_json::from_slice(&out.stdout).expect("--json emits valid JSON");
        Sample {
            host: doc["host"].clone(),
            self_pid,
            kthreads: before.into_iter().filter(|p| after.contains(p)).collect(),
            alive: alive_before
                .into_iter()
                .filter(|p| alive_after.contains(p))
                .collect(),
        }
    })
}

/// Point the child at a config directory that does not exist, so a developer's
/// saved view or grouping overrides cannot reshape the tree under test.
fn heft(args: &[&str]) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(args).env(
        "XDG_CONFIG_HOME",
        std::env::temp_dir().join("heft-no-config"),
    );
    cmd
}

/// Every pid `/proc` lists, straight from the directory.
fn all_pids() -> Vec<u64> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u64>().ok()))
        .collect()
}

/// Kernel threads straight from the kernel, parsed independently of heft: a
/// test that asked heft where the kernel threads are would prove nothing.
fn kthread_pids() -> Vec<u64> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ent in dir.flatten() {
        let Some(pid) = ent.file_name().to_str().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        if is_kthread(&stat) {
            out.push(pid);
        }
    }
    out
}

/// `PF_KTHREAD` is `stat` field 9 (flags), the 7th token after the `)` that
/// closes a comm which may itself contain spaces and parentheses.
fn is_kthread(stat: &str) -> bool {
    let Some(close) = stat.rfind(')') else {
        return false;
    };
    stat[close + 1..]
        .split_whitespace()
        .nth(6)
        .and_then(|f| f.parse::<u32>().ok())
        .is_some_and(|flags| flags & 0x0020_0000 != 0)
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

/// Every identity row in the tree, with the folder path that leads to it.
fn idents(host: &Value) -> Vec<(String, &Value)> {
    let mut out = Vec::new();
    for user in arr(host, "users") {
        let uid = user["uid"].as_u64().expect("a user node carries its uid");
        for folder in ["applications", "user_services", "containers"] {
            for ident in arr(user, folder) {
                out.push((format!("uid{uid}/{folder}"), ident));
            }
        }
    }
    for folder in ["containers", "system"] {
        for ident in arr(host, folder) {
            out.push((format!("host/{folder}"), ident));
        }
    }
    out
}

/// Flatten one node's process forest. Depth is bounded by the real ancestry
/// heft copied out of `/proc`, so recursion cannot outrun the stack.
fn procs_of(parent: &Value) -> Vec<&Value> {
    fn walk<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
        out.push(node);
        for child in arr(node, "children") {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    for root in arr(parent, "processes") {
        walk(root, &mut out);
    }
    out
}

/// `types::sum_opt`: blank plus a number is that number, and the result is
/// blank only when every input was. Mirrored here so the roll-up assertions
/// test heft's arithmetic rather than restate it.
fn sum_u64<'a>(nodes: impl IntoIterator<Item = &'a Value>, key: &str) -> Option<u64> {
    let mut acc: Option<u64> = None;
    for n in nodes {
        if let Some(v) = n.get(key).and_then(Value::as_u64) {
            acc = Some(acc.unwrap_or(0) + v);
        }
    }
    acc
}

/// pid -> the one row it was billed to. Panics on a second sighting, which is
/// the double-count this map exists to rule out.
fn placement(host: &Value) -> HashMap<u64, String> {
    let mut seen: HashMap<u64, String> = HashMap::new();
    for (folder, ident) in idents(host) {
        let row = format!("{folder}/{}", ident["id"].as_str().unwrap_or("?"));
        for inst in arr(ident, "instances") {
            for p in procs_of(inst) {
                let pid = p["pid"].as_u64().expect("a process node carries its pid");
                if let Some(first) = seen.insert(pid, row.clone()) {
                    panic!("pid {pid} billed to both {first} and {row}");
                }
            }
        }
    }
    seen
}

#[test]
fn every_pid_is_billed_to_exactly_one_row() {
    let host = &sample().host;
    let seen = placement(host);
    assert!(!seen.is_empty(), "a live walk saw no processes at all");

    // A project row republishes its members' processes under `containers[]`.
    // That second view may not introduce a pid the tree does not already
    // account for, and it is the only legal repetition.
    for (folder, ident) in idents(host) {
        for member in arr(ident, "containers") {
            for p in procs_of(member) {
                let pid = p["pid"].as_u64().expect("pid");
                assert!(
                    seen.contains_key(&pid),
                    "{folder}/{} member {} lists pid {pid}, which no row owns",
                    ident["id"],
                    member["id"]
                );
            }
        }
    }
}

/// The walk is split across threads, so the failure this guards is a chunk of
/// the pid list going missing: the tree still parses, still reconciles, and is
/// quietly short a slice of the machine. Nothing else here would notice, since
/// every other invariant is about the pids that *are* present.
#[test]
fn a_pid_alive_across_the_whole_walk_is_never_dropped() {
    let s = sample();
    let seen = placement(&s.host);
    let missing: Vec<u64> = s
        .alive
        .iter()
        .copied()
        .filter(|pid| !seen.contains_key(pid))
        .collect();
    assert!(
        missing.is_empty(),
        "{} of {} pids alive across the whole walk are billed to no row: {:?}",
        missing.len(),
        s.alive.len(),
        &missing[..missing.len().min(20)]
    );
}

#[test]
fn a_rows_totals_are_its_processes_totals() {
    for (folder, ident) in idents(&sample().host) {
        let mut counted = 0;
        for inst in arr(ident, "instances") {
            let procs = procs_of(inst);
            let n = u64::try_from(procs.len()).expect("process count fits u64");
            assert_eq!(
                inst["nproc"].as_u64(),
                Some(n),
                "{folder}/{} instance {} claims {} processes, lists {n}",
                ident["id"],
                inst["key"],
                inst["nproc"]
            );
            counted += n;
            for key in COUNTERS {
                assert_eq!(
                    sum_u64(procs.iter().copied(), key),
                    inst.get(key).and_then(Value::as_u64),
                    "{folder}/{} instance {} {key} is not its processes' sum",
                    ident["id"],
                    inst["key"]
                );
            }
        }
        assert_eq!(
            ident["nproc"].as_u64(),
            Some(counted),
            "{folder}/{} claims {} processes across instances holding {counted}",
            ident["id"],
            ident["nproc"]
        );
        for key in COUNTERS {
            assert_eq!(
                sum_u64(arr(ident, "instances"), key),
                ident.get(key).and_then(Value::as_u64),
                "{folder}/{} {key} is not its instances' sum",
                ident["id"]
            );
        }
    }
}

#[test]
fn heft_observes_itself_with_real_numbers() {
    let s = sample();
    let row = placement(&s.host)
        .remove(&s.self_pid)
        .expect("heft must see its own pid: /proc/self is always readable");

    let mut node = None;
    for (_, ident) in idents(&s.host) {
        for inst in arr(ident, "instances") {
            node = node.or_else(|| {
                procs_of(inst)
                    .into_iter()
                    .find(|p| p["pid"].as_u64() == Some(s.self_pid))
                    .cloned()
            });
        }
    }
    let node = node.expect("the row holding our pid holds its process node");

    let rss = node["rss_bytes"].as_u64();
    assert!(
        rss.is_some_and(|b| b > 0),
        "heft's own RSS must not be blank or zero: {node}"
    );
    // Only assert PSS where the kernel offers it at all: if this test process
    // can read its own rollup, so can heft, and a blank there is a bug rather
    // than an old kernel.
    if std::fs::read_to_string("/proc/self/smaps_rollup").is_ok() {
        assert!(
            node["pss_bytes"].as_u64().is_some_and(|b| b > 0),
            "--json always reads PSS, and heft's own rollup is readable: {node}"
        );
    }
    assert!(
        node["pss_bytes"].as_u64() <= rss,
        "PSS counts a shared page fractionally, so it cannot exceed RSS: {node}"
    );

    // Where heft's own cgroup puts it is a property of the sandbox, not an
    // invariant — under a container scope it is a Containers row with no uid.
    // What must hold is that a uid, once claimed, is the real one.
    if let Some(uid) = row.strip_prefix("uid").and_then(|r| r.split('/').next()) {
        assert_eq!(
            uid,
            self_ruid(),
            "heft billed itself to uid {uid}, we run as {}",
            self_ruid()
        );
    }
}

/// Real uid, the field heft groups by — not the euid, which diverges on setuid.
fn self_ruid() -> String {
    std::fs::read_to_string("/proc/self/status")
        .expect("/proc/self/status")
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|v| v.split_whitespace().next().map(str::to_string))
        .expect("Uid: in /proc/self/status")
}

#[test]
fn kernel_threads_only_ever_land_in_system() {
    let s = sample();
    let seen = placement(&s.host);
    // A pid namespace shows no kernel threads at all, so this is vacuous in a
    // container by design and never fails there for lack of them.
    for pid in &s.kthreads {
        if let Some(row) = seen.get(pid) {
            assert!(
                row.starts_with("host/system/"),
                "PF_KTHREAD pid {pid} landed in {row}"
            );
        }
    }
}

#[test]
fn the_header_bounds_what_the_tree_reports() {
    let host = &sample().host;
    let total = host["mem_total_bytes"].as_u64().expect("mem_total_bytes");
    assert!(total > 0, "MemTotal must be readable");
    assert!(
        host["mem_used_bytes"].as_u64().expect("mem_used_bytes") <= total,
        "clip_used keeps the MEM bar inside MemTotal"
    );
    assert!(host["nproc"].as_u64().is_some_and(|n| n >= 1));
    for key in ["cpu_pct", "cpu_user_pct", "cpu_system_pct", "cpu_wait_pct"] {
        let v = host[key].as_f64().expect(key);
        assert!((0.0..=100.0).contains(&v), "{key} is {v}, not a percentage");
    }
    if let (Some(used), Some(cap)) = (
        host["vram_used_bytes"].as_u64(),
        host["vram_total_bytes"].as_u64(),
    ) {
        assert!(used <= cap, "VRAM {used} used of {cap}");
    }

    // PSS apportions every shared page exactly once across its mappers, so the
    // whole visible machine still fits in RAM. RSS would not: it double-counts.
    let pss: u64 = idents(host)
        .iter()
        .flat_map(|(_, i)| arr(i, "instances"))
        .filter_map(|inst| inst["pss_bytes"].as_u64())
        .sum();
    assert!(pss <= total, "summed PSS {pss} exceeds MemTotal {total}");
}

#[test]
fn a_metric_is_blank_or_sane_but_never_wrong() {
    let host = &sample().host;
    for (folder, ident) in idents(host) {
        for inst in arr(ident, "instances") {
            for node in procs_of(inst) {
                let at = format!("{folder}/{} pid {}", ident["id"], node["pid"]);
                for key in RATES {
                    let Some(v) = node.get(key).and_then(Value::as_f64) else {
                        continue; // blank: heft could not read it, and says so
                    };
                    assert!(v.is_finite() && v >= 0.0, "{at} {key} is {v}");
                }
                // A GPU column is absent without a driver; present, it is a
                // share of one engine and cannot exceed it.
                for key in ["gfx_pct", "compute_pct"] {
                    if let Some(v) = node.get(key).and_then(Value::as_f64) {
                        assert!(v <= 100.0, "{at} {key} is {v}");
                    }
                }
            }
        }
    }
}

#[test]
fn a_pid_vanishing_mid_walk_is_skipped_not_fatal() {
    // PIDs really do die between readdir and the reads that follow it. Churn
    // makes that race likely rather than rare; heft must drop the process, not
    // the sample. `--once` also puts the table path under the same race.
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&stop);
    let churn = std::thread::spawn(move || {
        let mut spawned = 0u32;
        while !flag.load(Ordering::Relaxed) {
            if let Ok(mut c) = Command::new(BIN)
                .arg("--version")
                .stdout(Stdio::null())
                .spawn()
            {
                let _ = c.wait();
                spawned += 1;
            }
        }
        spawned
    });
    let out = heft(&["--once", "--interval", FAST])
        .output()
        .expect("run heft --once");
    stop.store(true, Ordering::Relaxed);
    let spawned = churn.join().expect("churn thread");

    assert!(spawned > 0, "the churn never started; the race was not run");
    assert!(
        out.status.success(),
        "heft --once exited {} under pid churn: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "heft --once warned under pid churn: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Host"),
        "the table still needs its Host row"
    );
}
