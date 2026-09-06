use std::cmp::Ordering;
use std::io::{self, Write};
use std::time::Duration;

use crate::config::View;
use crate::proc;
use crate::types::{
    Error, HostTree, IdentNode, Metrics, ProcNode, folder_nproc, host_metrics, sum_idents,
    tree_host_nproc, user_metrics, user_nproc,
};

/// One table column. Adding a column here is the whole change: it reaches the
/// `--once` table, the TUI table, and the sort cycle at once.
pub(crate) struct Column {
    /// Persisted in the saved view, so renaming one invalidates that sort.
    pub(crate) label: &'static str,
    pub(crate) header: &'static str,
    pub(crate) width: u16,
    pub(crate) fmt: fn(&str, u32, &Metrics) -> String,
    /// `None` on the name column: it has no numeric key, and it inverts the
    /// sort direction so the numeric default of high-to-low still reads A-Z.
    /// The inner `None` is a metric heft could not read (EACCES), which is not
    /// a zero and sorts last in either direction.
    pub(crate) key: Option<fn(u32, &Metrics) -> Option<f64>>,
}

/// What ordering needs from a row. Identity, instance, member container and
/// process rows all reduce to the three values the table already formats, so
/// one comparator orders every level of the tree.
type Row<'a> = (&'a str, u32, &'a Metrics);

pub(crate) const COLUMNS: &[Column] = &[
    Column {
        label: "name",
        header: "NAME",
        width: 28,
        fmt: |name, _, _| name.to_string(),
        key: None,
    },
    Column {
        label: "nproc",
        header: "N",
        width: 4,
        fmt: |_, n, _| n.to_string(),
        key: Some(|n, _| Some(f64::from(n))),
    },
    Column {
        label: "core",
        header: "%CORE",
        width: 7,
        fmt: |_, _, m| fmt_pct(m.cpu_core_pct),
        key: Some(|_, m| Some(m.cpu_core_pct)),
    },
    Column {
        label: "machine",
        header: "%MACH",
        width: 7,
        fmt: |_, _, m| fmt_pct(m.cpu_machine_pct),
        key: Some(|_, m| Some(m.cpu_machine_pct)),
    },
    Column {
        label: "pss",
        header: "PSS",
        width: 8,
        fmt: |_, _, m| fmt_bytes(m.pss_bytes),
        key: Some(|_, m| opt_u(m.pss_bytes)),
    },
    Column {
        label: "rss",
        header: "RSS",
        width: 8,
        fmt: |_, _, m| fmt_bytes(m.rss_bytes),
        key: Some(|_, m| opt_u(m.rss_bytes)),
    },
    Column {
        label: "diskr",
        header: "DISK R",
        width: 8,
        fmt: |_, _, m| fmt_rate(m.disk_r_bps),
        key: Some(|_, m| m.disk_r_bps),
    },
    Column {
        label: "diskw",
        header: "DISK W",
        width: 8,
        fmt: |_, _, m| fmt_rate(m.disk_w_bps),
        key: Some(|_, m| m.disk_w_bps),
    },
    Column {
        label: "vram",
        header: "VRAM",
        width: 8,
        fmt: |_, _, m| fmt_bytes(m.vram_bytes),
        key: Some(|_, m| opt_u(m.vram_bytes)),
    },
    Column {
        label: "gtt",
        header: "GTT",
        width: 8,
        fmt: |_, _, m| fmt_bytes(m.gtt_bytes),
        key: Some(|_, m| opt_u(m.gtt_bytes)),
    },
    Column {
        label: "gfx",
        header: "GFX",
        width: 5,
        fmt: |_, _, m| fmt_opt_pct(m.gfx_pct),
        key: Some(|_, m| m.gfx_pct),
    },
    Column {
        label: "compute",
        header: "CMP",
        width: 5,
        fmt: |_, _, m| fmt_opt_pct(m.compute_pct),
        key: Some(|_, m| m.compute_pct),
    },
    // NETNS, not NET: the counter belongs to a network namespace, and a
    // container is the only thing in this tree that owns one. Naming the
    // column after the resource would make a blank process cell read as "this
    // process moved no bytes" when it means "no namespace of its own, so no
    // figure exists" — the blank contract every other column already uses.
    Column {
        label: "netns_rx",
        header: "NETNS RX",
        width: 8,
        fmt: |_, _, m| fmt_rate(m.net_rx_bps),
        key: Some(|_, m| m.net_rx_bps),
    },
    Column {
        label: "netns_tx",
        header: "NETNS TX",
        width: 8,
        fmt: |_, _, m| fmt_rate(m.net_tx_bps),
        key: Some(|_, m| m.net_tx_bps),
    },
];

