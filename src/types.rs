use serde::Serialize;

#[derive(Clone, Debug, Default)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub pgrp: i32,
    pub uid: u32,
    /// `PF_KTHREAD` from `/proc/pid/stat` flags. Also gates the io/fdinfo/PSS
    /// reads a kernel thread has no files for.
    pub kthread: bool,
    pub comm: String,
    pub exe: Option<String>,
    pub cmdline: Vec<String>,
    pub cgroup: String,
    pub utime: u64,
    pub stime: u64,
    /// `num_threads` and `starttime`, fields 20 and 22 of `/proc/pid/stat` —
    /// the file already parsed for utime/stime, so neither costs a read.
    pub threads: Option<u64>,
    pub starttime_ticks: Option<u64>,
    /// State `D` from field 3 of that same `stat` line: in the kernel and not
    /// signallable. It costs no read and is the one stall signal that is a
    /// count rather than a percentage.
    pub d_state: bool,
    pub rss_pages: Option<u64>,
    pub pss_kb: Option<u64>,
    /// `SwapPss:` from the same `smaps_rollup` read as `pss_kb`, so it costs no
    /// extra file and arrives on the same `--pss-interval` cadence.
    pub swap_pss_kb: Option<u64>,
    pub read_bytes: Option<u64>,
    pub write_bytes: Option<u64>,
    pub gpu: GpuCounters,
}

#[derive(Clone, Debug, Default)]
pub struct GpuCounters {
    pub(crate) vram_bytes: Option<u64>,
    pub(crate) gtt_bytes: Option<u64>,
    pub(crate) gfx_ns: Option<u64>,
    pub(crate) compute_ns: Option<u64>,
    /// xe reports engine busy in GPU cycles instead of nanoseconds. Already
    /// divided by the class capacity; `total_cycles` is the divisor.
    pub(crate) gfx_cycles: Option<u64>,
    pub(crate) compute_cycles: Option<u64>,
    /// The GPU timestamp both cycle counters are measured against. One clock
    /// per device, so it is carried, never summed.
    pub(crate) total_cycles: Option<u64>,
}

/// Kernel constants every per-PID rate divides by. Read once at startup;
/// they never change while heft runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostHeader {
    pub nproc: u32,
    pub clk_tck: u64,
    pub page_size: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Metrics {
    pub(crate) cpu_core_pct: f64,
    pub(crate) cpu_machine_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rss_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pss_bytes: Option<u64>,
    /// `SwapPss`, not `Swap`: a swapped-out page shared by four processes is
    /// one page of swap, and `Swap` bills it to each of them, so a summed tree
    /// would report four. PSS is apportioned for the same reason, and the
    /// summed-PSS-fits-in-RAM invariant depends on that apportionment holding
    /// for swap too. Blank when the host has no swap configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) swap_bytes: Option<u64>,
    /// Threads sum the way `nproc` does: heft's `N` is a process count, so a
    /// process that spawned 4000 threads was indistinguishable from one that
    /// spawned none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) threads: Option<u64>,
    /// Seconds since the process started. On a row covering several processes
    /// this is the OLDEST of them — when the thing on this row first appeared
    /// — never a sum, which for a duration would be meaningless. `accumulate`
    /// takes the max for that reason, and a max is why no roll-up assertion
    /// can read it as a total.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) age_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disk_r_bps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disk_w_bps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) vram_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) gtt_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) gfx_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) compute_pct: Option<f64>,
    /// `/proc/<pid>/net/dev` is per network namespace, not per process, so
    /// these carry a container's traffic and are blank on every other row.
    /// `accumulate` deliberately leaves them out: summing a namespace total
    /// into a folder, User or Host row would present it as that row's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) net_rx_bps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) net_tx_bps: Option<f64>,
    /// Processes in uninterruptible sleep. A count, so unlike the three
    /// percentages below it sums, and a folder, User or Host row carries one
    /// where PSI has to leave a blank. Never an `Option`: every process heft
    /// can see at all has a state, so `0` here is an answer, not a blank.
    pub(crate) d_state_procs: u32,
    /// Percent of the interval this row's cgroup had at least one task stalled
    /// on the resource. Like the netns pair, `accumulate` leaves these out: a
    /// percentage of an interval is not a quantity, so summing two cgroups'
    /// stall would produce a number the kernel never measured. A row only
    /// carries them when it *is* one non-root cgroup; see `psi`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cpu_stall_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) io_stall_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mem_stall_pct: Option<f64>,
}

