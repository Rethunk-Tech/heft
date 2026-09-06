use serde::Serialize;

#[derive(Clone, Debug, Default)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub pgrp: i32,
    pub sid: i32,
    pub uid: u32,
    pub comm: String,
    pub exe: Option<String>,
    pub cmdline: Vec<String>,
    pub cgroup: String,
    pub utime: u64,
    pub stime: u64,
    pub rss_pages: Option<u64>,
    pub pss_kb: Option<u64>,
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
}

impl Metrics {
    pub(crate) fn accumulate(&mut self, other: &Metrics) {
        self.cpu_core_pct += other.cpu_core_pct;
        self.cpu_machine_pct += other.cpu_machine_pct;
        self.rss_bytes = sum_opt(self.rss_bytes, other.rss_bytes);
        self.pss_bytes = sum_opt(self.pss_bytes, other.pss_bytes);
        self.disk_r_bps = sum_opt_f(self.disk_r_bps, other.disk_r_bps);
        self.disk_w_bps = sum_opt_f(self.disk_w_bps, other.disk_w_bps);
        self.vram_bytes = sum_opt(self.vram_bytes, other.vram_bytes);
        self.gtt_bytes = sum_opt(self.gtt_bytes, other.gtt_bytes);
        self.gfx_pct = sum_opt_f(self.gfx_pct, other.gfx_pct);
        self.compute_pct = sum_opt_f(self.compute_pct, other.compute_pct);
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
    pub(crate) nproc: u32,
    pub(crate) cpu_pct: f64,
    pub(crate) cpu_user_pct: f64,
    pub(crate) cpu_system_pct: f64,
    pub(crate) cpu_wait_pct: f64,
    pub(crate) mem_used_bytes: u64,
    pub(crate) mem_total_bytes: u64,
    pub(crate) mem_buffers_bytes: u64,
    pub(crate) mem_cached_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) vram_used_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) vram_total_bytes: Option<u64>,
    pub(crate) unified_memory: bool,
    pub users: Vec<UserNode>,
    pub containers: Vec<IdentNode>,
    pub system: Vec<IdentNode>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UserNode {
    pub uid: u32,
    pub(crate) name: String,
    pub applications: Vec<IdentNode>,
    pub user_services: Vec<IdentNode>,
    pub containers: Vec<IdentNode>,
}

#[derive(Clone, Debug, Serialize)]
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

#[derive(Clone, Debug, Serialize)]
pub struct MemberContainer {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) nproc: u32,
    #[serde(flatten)]
    pub(crate) metrics: Metrics,
    pub(crate) processes: Vec<ProcNode>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InstanceNode {
    pub(crate) key: String,
    pub(crate) nproc: u32,
    #[serde(flatten)]
    pub(crate) metrics: Metrics,
    pub processes: Vec<ProcNode>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProcNode {
    pub(crate) pid: u32,
    pub name: String,
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

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self(e.to_string())
    }
}