/// Byte counts stay exact in f64 out past 9 PB, so one key type covers the
/// integer and rate columns alike.
fn opt_u(v: Option<u64>) -> Option<f64> {
    v.map(|x| x as f64)
}

/// Index into `COLUMNS`; the saved view stores that column's label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sort(usize);

impl Sort {
    /// Only the tests enumerate the sorts; `next` and `from_label` index
    /// `COLUMNS` directly.
    #[cfg(test)]
    fn all() -> Vec<Sort> {
        (0..COLUMNS.len()).map(Sort).collect()
    }

    pub(crate) fn label(self) -> &'static str {
        COLUMNS[self.0].label
    }

    pub(crate) fn from_label(s: &str) -> Self {
        let find = |l: &str| COLUMNS.iter().position(|c| c.label == l);
        // An unknown label means a saved view from another column set; PSS is
        // the documented default.
        Sort(find(s).or_else(|| find("pss")).unwrap_or(0))
    }

    pub(crate) fn next(self) -> Self {
        Sort((self.0 + 1) % COLUMNS.len())
    }
}

fn cmp_row(a: Row<'_>, b: Row<'_>, sort: Sort, desc: bool) -> Ordering {
    let Some(key) = COLUMNS[sort.0].key else {
        let ord = a.0.cmp(b.0);
        return if desc { ord } else { ord.reverse() };
    };
    let ord = match (key(a.1, a.2), key(b.1, b.2)) {
        (Some(x), Some(y)) => {
            let o = x.total_cmp(&y);
            if desc { o.reverse() } else { o }
        }
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (None, None) => Ordering::Equal,
    };
    // Name breaks a metric tie in both directions; below that the sort is
    // stable over the pid order `group::proc_forest` builds, so equal rows do
    // not shuffle between ticks.
    ord.then_with(|| a.0.cmp(b.0))
}

/// The order every published tree starts in, so HUMANS.md's documented
/// default has exactly one definition: the saved view's.
pub(crate) fn sort_default(tree: &mut HostTree) {
    let v = View::default();
    sort_tree(tree, Sort::from_label(&v.sort), v.desc);
}

/// Orders every sibling group in the tree: identity rows, their instances and
/// member containers, and the process forests under both.
pub(crate) fn sort_tree(tree: &mut HostTree, sort: Sort, desc: bool) {
    for u in &mut tree.users {
        for v in [&mut u.applications, &mut u.user_services, &mut u.containers] {
            sort_idents(v, sort, desc);
        }
    }
    sort_idents(&mut tree.containers, sort, desc);
    sort_idents(&mut tree.system, sort, desc);
}

fn sort_idents(idents: &mut [IdentNode], sort: Sort, desc: bool) {
    for i in idents.iter_mut() {
        for inst in &mut i.instances {
            sort_procs(&mut inst.processes, sort, desc);
        }
        i.instances.sort_by(|a, b| {
            cmp_row(
                (&a.key, a.nproc, &a.metrics),
                (&b.key, b.nproc, &b.metrics),
                sort,
                desc,
            )
        });
        for m in &mut i.containers {
            sort_procs(&mut m.processes, sort, desc);
        }
        i.containers.sort_by(|a, b| {
            cmp_row(
                (&a.title, a.nproc, &a.metrics),
                (&b.title, b.nproc, &b.metrics),
                sort,
                desc,
            )
        });
    }
    idents.sort_by(|a, b| {
        cmp_row(
            (&a.title, a.nproc, &a.metrics),
            (&b.title, b.nproc, &b.metrics),
            sort,
            desc,
        )
    });
}