impl Metrics {
    pub(crate) fn accumulate(&mut self, other: &Metrics) {
        self.cpu_core_pct += other.cpu_core_pct;
        self.cpu_machine_pct += other.cpu_machine_pct;
        self.rss_bytes = sum_opt(self.rss_bytes, other.rss_bytes);
        self.pss_bytes = sum_opt(self.pss_bytes, other.pss_bytes);
        self.swap_bytes = sum_opt(self.swap_bytes, other.swap_bytes);
        self.threads = sum_opt(self.threads, other.threads);
        self.age_secs = self.age_secs.max(other.age_secs);
        self.disk_r_bps = sum_opt_f(self.disk_r_bps, other.disk_r_bps);
        self.disk_w_bps = sum_opt_f(self.disk_w_bps, other.disk_w_bps);
        self.vram_bytes = sum_opt(self.vram_bytes, other.vram_bytes);
        self.gtt_bytes = sum_opt(self.gtt_bytes, other.gtt_bytes);
        self.gfx_pct = sum_opt_f(self.gfx_pct, other.gfx_pct);
        self.compute_pct = sum_opt_f(self.compute_pct, other.compute_pct);
        self.d_state_procs += other.d_state_procs;
        // net_rx_bps / net_tx_bps and the three *_stall_pct are not summed;
        // see their field comments.
    }
}

pub(crate) fn sum_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        (x, y) => Some(x.unwrap_or(0) + y.unwrap_or(0)),
    }
}

