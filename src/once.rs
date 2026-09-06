use std::cmp::Ordering;
use std::io::{self, Write};
use std::time::Duration;

use crate::config::{self, View};
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
    // `N` counts processes, so a thread leak was invisible: one process with
    // 4000 threads and one with none rendered identically.
    Column {
        label: "threads",
        header: "THR",
        width: 5,
        fmt: |_, _, m| m.threads.map(|t| t.to_string()).unwrap_or_default(),
        key: Some(|_, m| opt_u(m.threads)),
    },
    // Oldest, not a sum — see `Metrics::age_secs`.
    Column {
        label: "age",
        header: "AGE",
        width: 5,
        fmt: |_, _, m| fmt_age(m.age_secs),
        key: Some(|_, m| opt_u(m.age_secs)),
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
    // Blank, not 0, on a machine with no swap: `SwapTotal: 0` means no figure
    // exists rather than nothing being paged out, the same distinction the
    // NETNS columns draw below.
    Column {
        label: "swap",
        header: "SWAP",
        width: 6,
        fmt: |_, _, m| fmt_bytes(m.swap_bytes),
        key: Some(|_, m| opt_u(m.swap_bytes)),
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
    // Stall, not PSI: the column says what the number means to someone who has
    // never heard of pressure stall information. It is a percentage of the
    // interval the cgroup had at least one task waiting, so it answers what
    // %CORE and PSS cannot — this row is slow because it is *not* running.
    // Blank on every row that is not exactly one non-root cgroup, which is the
    // NETNS blank contract again: no figure exists, rather than a zero.
    Column {
        label: "cpustall",
        header: "CPU ST",
        width: 6,
        fmt: |_, _, m| fmt_opt_pct(m.cpu_stall_pct),
        key: Some(|_, m| m.cpu_stall_pct),
    },
    Column {
        label: "iostall",
        header: "IO ST",
        width: 6,
        fmt: |_, _, m| fmt_opt_pct(m.io_stall_pct),
        key: Some(|_, m| m.io_stall_pct),
    },
    Column {
        label: "memstall",
        header: "MEM ST",
        width: 6,
        fmt: |_, _, m| fmt_opt_pct(m.mem_stall_pct),
        key: Some(|_, m| m.mem_stall_pct),
    },
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

/// The columns one surface renders, as indices into `COLUMNS` in table order.
///
/// Visibility is resolved once, here, so no render site branches on it — and
/// nothing about it reaches sampling: heft reads `/proc` files, not columns, so
/// a hidden column costs exactly the same walk and the roll-up invariants still
/// hold over every metric.
pub(crate) struct Columns(Vec<usize>);

impl Columns {
    /// `view.hide_columns` applied to `COLUMNS`.
    ///
    /// An unknown label warns and is ignored, the way a malformed
    /// `grouping.json` warns and leaves grouping alone: a monitor that refuses
    /// to start over a stale config entry is worse than one with no config.
    /// This is deliberately not `Sort::from_label`'s silent fallback — a
    /// mistyped sort still produces a usable table, a mistyped hide entry would
    /// hide nothing and say nothing.
    pub(crate) fn from_view(view: &View) -> Self {
        let mut hidden: Vec<&str> = Vec::new();
        for label in &view.hide_columns {
            if label == COLUMNS[0].label {
                eprintln!(
                    "heft: {} cannot be hidden; a table of numbers with no labels is unreadable",
                    COLUMNS[0].label
                );
            } else if let Some(c) = COLUMNS.iter().find(|c| c.label == label.as_str()) {
                hidden.push(c.label);
            } else {
                eprintln!(
                    "heft: ignoring unknown column {label:?} in {}",
                    config::view_path().display()
                );
            }
        }
        Self(
            (0..COLUMNS.len())
                .filter(|&i| !hidden.contains(&COLUMNS[i].label))
                .collect(),
        )
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &'static Column> + '_ {
        self.0.iter().map(|&i| &COLUMNS[i])
    }
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

    fn exact(s: &str) -> Option<Self> {
        COLUMNS.iter().position(|c| c.label == s).map(Sort)
    }

    pub(crate) fn from_label(s: &str) -> Self {
        // An unknown label means a saved view from another column set; PSS is
        // the documented default.
        Self::exact(s)
            .or_else(|| Self::exact("pss"))
            .unwrap_or(Sort(0))
    }

    /// The next visible column, wrapping. Cycling onto a hidden one would move
    /// the sort somewhere the reader cannot watch it happen.
    pub(crate) fn next(self, cols: &Columns) -> Self {
        let Some(p) = cols.0.iter().position(|&i| i == self.0) else {
            // A saved view may legally sort by a column it also hides; the
            // cycle then restarts at the first visible one.
            return Sort(cols.0[0]);
        };
        Sort(cols.0[(p + 1) % cols.0.len()])
    }
}

/// Every sort label, so `--sort` can name the valid ones in its usage error.
#[must_use]
pub fn sort_labels() -> Vec<&'static str> {
    COLUMNS.iter().map(|c| c.label).collect()
}

/// Keeps a row when its own name contains `filter`, or when it is an ancestor
/// of a row below that does. `filter` is already lowercased; the name is
/// lowercased here.
///
/// One definition for every surface: the TUI reads it over the flattened rows
/// it draws, `--once` over the rows it prints. Ancestor rows keep the metrics
/// they were built with, so a filtered Host line still totals the machine
/// rather than the match.
///
/// `want` is the depth still needed to complete the ancestor chain of the
/// nearest match below. Tightening it to each kept row's own depth is what
/// limits the walk to that chain: a shallower row on another branch is always
/// preceded by the deeper rows of its own subtree, which do not match and do
/// not lower `want`, so it never becomes an empty header.
/// A compiled `--filter` pattern.
///
/// Case-insensitive by default, via a `(?i)` the pattern never sees: a bare
/// `code` has always matched `Code`, and a regex that quietly became
/// case-sensitive would break every filter anyone had saved. `(?-i)` turns it
/// back off for whoever wants that.
///
/// `regex-lite` rather than `regex`, measured: the full engine takes the
/// stripped release binary from 1.53 MB to 2.93 MB and pulls in four more
/// crates for its SIMD literal search, which matches a few hundred process
/// names once a tick and is not worth 92% of the binary. `regex-lite` costs
/// 70 KB and one crate, and gives up only Unicode character classes.
pub struct Filter(regex_lite::Regex);

impl Filter {
    /// `None` when the pattern does not compile. Every caller decides what
    /// that means for it: a usage error on the command line, a warning for a
    /// stale saved view, and nothing at all mid-keystroke in the TUI.
    pub fn new(pattern: &str) -> Option<Self> {
        regex_lite::Regex::new(&format!("(?i){pattern}"))
            .ok()
            .map(Filter)
    }

    fn is_match(&self, name: &str) -> bool {
        self.0.is_match(name)
    }
}

pub(crate) fn keep_matches<T>(rows: &mut Vec<T>, filter: &Filter, row: impl Fn(&T) -> (u16, &str)) {
    let mut want = 0;
    let mut keep = vec![false; rows.len()];
    for (i, r) in rows.iter().enumerate().rev() {
        let (d, name) = row(r);
        if filter.is_match(name) || d < want {
            keep[i] = true;
            want = d;
        }
    }
    let mut flags = keep.into_iter();
    rows.retain(|_| flags.next() == Some(true));
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
fn layout(cols: &Columns, cell: impl Fn(&Column) -> String) -> String {
    let mut line = String::new();
    for (i, col) in cols.iter().enumerate() {
        if i > 0 {
            line.push(' ');
        }
        let w = usize::from(col.width);
        let text = cell(col);
        // The name column is the only left-aligned one, and it cannot be
        // hidden, so the first rendered column is always it.
        line.push_str(&if i == 0 {
            format!("{text:<w$}")
        } else {
            format!("{text:>w$}")
        });
    }
    line
}

/// # Errors
///
/// Returns an error if writing the table to stdout fails.
/// Keep the `n` heaviest rows under each parent, at every depth.
///
/// A row-level trim, deliberately, and not a tree one. Rows are built from the
/// whole tree first, so a surviving parent still shows the total it was built
/// with rather than the total of what it kept — the same contract `--filter`
/// has, and the reason `Host` does not start claiming the machine holds three
/// applications. Dropping a row drops its subtree with it: a child of a row
/// nobody can see is not a row.
///
/// Generic over the row type for the same reason `keep_matches` is: the TUI
/// trims its flattened rows and `--once` trims the ones it prints, and `--top`
/// has to mean one thing on both.
pub(crate) fn keep_top<T>(rows: &mut Vec<T>, n: usize, row: impl Fn(&T) -> (u16, bool)) {
    if n == 0 || rows.is_empty() {
        return;
    }
    // Siblings kept so far, indexed by depth. Seeing a row at depth `d` means
    // every deeper parent has been left behind, so those counts are discarded
    // and the next child under the new parent starts from zero.
    let mut kept: Vec<usize> = Vec::new();
    let mut pruned_at: Option<u16> = None;
    let mut keep = Vec::with_capacity(rows.len());
    for r in rows.iter() {
        let (d, trimmable) = row(r);
        if let Some(cut) = pruned_at {
            if d > cut {
                keep.push(false);
                continue;
            }
            pruned_at = None;
        }
        let i = usize::from(d);
        if kept.len() <= i {
            kept.resize(i + 1, 0);
        }
        // Moving back up leaves every deeper parent behind, so the next child
        // under the new one starts counting from zero.
        kept.truncate(i + 1);
        if !trimmable {
            keep.push(true);
            continue;
        }
        kept[i] += 1;
        let over = kept[i] > n;
        if over {
            pruned_at = Some(d);
        }
        keep.push(!over);
    }
    let mut flags = keep.into_iter();
    rows.retain(|_| flags.next() == Some(true));
}

/// Drop every User node but the ones named, on every surface.
///
/// A prune, not a `keep_matches` filter, and the two deliberately disagree
/// about what happens to the Host row. `--filter` builds every row from the
/// whole tree and drops rows afterwards, so an ancestor keeps the total it
/// always had: you need that to see what fraction of a browser the matching
/// helper is. This cuts branches out of the tree before any row is built, so
/// the Host row totals what is left — which is what someone who asked for one
/// user wants the top row to mean.
///
/// System and Host-level Containers stay either way. They are the machine's
/// cost and belong to nobody, so hiding them would leave a tree that no longer
/// explains the header above it.
pub fn keep_users(tree: &mut HostTree, uids: &[u32]) {
    if uids.is_empty() {
        return;
    }
    tree.users.retain(|u| uids.contains(&u.uid));
}

pub fn print_table(interval: Duration, view: &View) -> Result<(), Error> {
    let cols = Columns::from_view(view);
    let tree = proc::sample_world(interval);
    render_table(&mut io::stdout(), &tree, view, &cols)
}

/// One table per `--interval`, forever. Each sample reprints its own header
/// rather than repeating one at the top, so any line of the stream still says
/// which machine state it belongs to and a blank line separates the samples.
///
/// # Errors
///
/// Returns an error if a sample cannot be written.
pub fn follow_table(interval: Duration, pss_interval: Duration, view: &View) -> Result<(), Error> {
    let cols = Columns::from_view(view);
    let mut out = io::stdout();
    proc::sample_stream(interval, pss_interval, |tree| {
        render_table(&mut out, tree, view, &cols)?;
        writeln!(out)?;
        out.flush()?;
        Ok(())
    })
}

fn render_table(
    out: &mut impl Write,
    tree: &HostTree,
    view: &View,
    cols: &Columns,
) -> Result<(), Error> {
    let mut tree = tree.clone();
    keep_users(&mut tree, &view.users);
    sort_tree(&mut tree, Sort::from_label(&view.sort), view.desc);
    let mut rows = table_rows(&tree);
    if !view.filter.is_empty() {
        // A saved view must not stop the monitor, so an unusable pattern warns
        // and the table prints unfiltered — the same call `hide_columns` makes,
        // and not `--filter`'s, which is an argument just typed.
        match Filter::new(&view.filter) {
            Some(f) => keep_matches(&mut rows, &f, |r| (r.depth, r.name.as_str())),
            None => eprintln!(
                "heft: ignoring unusable filter {:?} in {}",
                view.filter,
                crate::config::view_path().display()
            ),
        }
    }
    // After the filter, so `--filter chrome --top 3` is the three heaviest
    // rows that match rather than whatever of the top three happened to.
    if let Some(n) = view.top {
        keep_top(&mut rows, n, |r| (r.depth, r.trimmable));
    }
    writeln!(
        out,
        "HOST  cpu {:>5.1}%  usr {:>4.1} sys {:>4.1} wait {:>4.1}  mem {} / {}{}  nproc {}{}",
        tree.cpu_pct,
        tree.cpu_user_pct,
        tree.cpu_system_pct,
        tree.cpu_wait_pct,
        fmt_bytes(Some(tree.mem_used_bytes)),
        fmt_bytes(Some(tree.mem_total_bytes)),
        host_swap(&tree),
        tree.nproc,
        crate::psi::header_tail(&tree)
    )?;
    write_header(out, cols)?;
    write_rows(out, cols, &rows)?;
    Ok(())
}

/// One `--once` line before it is formatted, in the depth-and-name shape
/// `keep_matches` reads, so `--filter` means the same thing here and in the TUI.
struct TableRow {
    depth: u16,
    name: String,
    nproc: u32,
    metrics: Metrics,
    /// False for Host, a User, and a folder header. `--top` never trims those:
    /// they are the shape of the tree rather than entries competing to be
    /// heaviest, and neither Users nor the host-level folders are ordered by
    /// the sort at all, so "the top two" of them would cut whichever ones
    /// `assemble` happened to build first.
    trimmable: bool,
}

fn table_rows(tree: &HostTree) -> Vec<TableRow> {
    let mut rows = vec![TableRow {
        depth: 0,
        name: "Host".into(),
        nproc: tree_host_nproc(tree),
        metrics: host_metrics(tree),
        trimmable: false,
    }];
    for user in &tree.users {
        rows.push(TableRow {
            depth: 1,
            name: format!("{} ({})", user.name, user.uid),
            nproc: user_nproc(user),
            metrics: user_metrics(user),
            trimmable: false,
        });
        push_folder(&mut rows, 2, "Applications", &user.applications);
        push_folder(&mut rows, 2, "User Services", &user.user_services);
        push_folder(&mut rows, 2, "Containers", &user.containers);
    }
    push_folder(&mut rows, 1, "Containers", &tree.containers);
    push_folder(&mut rows, 1, "System", &tree.system);
    rows
}

fn push_folder(rows: &mut Vec<TableRow>, depth: u16, name: &str, idents: &[IdentNode]) {
    rows.push(TableRow {
        depth,
        name: name.into(),
        nproc: folder_nproc(idents),
        metrics: sum_idents(idents),
        trimmable: false,
    });
    for ident in idents {
        rows.push(TableRow {
            depth: depth + 1,
            name: ident.title.clone(),
            nproc: ident.nproc,
            metrics: ident.metrics.clone(),
            trimmable: true,
        });
        for member in &ident.containers {
            rows.push(TableRow {
                depth: depth + 2,
                name: member.title.clone(),
                nproc: member.nproc,
                metrics: member.metrics.clone(),
                trimmable: true,
            });
        }
    }
}

/// `  swap used / total`, or nothing at all on a machine with no swap: an
/// unconditional ` swap 0 / 0` would be a field about a device that is not
/// there.
fn host_swap(tree: &HostTree) -> String {
    if tree.swap_total_bytes == 0 {
        return String::new();
    }
    format!(
        "  swap {} / {}",
        fmt_bytes(Some(tree.swap_used_bytes)),
        fmt_bytes(Some(tree.swap_total_bytes))
    )
}

/// # Errors
///
/// Returns an error if the tree cannot be serialized or stdout cannot be written.
pub fn print_json(interval: Duration, view: &View) -> Result<(), Error> {
    let tree = proc::sample_world(interval);
    let text = json_text(&tree, view, true)?;
    writeln!(io::stdout(), "{text}")?;
    Ok(())
}

/// One JSON document per line per `--interval`, forever: NDJSON, so a reader
/// can take a line at a time without a streaming parser. Compact rather than
/// pretty for the same reason — a document that spans lines is not a record.
///
/// # Errors
///
/// Returns an error if a sample cannot be serialized or written.
pub fn follow_json(interval: Duration, pss_interval: Duration, view: &View) -> Result<(), Error> {
    let mut out = io::stdout();
    proc::sample_stream(interval, pss_interval, |tree| {
        let text = json_text(tree, view, false)?;
        writeln!(out, "{text}")?;
        out.flush()?;
        Ok(())
    })
}

fn json_text(tree: &HostTree, view: &View, pretty: bool) -> Result<String, Error> {
    let mut tree = tree.clone();
    keep_users(&mut tree, &view.users);
    sort_tree(&mut tree, Sort::from_label(&view.sort), view.desc);
    let doc = serde_json::json!({ "host": tree });
    // `println!` panics when the reader closes, and the release profile is
    // `panic = abort`, so `heft --json | head` would abort. Serialize first,
    // then write through `io::Write`: serializing into the stream instead
    // would bury the EPIPE inside a `serde_json::Error`, which `main` cannot
    // recognise as a closed pipe.
    Ok(if pretty {
        serde_json::to_string_pretty(&doc)?
    } else {
        serde_json::to_string(&doc)?
    })
}

fn write_header(out: &mut impl Write, cols: &Columns) -> io::Result<()> {
    writeln!(out, "{}", layout(cols, |c| c.header.to_string()))
}

fn write_rows(out: &mut impl Write, cols: &Columns, rows: &[TableRow]) -> io::Result<()> {
    for r in rows {
        let indent = "  ".repeat(usize::from(r.depth));
        // The name column pads but never clips, so the label is cut to fit
        // first.
        let label = trunc(
            &format!("{indent}{}", r.name),
            usize::from(COLUMNS[0].width),
        );
        writeln!(
            out,
            "{}",
            layout(cols, |c| (c.fmt)(&label, r.nproc, &r.metrics))
        )?;
    }
    Ok(())
}

fn trunc(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push(crate::glyph::ellipsis());
    out
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
/// One unit, largest that fits: `45s`, `12m`, `3h`, `9d`. A duration is read
/// at a glance to place a process in time, so the coarse unit is the whole
/// point — `2d` beats `191243s` and beats a start timestamp, which would make
/// the reader do the subtraction.
pub(crate) fn fmt_age(secs: Option<u64>) -> String {
    let Some(s) = secs else {
        return String::new();
    };
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    if s >= DAY {
        format!("{}d", s / DAY)
    } else if s >= HOUR {
        format!("{}h", s / HOUR)
    } else if s >= MIN {
        format!("{}m", s / MIN)
    } else {
        format!("{s}s")
    }
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
    #[test]
    fn keep_users_prunes_users_and_spares_the_machine() {
        let mut tree = ordering_tree();
        tree.users.push(UserNode {
            uid: 0,
            name: "root".into(),
            applications: Vec::new(),
            user_services: Vec::new(),
            containers: Vec::new(),
        });
        tree.system = vec![IdentNode {
            id: "kthread".into(),
            title: "kthread".into(),
            nproc: 1,
            metrics: metrics(Some(1), 0.0),
            instances: Vec::new(),
            containers: Vec::new(),
        }];

        // An empty list is "no --user was given", never "keep nobody".
        let mut untouched = tree.clone();
        keep_users(&mut untouched, &[]);
        assert_eq!(untouched.users.len(), 2);

        keep_users(&mut tree, &[1000]);
        assert_eq!(
            tree.users.iter().map(|u| u.uid).collect::<Vec<_>>(),
            vec![1000]
        );
        assert_eq!(tree.system.len(), 1, "System is nobody's and always stays");
    }

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

    fn every_column() -> Columns {
        Columns::from_view(&View::default())
    }

    #[test]
    fn sort_next_cycles_every_variant() {
        let all = Sort::all();
        let cols = every_column();
        let mut s = all[0];
        let mut seen = Vec::new();
        for _ in 0..all.len() {
            s = s.next(&cols);
            seen.push(s);
        }
        let mut expect: Vec<Sort> = all[1..].to_vec();
        expect.push(all[0]);
        assert_eq!(seen, expect);
    }

    fn labels(cols: &Columns) -> Vec<&'static str> {
        cols.iter().map(|c| c.label).collect()
    }

    fn all_labels() -> Vec<&'static str> {
        COLUMNS.iter().map(|c| c.label).collect()
    }

    fn hiding(hide: &[&str]) -> Columns {
        Columns::from_view(&View {
            hide_columns: hide.iter().map(|s| (*s).to_string()).collect(),
            ..View::default()
        })
    }

    /// A default view shows the whole set, a stale or malicious entry cannot
    /// take the labels away, and an unknown one leaves the table alone.
    #[test]
    fn hidden_columns_leave_the_rest_in_table_order() {
        assert_eq!(labels(&every_column()), all_labels());
        assert_eq!(
            labels(&hiding(&["gtt", "vram", "netns_rx", "netns_tx"])),
            [
                "name", "nproc", "threads", "age", "core", "machine", "pss", "rss", "swap",
                "diskr", "diskw", "gfx", "compute", "cpustall", "iostall", "memstall"
            ]
        );
        assert_eq!(labels(&hiding(&["name"])), all_labels());
        assert_eq!(labels(&hiding(&["cpu", ""])), all_labels());
    }

    /// Hiding is presentation: the cells disappear, the widths of what is left
    /// do not move, and nothing about the metrics behind them changes.
    #[test]
    fn hiding_a_column_only_removes_its_cells() {
        let mut out = Vec::new();
        let cols = hiding(&[
            "nproc", "threads", "age", "diskr", "diskw", "gfx", "compute", "cpustall", "iostall",
            "memstall", "netns_rx", "netns_tx",
        ]);
        write_header(&mut out, &cols).unwrap();
        write_rows(&mut out, &cols, &folder_rows(1536.0)).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "NAME                           %CORE   %MACH      PSS      RSS   SWAP     VRAM      GTT\n  Applications                  12.2     1.5     1.5K     2.0K            1.0M         \n    an-identity-name-long-e\u{2026}    12.2     1.5     1.5K     2.0K            1.0M         \n"
        );
    }

    /// The cycle has to skip what it cannot show, or `c` moves the sort to a
    /// column the reader has no way to see.
    #[test]
    fn sort_cycle_skips_hidden_columns() {
        let cols = hiding(&["nproc", "threads"]);
        assert_eq!(Sort::from_label("name").next(&cols).label(), "age");
        // A saved view may sort by a column it also hides; the cycle restarts
        // rather than stalling on it.
        assert_eq!(Sort::from_label("nproc").next(&cols).label(), "name");
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
                // A swapless host: the whole column is blank, not a column of
                // zeros, which is the layout most machines print.
                swap_bytes: None,
                threads: Some(19),
                age_secs: Some(3 * 3600 + 12),
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
                // One cgroup, so this row does carry a stall — the mixed case
                // (a figure here, blanks in the pair above) is what the width
                // and truncation assertions need to see.
                cpu_stall_pct: Some(0.5),
                io_stall_pct: Some(12.0),
                mem_stall_pct: None,
            },
            instances: Vec::new(),
            containers: Vec::new(),
        }
    }

    /// `--once` shares the TUI's filter, so the table keeps the ancestors of a
    /// match and drops the siblings — and an ancestor keeps the total it was
    /// built with, not the total of whatever survived, exactly as the TUI's
    /// Host line does under `/`.
    #[test]
    fn once_filter_keeps_ancestors_and_their_totals() {
        let named = |title: &str| IdentNode {
            title: title.into(),
            ..sample_ident(1536.0)
        };
        let tree = HostTree {
            users: vec![UserNode {
                uid: 1000,
                name: "u".into(),
                applications: vec![named("firefox"), named("vim")],
                user_services: Vec::new(),
                containers: Vec::new(),
            }],
            ..HostTree::default()
        };
        let mut rows = table_rows(&tree);
        assert_eq!(rows[0].metrics.pss_bytes, Some(3072));
        let filter = Filter::new("firefox").expect("test patterns compile");
        keep_matches(&mut rows, &filter, |r| (r.depth, r.name.as_str()));
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Host", "u (1000)", "Applications", "firefox"]);
        assert_eq!(rows[0].metrics.pss_bytes, Some(3072));
    }

    /// `--top` counts per parent, not per depth: two folders each keep their
    /// own N. Structural rows never count and are never cut, since Users and
    /// the host-level folders are not ordered by the sort at all, so "the top
    /// two" of them would drop whichever ones came last for no reason a reader
    /// could see — losing the whole System section to a machine that happened
    /// to have two users.
    #[test]
    fn top_counts_siblings_under_each_parent_and_spares_the_shape() {
        let row = |depth: u16, name: &str, trimmable: bool| TableRow {
            depth,
            name: name.into(),
            nproc: 1,
            metrics: Metrics::default(),
            trimmable,
        };
        let mut rows = vec![
            row(0, "Host", false),
            row(1, "alice", false),
            row(2, "Applications", false),
            row(3, "a1", true),
            row(3, "a2", true),
            row(3, "a3", true),
            row(2, "User Services", false),
            row(3, "s1", true),
            row(3, "s2", true),
            row(1, "System", false),
            row(2, "k1", true),
            row(2, "k2", true),
        ];
        keep_top(&mut rows, 2, |r| (r.depth, r.trimmable));
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            [
                "Host",
                "alice",
                "Applications",
                "a1",
                "a2",
                "User Services",
                "s1",
                "s2",
                "System",
                "k1",
                "k2"
            ]
        );
    }

    /// A child of a row nobody can see is not a row.
    #[test]
    fn top_drops_the_subtree_of_a_row_it_cut() {
        let row = |depth: u16, name: &str, trimmable: bool| TableRow {
            depth,
            name: name.into(),
            nproc: 1,
            metrics: Metrics::default(),
            trimmable,
        };
        let mut rows = vec![
            row(0, "Host", false),
            row(1, "kept", true),
            row(2, "kept-child", true),
            row(1, "cut", true),
            row(2, "cut-child", true),
            row(3, "cut-grandchild", true),
        ];
        keep_top(&mut rows, 1, |r| (r.depth, r.trimmable));
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["Host", "kept", "kept-child"]
        );
    }

    fn folder_rows(disk_r_bps: f64) -> Vec<TableRow> {
        let mut rows = Vec::new();
        push_folder(&mut rows, 1, "Applications", &[sample_ident(disk_r_bps)]);
        rows
    }

    /// `--once` is a fixed-width table other tools slice by column, so the
    /// exact byte layout is the contract, not just the values. The second case
    /// is a cell wider than its column: it pushes the line out rather than
    /// clipping, and downstream slicing has always had to cope with that.
    #[test]
    fn once_rows_keep_their_byte_layout() {
        let mut out = Vec::new();
        let cols = every_column();
        write_header(&mut out, &cols).unwrap();
        write_rows(&mut out, &cols, &folder_rows(1536.0)).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "NAME                            N   THR   AGE   %CORE   %MACH      PSS      RSS   SWAP   DISK R   DISK W     VRAM      GTT   GFX   CMP CPU ST  IO ST MEM ST NETNS RX NETNS TX\n  Applications                  7    19    3h    12.2     1.5     1.5K     2.0K          1.5K/s              1.0M            3.0                                             \n    an-identity-name-long-e\u{2026}    7    19    3h    12.2     1.5     1.5K     2.0K          1.5K/s              1.0M            3.0          0.5   12.0                         \n"
        );

        let mut out = Vec::new();
        write_rows(&mut out, &cols, &folder_rows(1_030_963.0)).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "  Applications                  7    19    3h    12.2     1.5     1.5K     2.0K        1006.8K/s              1.0M            3.0                                             \n    an-identity-name-long-e\u{2026}    7    19    3h    12.2     1.5     1.5K     2.0K        1006.8K/s              1.0M            3.0          0.5   12.0                         \n"
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

    #[test]
    fn age_takes_the_largest_unit_that_fits() {
        assert_eq!(fmt_age(None), "");
        assert_eq!(fmt_age(Some(0)), "0s");
        assert_eq!(fmt_age(Some(59)), "59s");
        assert_eq!(fmt_age(Some(60)), "1m");
        assert_eq!(fmt_age(Some(3599)), "59m");
        assert_eq!(fmt_age(Some(3600)), "1h");
        assert_eq!(fmt_age(Some(86_399)), "23h");
        assert_eq!(fmt_age(Some(86_400)), "1d");
        assert_eq!(fmt_age(Some(191_243)), "2d");
    }

    /// An identity row's age is the oldest process on it, and its threads are
    /// every process's threads. Age must not behave like the summing columns
    /// beside it: two hour-old processes make an hour-old row, not two.
    #[test]
    fn threads_sum_but_age_takes_the_oldest() {
        let row = |threads, age| Metrics {
            threads: Some(threads),
            age_secs: Some(age),
            pss_bytes: Some(10),
            ..Metrics::default()
        };
        let mut m = row(4, 3600);
        m.accumulate(&row(6, 3600));
        assert_eq!(m.threads, Some(10));
        assert_eq!(m.age_secs, Some(3600));

        m.accumulate(&row(1, 90_000));
        assert_eq!(m.threads, Some(11));
        assert_eq!(m.age_secs, Some(90_000));

        // A blank on either side stays out of the way of a real figure.
        let mut blank = Metrics::default();
        blank.accumulate(&row(3, 42));
        assert_eq!((blank.threads, blank.age_secs), (Some(3), Some(42)));
        blank.accumulate(&Metrics::default());
        assert_eq!((blank.threads, blank.age_secs), (Some(3), Some(42)));
    }
}
