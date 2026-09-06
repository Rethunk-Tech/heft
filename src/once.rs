use std::io::{self, Write};
use std::time::Duration;

use crate::proc;
use crate::types::{
    Error, IdentNode, Metrics, folder_nproc, host_metrics, sum_idents, sum_lists, tree_host_nproc,
    user_nproc,
};

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
    writeln!(
        out,
        "{:<28} {:>4} {:>7} {:>7} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>5} {:>5}",
        "NAME",
        "N",
        "%CORE",
        "%MACH",
        "PSS",
        "RSS",
        "DISK R",
        "DISK W",
        "VRAM",
        "GTT",
        "GFX",
        "CMP"
    )?;
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
            &sum_lists([&user.applications, &user.user_services, &user.containers]),
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
    let label = format!("{indent}{name}");
    writeln!(
        out,
        "{:<28} {:>4} {:>7} {:>7} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>5} {:>5}",
        trunc(&label, 28),
        n,
        fmt_pct(m.cpu_core_pct),
        fmt_pct(m.cpu_machine_pct),
        fmt_bytes(m.pss_bytes),
        fmt_bytes(m.rss_bytes),
        fmt_rate(m.disk_r_bps),
        fmt_rate(m.disk_w_bps),
        fmt_bytes(m.vram_bytes),
        fmt_bytes(m.gtt_bytes),
        fmt_opt_pct(m.gfx_pct),
        fmt_opt_pct(m.compute_pct),
    )
}

fn trunc(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    s.chars().take(width.saturating_sub(1)).collect::<String>() + "…"
}

pub fn fmt_bytes(n: Option<u64>) -> String {
    let Some(b) = n else {
        return String::new();
    };
    const K: f64 = 1024.0;
    let x = b as f64;
    if x >= K * K * K {
        format!("{:.1}G", x / (K * K * K))
    } else if x >= K * K {
        format!("{:.1}M", x / (K * K))
    } else if x >= K {
        format!("{:.1}K", x / K)
    } else {
        format!("{b}")
    }
}

pub fn fmt_rate(n: Option<f64>) -> String {
    n.map(|v| {
        if v <= 0.0 {
            "0".into()
        } else {
            format!("{}/s", fmt_bytes(Some(v as u64)))
        }
    })
    .unwrap_or_default()
}

pub fn fmt_pct(v: f64) -> String {
    format!("{v:.1}")
}

pub fn fmt_opt_pct(v: Option<f64>) -> String {
    v.map(fmt_pct).unwrap_or_default()
}