fn sum_opt_f(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (None, None) => None,
        (x, y) => Some(x.unwrap_or(0.0) + y.unwrap_or(0.0)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Folder {
    Applications,
    UserServices,
    Containers,
    System,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct HostTree {
    /// When this sample was taken, unix seconds. A `--json --follow` stream is
    /// one document per line with no other clock in it, so a reader holding two
    /// lines has nothing else to tell how far apart they were sampled.
    pub sampled_at: u64,
    /// The kernel's own thread count, from `/proc/loadavg`. A global counter
    /// rather than a walk, so it survives what hides pids from `/proc`, which
    /// is what makes it a check on how much of the machine heft can see.
    ///
    /// Not serialized: it exists to qualify what the tree reports, and a
    /// consumer of the JSON is reading the tree itself.
    #[serde(skip)]
    pub(crate) kernel_threads: Option<u64>,
    pub(crate) nproc: u32,
    pub(crate) cpu_pct: f64,
    pub(crate) cpu_user_pct: f64,
    pub(crate) cpu_system_pct: f64,
    pub(crate) cpu_wait_pct: f64,
    pub(crate) mem_used_bytes: u64,
    pub(crate) mem_total_bytes: u64,
    pub(crate) mem_buffers_bytes: u64,
    pub(crate) mem_cached_bytes: u64,
    pub(crate) swap_used_bytes: u64,
    pub(crate) swap_total_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) vram_used_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) vram_total_bytes: Option<u64>,
    pub(crate) unified_memory: bool,
    /// The kernel's own machine-wide `some avg10`, for the header only. The
    /// table's stall columns are interval deltas instead; the two time bases
    /// are deliberate, the way gfx%/compute% already use two formulas.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) psi_cpu_avg10: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) psi_io_avg10: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) psi_mem_avg10: Option<f64>,
    pub users: Vec<UserNode>,
    pub containers: Vec<IdentNode>,
    pub system: Vec<IdentNode>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct UserNode {
    pub uid: u32,
    pub(crate) name: String,
    pub applications: Vec<IdentNode>,
    pub user_services: Vec<IdentNode>,
    pub containers: Vec<IdentNode>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct IdentNode {
    pub id: String,
    pub title: String,
    pub(crate) nproc: u32,
    #[serde(flatten)]
    pub(crate) metrics: Metrics,
    pub instances: Vec<InstanceNode>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub containers: Vec<MemberContainer>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MemberContainer {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) nproc: u32,
    #[serde(flatten)]
    pub(crate) metrics: Metrics,
    pub(crate) processes: Vec<ProcNode>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct InstanceNode {
    pub(crate) key: String,
    pub(crate) nproc: u32,
    #[serde(flatten)]
    pub(crate) metrics: Metrics,
    pub processes: Vec<ProcNode>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ProcNode {
    pub(crate) pid: u32,
    pub name: String,
    /// The argv this process was started with, space-joined. `name` is the exe
    /// basename, so nothing else on the node answers which of four identical
    /// workers holds `--port 8080`.
    ///
    /// Serialized, so a `--follow` reader can identify a process from the
    /// record alone. Going back to `/proc` is not equivalent: by the time a
    /// line is read the pid may be gone, or worse, reused.
    pub cmdline: String,
    #[serde(flatten)]
    pub(crate) metrics: Metrics,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ProcNode>,
}

pub(crate) fn folder_nproc(idents: &[IdentNode]) -> u32 {
    idents.iter().map(|i| i.nproc).sum()
}

pub(crate) fn sum_idents(idents: &[IdentNode]) -> Metrics {
    let mut m = Metrics::default();
    for i in idents {
        m.accumulate(&i.metrics);
    }
    m
}

pub(crate) fn user_metrics(user: &UserNode) -> Metrics {
    let mut m = sum_idents(&user.applications);
    m.accumulate(&sum_idents(&user.user_services));
    m.accumulate(&sum_idents(&user.containers));
    m
}

pub(crate) fn user_nproc(user: &UserNode) -> u32 {
    folder_nproc(&user.applications)
        + folder_nproc(&user.user_services)
        + folder_nproc(&user.containers)
}

pub(crate) fn tree_host_nproc(tree: &HostTree) -> u32 {
    tree.users.iter().map(user_nproc).sum::<u32>()
        + folder_nproc(&tree.containers)
        + folder_nproc(&tree.system)
}

pub(crate) fn host_metrics(tree: &HostTree) -> Metrics {
    let mut m = sum_idents(&tree.containers);
    m.accumulate(&sum_idents(&tree.system));
    for u in &tree.users {
        m.accumulate(&user_metrics(u));
    }
    m
}

/// Errors here only ever reach `main`, which prints them and exits, so nothing
/// matches on a variant. `Box<dyn Error>` gets the `?` conversions from std;
/// it need not be `Send` because the sampler thread returns `io::Result`.
pub type Error = Box<dyn std::error::Error>;

#[cfg(test)]
mod tests {
    use super::Metrics;

    /// The rule both the netns pair and the stall trio depend on. Every other
    /// column here is a quantity that adds up; these are a namespace's traffic
    /// and a percentage of an interval, and a folder, User or Host row that
    /// summed them would present one cgroup's stall as its own. Guarding it on
    /// `accumulate` catches it at the only place it could go wrong, rather
    /// than on whichever row happened to be checked.
    #[test]
    fn a_rate_that_belongs_to_one_namespace_or_cgroup_is_never_summed() {
        let with = |v: f64| Metrics {
            pss_bytes: Some(4),
            d_state_procs: 1,
            net_rx_bps: Some(v),
            net_tx_bps: Some(v),
            cpu_stall_pct: Some(v),
            io_stall_pct: Some(v),
            mem_stall_pct: Some(v),
            ..Metrics::default()
        };
        let mut folder = with(10.0);
        folder.accumulate(&with(20.0));

        assert_eq!(folder.pss_bytes, Some(8), "quantities still add up");
        // The contrast that makes the rule legible: D is a count of processes,
        // so it rolls up the way PSS does even though it answers the same
        // question as the three percentages that cannot.
        assert_eq!(folder.d_state_procs, 2, "a count still adds up");
        for (name, got) in [
            ("net_rx_bps", folder.net_rx_bps),
            ("net_tx_bps", folder.net_tx_bps),
            ("cpu_stall_pct", folder.cpu_stall_pct),
            ("io_stall_pct", folder.io_stall_pct),
            ("mem_stall_pct", folder.mem_stall_pct),
        ] {
            assert_eq!(got, Some(10.0), "{name} was summed into its parent");
        }
    }
}