fn sort_procs(procs: &mut [ProcNode], sort: Sort, desc: bool) {
    for p in procs.iter_mut() {
        sort_procs(&mut p.children, sort, desc);
    }
    // A process row counts as one, the same way the table renders it.
    procs.sort_by(|a, b| {
        cmp_row(
            (&a.name, 1, &a.metrics),
            (&b.name, 1, &b.metrics),
            sort,
            desc,
        )
    });
}

/// The fixed-width layout: name left-aligned, every other column right, one
/// space between. Header and body share it so they cannot drift apart.
fn layout(cells: impl IntoIterator<Item = String>) -> String {
    let mut line = String::new();
    for (i, (col, cell)) in COLUMNS.iter().zip(cells).enumerate() {
        if i > 0 {
            line.push(' ');
        }
        let w = usize::from(col.width);
        line.push_str(&if i == 0 {
            format!("{cell:<w$}")
        } else {
            format!("{cell:>w$}")
        });
    }
    line
}

/// # Errors
///
/// Returns an error if writing the table to stdout fails.
pub fn print_table(interval: Duration) -> Result<(), Error> {
    let tree = proc::sample_world(interval);
    let mut out = io::stdout();
    writeln!(
        out,
        "HOST  cpu {:>5.1}%  usr {:>4.1} sys {:>4.1} wait {:>4.1}  mem {} / {}  nproc {}",
        tree.cpu_pct,
        tree.cpu_user_pct,
        tree.cpu_system_pct,
        tree.cpu_wait_pct,
        fmt_bytes(Some(tree.mem_used_bytes)),
        fmt_bytes(Some(tree.mem_total_bytes)),
        tree.nproc
    )?;
    write_header(&mut out)?;
    emit_row(
        &mut out,
        0,
        "Host",
        tree_host_nproc(&tree),
        &host_metrics(&tree),
    )?;
    for user in &tree.users {
        emit_row(
            &mut out,
            1,
            &format!("{} ({})", user.name, user.uid),
            user_nproc(user),
            &user_metrics(user),
        )?;
        write_folder(&mut out, 2, "Applications", &user.applications)?;
        write_folder(&mut out, 2, "User Services", &user.user_services)?;
        write_folder(&mut out, 2, "Containers", &user.containers)?;
    }
    write_folder(&mut out, 1, "Containers", &tree.containers)?;
    write_folder(&mut out, 1, "System", &tree.system)?;
    Ok(())
}

/// # Errors
///
/// Returns an error if the tree cannot be serialized or stdout cannot be written.
pub fn print_json(interval: Duration) -> Result<(), Error> {
    let tree = proc::sample_world(interval);
    let doc = serde_json::json!({ "host": tree });
    // `println!` panics when the reader closes, and the release profile is
    // `panic = abort`, so `heft --json | head` would abort. Serialize first,
    // then write through `io::Write`: serializing into the stream instead
    // would bury the EPIPE inside a `serde_json::Error`, which `main` cannot
    // recognise as a closed pipe.
    let text = serde_json::to_string_pretty(&doc)?;
    writeln!(io::stdout(), "{text}")?;
    Ok(())
}

fn write_header(out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "{}", layout(COLUMNS.iter().map(|c| c.header.into())))
}

fn write_folder(
    out: &mut impl Write,
    depth: usize,
    name: &str,
    idents: &[IdentNode],
) -> io::Result<()> {
    if idents.is_empty() {
        emit_row(out, depth, name, 0, &Metrics::default())?;
        return Ok(());
    }
    emit_row(out, depth, name, folder_nproc(idents), &sum_idents(idents))?;
    for ident in idents {
        emit_ident(out, depth + 1, ident)?;
    }
    Ok(())
}

fn emit_ident(out: &mut impl Write, depth: usize, ident: &IdentNode) -> io::Result<()> {
    emit_row(out, depth, &ident.title, ident.nproc, &ident.metrics)?;
    for member in &ident.containers {
        emit_row(out, depth + 1, &member.title, member.nproc, &member.metrics)?;
    }
    Ok(())
}

fn emit_row(out: &mut impl Write, depth: usize, name: &str, n: u32, m: &Metrics) -> io::Result<()> {
    let indent = "  ".repeat(depth);
    // The name column pads but never clips, so the label is cut to fit first.
    let label = trunc(&format!("{indent}{name}"), usize::from(COLUMNS[0].width));
    writeln!(
        out,
        "{}",
        layout(COLUMNS.iter().map(|c| (c.fmt)(&label, n, m)))
    )
}

