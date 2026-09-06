use std::io::{self, Write};
use std::time::Duration;

use crate::proc;
use crate::types::{
    Error, IdentNode, Metrics, folder_nproc, host_metrics, sum_idents, sum_lists, tree_host_nproc,
    user_nproc,
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
    pub(crate) key: Option<fn(&IdentNode) -> f64>,
}

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
        key: Some(|i| f64::from(i.nproc)),
    },
    Column {
        label: "core",
        header: "%CORE",
        width: 7,
        fmt: |_, _, m| fmt_pct(m.cpu_core_pct),
        key: Some(|i| i.metrics.cpu_core_pct),
    },
    Column {
        label: "machine",
        header: "%MACH",
        width: 7,
        fmt: |_, _, m| fmt_pct(m.cpu_machine_pct),
        key: Some(|i| i.metrics.cpu_machine_pct),
    },
    Column {
        label: "pss",
        header: "PSS",
        width: 8,
        fmt: |_, _, m| fmt_bytes(m.pss_bytes),
        key: Some(|i| opt_u(i.metrics.pss_bytes)),
    },
    Column {
        label: "rss",
        header: "RSS",
        width: 8,
        fmt: |_, _, m| fmt_bytes(m.rss_bytes),
        key: Some(|i| opt_u(i.metrics.rss_bytes)),
    },
    Column {
        label: "diskr",
        header: "DISK R",
        width: 8,
        fmt: |_, _, m| fmt_rate(m.disk_r_bps),
        key: Some(|i| i.metrics.disk_r_bps.unwrap_or(0.0)),
    },
    Column {
        label: "diskw",
        header: "DISK W",
        width: 8,
        fmt: |_, _, m| fmt_rate(m.disk_w_bps),
        key: Some(|i| i.metrics.disk_w_bps.unwrap_or(0.0)),
    },
    Column {
        label: "vram",
        header: "VRAM",
        width: 8,
        fmt: |_, _, m| fmt_bytes(m.vram_bytes),
        key: Some(|i| opt_u(i.metrics.vram_bytes)),
    },
    Column {
        label: "gtt",
        header: "GTT",
        width: 8,
        fmt: |_, _, m| fmt_bytes(m.gtt_bytes),
        key: Some(|i| opt_u(i.metrics.gtt_bytes)),
    },
    Column {
        label: "gfx",
        header: "GFX",
        width: 5,
        fmt: |_, _, m| fmt_opt_pct(m.gfx_pct),
        key: Some(|i| i.metrics.gfx_pct.unwrap_or(0.0)),
    },
    Column {
        label: "compute",
        header: "CMP",
        width: 5,
        fmt: |_, _, m| fmt_opt_pct(m.compute_pct),
        key: Some(|i| i.metrics.compute_pct.unwrap_or(0.0)),
    },
];

/// Byte counts stay exact in f64 out past 9 PB, so one key type covers the
/// integer and rate columns alike.
fn opt_u(v: Option<u64>) -> f64 {
    v.unwrap_or(0) as f64
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
            &sum_lists(&[&user.applications, &user.user_services, &user.containers]),
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
    println!("{}", serde_json::to_string_pretty(&doc)?);
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
            "NAME                            N   %CORE   %MACH      PSS      RSS   DISK R   DISK W     VRAM      GTT   GFX   CMP\n  Applications                  7    12.2     1.5     1.5K     2.0K   1.5K/s              1.0M            3.0      \n    an-identity-name-long-e\u{2026}    7    12.2     1.5     1.5K     2.0K   1.5K/s              1.0M            3.0      \n"
        );

        let mut out = Vec::new();
        write_folder(&mut out, 1, "Applications", &[sample_ident(1_030_963.0)]).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "  Applications                  7    12.2     1.5     1.5K     2.0K 1006.8K/s              1.0M            3.0      \n    an-identity-name-long-e\u{2026}    7    12.2     1.5     1.5K     2.0K 1006.8K/s              1.0M            3.0      \n"
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