fn trunc(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    s.chars().take(width.saturating_sub(1)).collect::<String>() + "…"
}
/// 1024-scale suffix, or None below 1 KiB where the caller decides: a byte
/// count prints exactly, a rate rounds.
fn scale_1024(x: f64) -> Option<String> {
    const K: f64 = 1024.0;
    if x >= K * K * K {
        Some(format!("{:.1}G", x / (K * K * K)))
    } else if x >= K * K {
        Some(format!("{:.1}M", x / (K * K)))
    } else if x >= K {
        Some(format!("{:.1}K", x / K))
    } else {
        None
    }
}

pub(crate) fn fmt_bytes(n: Option<u64>) -> String {
    let Some(b) = n else {
        return String::new();
    };
    scale_1024(b as f64).unwrap_or_else(|| b.to_string())
}

pub(crate) fn fmt_rate(n: Option<f64>) -> String {
    n.map(|v| {
        if v <= 0.0 {
            "0".into()
        } else {
            format!("{}/s", scale_1024(v).unwrap_or_else(|| format!("{v:.0}")))
        }
    })
    .unwrap_or_default()
}
pub(crate) fn fmt_pct(v: f64) -> String {
    format!("{v:.1}")
}

pub(crate) fn fmt_opt_pct(v: Option<f64>) -> String {
    v.map(fmt_pct).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{InstanceNode, ProcNode, UserNode};

    fn metrics(pss: Option<u64>, cpu: f64) -> Metrics {
        Metrics {
            cpu_machine_pct: cpu,
            pss_bytes: pss,
            ..Metrics::default()
        }
    }

    fn proc_node(pid: u32, pss: Option<u64>) -> ProcNode {
        ProcNode {
            pid,
            name: format!("p{pid}"),
            metrics: metrics(pss, 0.0),
            children: Vec::new(),
        }
    }

    /// One identity, two instances whose CPU order is the reverse of their PSS
    /// order, and a process forest in pid order with one unreadable PSS.
    fn ordering_tree() -> HostTree {
        let inst = |key: &str, pss: u64, cpu: f64, procs: Vec<ProcNode>| InstanceNode {
            key: key.into(),
            nproc: 1,
            metrics: metrics(Some(pss), cpu),
            processes: procs,
        };
        HostTree {
            users: vec![UserNode {
                uid: 1000,
                name: "u".into(),
                applications: vec![IdentNode {
                    id: "app".into(),
                    title: "app".into(),
                    nproc: 4,
                    metrics: metrics(Some(1000), 10.0),
                    instances: vec![
                        inst("busy-cpu", 100, 9.0, Vec::new()),
                        inst(
                            "big-pss",
                            900,
                            1.0,
                            vec![
                                proc_node(1, Some(10)),
                                proc_node(2, Some(500)),
                                proc_node(3, None),
                            ],
                        ),
                    ],
                    containers: Vec::new(),
                }],
                user_services: Vec::new(),
                containers: Vec::new(),
            }],
            ..HostTree::default()
        }
    }

    fn instance_keys(tree: &HostTree) -> Vec<&str> {
        tree.users[0].applications[0]
            .instances
            .iter()
            .map(|i| i.key.as_str())
            .collect()
    }

    fn pids(tree: &HostTree) -> Vec<u32> {
        tree.users[0].applications[0]
            .instances
            .iter()
            .find(|i| i.key == "big-pss")
            .expect("big-pss instance")
            .processes
            .iter()
            .map(|p| p.pid)
            .collect()
    }

    /// The `c` / `d` keys have to reach below an identity row, and an
    /// unreadable metric is not a zero: it sits last whichever way the sort
    /// runs.
    #[test]
    fn sort_tree_orders_instances_and_processes() {
        let mut tree = ordering_tree();
        sort_default(&mut tree);
        assert_eq!(instance_keys(&tree), ["big-pss", "busy-cpu"]);
        assert_eq!(pids(&tree), [2, 1, 3]);

        sort_tree(&mut tree, Sort::from_label("pss"), false);
        assert_eq!(instance_keys(&tree), ["busy-cpu", "big-pss"]);
        assert_eq!(pids(&tree), [1, 2, 3]);

        sort_tree(&mut tree, Sort::from_label("machine"), true);
        assert_eq!(instance_keys(&tree), ["busy-cpu", "big-pss"]);
    }

    #[test]
    fn default_sort_is_pss_desc() {
        let v = View::default();
        assert_eq!(v.sort, "pss");
        assert!(v.desc);
        assert_eq!(Sort::from_label("").label(), "pss");
        assert_eq!(Sort::from_label("machine").label(), "machine");
    }

    #[test]
    fn sort_labels_round_trip() {
        for s in Sort::all() {
            assert_eq!(Sort::from_label(s.label()), s, "label {}", s.label());
        }
    }

    #[test]
    fn sort_next_cycles_every_variant() {
        let all = Sort::all();
        let mut s = all[0];
        let mut seen = Vec::new();
        for _ in 0..all.len() {
            s = s.next();
            seen.push(s);
        }
        let mut expect: Vec<Sort> = all[1..].to_vec();
        expect.push(all[0]);
        assert_eq!(seen, expect);
    }

    fn sample_ident(disk_r_bps: f64) -> IdentNode {
        IdentNode {
            id: "a".into(),
            title: "an-identity-name-long-enough-to-truncate".into(),
            nproc: 7,
            metrics: Metrics {
                cpu_core_pct: 12.25,
                cpu_machine_pct: 1.5,
                rss_bytes: Some(2048),
                pss_bytes: Some(1536),
                disk_r_bps: Some(disk_r_bps),
                disk_w_bps: None,
                vram_bytes: Some(1024 * 1024),
                gtt_bytes: None,
                gfx_pct: Some(3.0),
                compute_pct: None,
                // An Applications row can never carry a netns rate; the blank
                // pair is the layout this table shows for every non-container.
                net_rx_bps: None,
                net_tx_bps: None,
            },
            instances: Vec::new(),
            containers: Vec::new(),
        }
    }

    /// `--once` is a fixed-width table other tools slice by column, so the
    /// exact byte layout is the contract, not just the values. The second case
    /// is a cell wider than its column: it pushes the line out rather than
    /// clipping, and downstream slicing has always had to cope with that.
    #[test]
    fn once_rows_keep_their_byte_layout() {
        let mut out = Vec::new();
        write_header(&mut out).unwrap();
        write_folder(&mut out, 1, "Applications", &[sample_ident(1536.0)]).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "NAME                            N   %CORE   %MACH      PSS      RSS   DISK R   DISK W     VRAM      GTT   GFX   CMP NETNS RX NETNS TX\n  Applications                  7    12.2     1.5     1.5K     2.0K   1.5K/s              1.0M            3.0                        \n    an-identity-name-long-e\u{2026}    7    12.2     1.5     1.5K     2.0K   1.5K/s              1.0M            3.0                        \n"
        );

        let mut out = Vec::new();
        write_folder(&mut out, 1, "Applications", &[sample_ident(1_030_963.0)]).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "  Applications                  7    12.2     1.5     1.5K     2.0K 1006.8K/s              1.0M            3.0                        \n    an-identity-name-long-e\u{2026}    7    12.2     1.5     1.5K     2.0K 1006.8K/s              1.0M            3.0                        \n"
        );
    }

    #[test]
    fn scaling_keeps_byte_counts_exact_and_rates_fractional() {
        assert_eq!(fmt_bytes(None), "");
        assert_eq!(fmt_bytes(Some(0)), "0");
        assert_eq!(fmt_bytes(Some(1023)), "1023");
        assert_eq!(fmt_bytes(Some(1024)), "1.0K");
        assert_eq!(fmt_bytes(Some(1024 * 1024)), "1.0M");
        assert_eq!(fmt_bytes(Some(1024 * 1024 * 1024)), "1.0G");
        assert_eq!(fmt_rate(None), "");
        assert_eq!(fmt_rate(Some(0.0)), "0");
        assert_eq!(fmt_rate(Some(-1.0)), "0");
        // the fraction survives instead of truncating through u64
        assert_eq!(fmt_rate(Some(1536.0)), "1.5K/s");
        assert_eq!(fmt_rate(Some(900.6)), "901/s");
    }
}
