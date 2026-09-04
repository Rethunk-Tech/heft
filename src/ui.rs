use std::collections::HashSet;
use std::io::{self, stdout};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::config::{self, View};
use crate::cpu;
use crate::once::{fmt_bytes, fmt_opt_pct, fmt_pct, fmt_rate};
use crate::proc;
use crate::types::{Error, HostTree, IdentNode, Metrics, ProcNode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sort {
    Name,
    Nproc,
    Core,
    Machine,
    Pss,
    Rss,
    DiskR,
    DiskW,
    Vram,
    Gtt,
    Gfx,
    Compute,
}

impl Sort {
    fn all() -> [Sort; 12] {
        [
            Sort::Name,
            Sort::Nproc,
            Sort::Core,
            Sort::Machine,
            Sort::Pss,
            Sort::Rss,
            Sort::DiskR,
            Sort::DiskW,
            Sort::Vram,
            Sort::Gtt,
            Sort::Gfx,
            Sort::Compute,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::Nproc => "nproc",
            Sort::Core => "core",
            Sort::Machine => "machine",
            Sort::Pss => "pss",
            Sort::Rss => "rss",
            Sort::DiskR => "diskr",
            Sort::DiskW => "diskw",
            Sort::Vram => "vram",
            Sort::Gtt => "gtt",
            Sort::Gfx => "gfx",
            Sort::Compute => "compute",
        }
    }

    fn from_label(s: &str) -> Self {
        Self::all()
            .into_iter()
            .find(|x| x.label() == s)
            .unwrap_or(Sort::Machine)
    }

    fn next(self) -> Self {
        let all = Self::all();
        let i = all.iter().position(|x| *x == self).unwrap_or(0);
        all[(i + 1) % all.len()]
    }
}

struct App {
    tree: HostTree,
    interval: Duration,
    last: Instant,
    cursor: usize,
    expand: HashSet<String>,
    view: View,
    filter_edit: bool,
    col_off: u16,
    status: String,
}

pub fn run(interval: Duration) -> Result<(), Error> {
    config::ensure_dirs();
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, Hide)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, interval);
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, Show)?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    interval: Duration,
) -> Result<(), Error> {
    let view = config::load_view();
    let tree = proc::sample_world(interval);
    let mut app = App {
        expand: default_expand(&tree),
        tree,
        interval,
        last: Instant::now(),
        cursor: 0,
        view,
        filter_edit: false,
        col_off: 0,
        status: String::new(),
    };
    loop {
        let rows = flatten(&app.tree, &app.expand, &app.view);
        if app.cursor >= rows.len() {
            app.cursor = rows.len().saturating_sub(1);
        }
        terminal.draw(|f| draw(f, &app, &rows))?;
        let timeout = app.interval.saturating_sub(app.last.elapsed());
        if event::poll(timeout.max(Duration::from_millis(20)))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if handle_key(&mut app, k.code, k.modifiers, &rows)? {
                        break;
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        if app.last.elapsed() >= app.interval {
            app.tree = proc::sample_world(app.interval);
            app.last = Instant::now();
        }
    }
    Ok(())
}

fn default_expand(tree: &HostTree) -> HashSet<String> {
    let mut s = HashSet::new();
    s.insert("host".into());
    let me = cpu::euid();
    s.insert(format!("user:{me}"));
    s.insert(format!("user:{me}/apps"));
    s.insert(format!("user:{me}/containers"));
    if tree.users.iter().any(|u| u.uid == me) {
        // keep defaults
    }
    s
}

struct Flat {
    id: String,
    depth: u16,
    name: String,
    nproc: u32,
    metrics: Metrics,
    expandable: bool,
}

fn flatten(tree: &HostTree, expand: &HashSet<String>, view: &View) -> Vec<Flat> {
    let filter = view.filter.to_ascii_lowercase();
    let sort = Sort::from_label(&view.sort);
    let mut rows = Vec::new();
    let host_n = tree.users.iter().map(user_n).sum::<u32>()
        + folder_n(&tree.containers)
        + folder_n(&tree.system);
    rows.push(Flat {
        id: "host".into(),
        depth: 0,
        name: "Host".into(),
        nproc: host_n,
        metrics: host_m(tree),
        expandable: true,
    });
    if expand.contains("host") {
        for user in &tree.users {
            let uid = user.uid;
            let id = format!("user:{uid}");
            push_maybe(
                &mut rows,
                Flat {
                    id: id.clone(),
                    depth: 1,
                    name: format!("{} ({uid})", user.name),
                    nproc: user_n(user),
                    metrics: sum_lists([&user.applications, &user.user_services, &user.containers]),
                    expandable: true,
                },
                &filter,
            );
            if expand.contains(&id) {
                push_folder(
                    &mut rows,
                    FolderPush {
                        expand,
                        sort,
                        desc: view.desc,
                        depth: 2,
                        id: &format!("user:{uid}/apps"),
                        title: "Applications",
                        idents: &user.applications,
                    },
                );
                push_folder(
                    &mut rows,
                    FolderPush {
                        expand,
                        sort,
                        desc: view.desc,
                        depth: 2,
                        id: &format!("user:{uid}/services"),
                        title: "User Services",
                        idents: &user.user_services,
                    },
                );
                push_folder(
                    &mut rows,
                    FolderPush {
                        expand,
                        sort,
                        desc: view.desc,
                        depth: 2,
                        id: &format!("user:{uid}/containers"),
                        title: "Containers",
                        idents: &user.containers,
                    },
                );
            }
        }
        push_folder(
            &mut rows,
            FolderPush {
                expand,
                sort,
                desc: view.desc,
                depth: 1,
                id: "host/containers",
                title: "Containers",
                idents: &tree.containers,
            },
        );
        push_folder(
            &mut rows,
            FolderPush {
                expand,
                sort,
                desc: view.desc,
                depth: 1,
                id: "host/system",
                title: "System",
                idents: &tree.system,
            },
        );
    }
    if !filter.is_empty() {
        keep_matches(&mut rows, &filter);
    }
    rows
}

fn push_maybe(rows: &mut Vec<Flat>, row: Flat, filter: &str) {
    let _ = filter;
    rows.push(row);
}

fn keep_matches(rows: &mut Vec<Flat>, filter: &str) {
    let mut keep = vec![false; rows.len()];
    for i in 0..rows.len() {
        if rows[i].name.to_ascii_lowercase().contains(filter) {
            keep[i] = true;
            let d = rows[i].depth;
            let mut j = i;
            while j > 0 {
                j -= 1;
                if rows[j].depth < d {
                    keep[j] = true;
                    let d2 = rows[j].depth;
                    if d2 == 0 {
                        break;
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for (i, row) in rows.drain(..).enumerate() {
        if keep[i] {
            out.push(row);
        }
    }
    *rows = out;
}

struct FolderPush<'a> {
    expand: &'a HashSet<String>,
    sort: Sort,
    desc: bool,
    depth: u16,
    id: &'a str,
    title: &'a str,
    idents: &'a [IdentNode],
}

fn push_folder(rows: &mut Vec<Flat>, p: FolderPush<'_>) {
    rows.push(Flat {
        id: p.id.to_string(),
        depth: p.depth,
        name: p.title.to_string(),
        nproc: folder_n(p.idents),
        metrics: sum_idents(p.idents),
        expandable: true,
    });
    if !p.expand.contains(p.id) {
        return;
    }
    let mut ordered: Vec<&IdentNode> = p.idents.iter().collect();
    ordered.sort_by(|a, b| cmp_ident(a, b, p.sort, p.desc));
    for ident in ordered {
        let iid = format!("{}/{}", p.id, ident.id);
        rows.push(Flat {
            id: iid.clone(),
            depth: p.depth + 1,
            name: ident.title.clone(),
            nproc: ident.nproc,
            metrics: ident.metrics.clone(),
            expandable: true,
        });
        if !p.expand.contains(&iid) {
            continue;
        }
        for member in &ident.containers {
            let mid = format!("{iid}/m/{}", member.id);
            rows.push(Flat {
                id: mid.clone(),
                depth: p.depth + 2,
                name: member.title.clone(),
                nproc: member.nproc,
                metrics: member.metrics.clone(),
                expandable: true,
            });
            if p.expand.contains(&mid) {
                push_procs(rows, p.expand, p.depth + 3, &mid, &member.processes);
            }
        }
        for inst in &ident.instances {
            let sid = format!("{iid}/i/{}", inst.key);
            rows.push(Flat {
                id: sid.clone(),
                depth: p.depth + 2,
                name: inst.key.clone(),
                nproc: inst.nproc,
                metrics: inst.metrics.clone(),
                expandable: true,
            });
            if p.expand.contains(&sid) {
                push_procs(rows, p.expand, p.depth + 3, &sid, &inst.processes);
            }
        }
    }
}

fn push_procs(
    rows: &mut Vec<Flat>,
    expand: &HashSet<String>,
    depth: u16,
    prefix: &str,
    procs: &[ProcNode],
) {
    for p in procs {
        let id = format!("{prefix}/p/{}", p.pid);
        rows.push(Flat {
            id: id.clone(),
            depth,
            name: format!("{} [{}]", p.name, p.pid),
            nproc: 1,
            metrics: p.metrics.clone(),
            expandable: !p.children.is_empty(),
        });
        if expand.contains(&id) {
            push_procs(rows, expand, depth + 1, &id, &p.children);
        }
    }
}

fn cmp_ident(a: &IdentNode, b: &IdentNode, sort: Sort, desc: bool) -> std::cmp::Ordering {
    let ord = match sort {
        Sort::Name => a.title.cmp(&b.title),
        Sort::Nproc => a.nproc.cmp(&b.nproc),
        Sort::Core => a.metrics.cpu_core_pct.total_cmp(&b.metrics.cpu_core_pct),
        Sort::Machine => a
            .metrics
            .cpu_machine_pct
            .total_cmp(&b.metrics.cpu_machine_pct),
        Sort::Pss => opt_u(a.metrics.pss_bytes).cmp(&opt_u(b.metrics.pss_bytes)),
        Sort::Rss => opt_u(a.metrics.rss_bytes).cmp(&opt_u(b.metrics.rss_bytes)),
        Sort::DiskR => opt_f(a.metrics.disk_r_bps).total_cmp(&opt_f(b.metrics.disk_r_bps)),
        Sort::DiskW => opt_f(a.metrics.disk_w_bps).total_cmp(&opt_f(b.metrics.disk_w_bps)),
        Sort::Vram => opt_u(a.metrics.vram_bytes).cmp(&opt_u(b.metrics.vram_bytes)),
        Sort::Gtt => opt_u(a.metrics.gtt_bytes).cmp(&opt_u(b.metrics.gtt_bytes)),
        Sort::Gfx => opt_f(a.metrics.gfx_pct).total_cmp(&opt_f(b.metrics.gfx_pct)),
        Sort::Compute => opt_f(a.metrics.compute_pct).total_cmp(&opt_f(b.metrics.compute_pct)),
    };
    let reverse = if sort == Sort::Name { !desc } else { desc };
    if reverse { ord.reverse() } else { ord }
}

fn opt_u(v: Option<u64>) -> u64 {
    v.unwrap_or(0)
}
fn opt_f(v: Option<f64>) -> f64 {
    v.unwrap_or(0.0)
}

fn handle_key(
    app: &mut App,
    code: KeyCode,
    mods: KeyModifiers,
    rows: &[Flat],
) -> Result<bool, Error> {
    if app.filter_edit {
        match code {
            KeyCode::Esc => {
                app.filter_edit = false;
                app.view.filter.clear();
            }
            KeyCode::Enter => app.filter_edit = false,
            KeyCode::Backspace => {
                app.view.filter.pop();
            }
            KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => {
                app.view.filter.push(c);
            }
            _ => {}
        }
        return Ok(false);
    }
    match code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Esc => return Ok(true),
        KeyCode::Char('/') => app.filter_edit = true,
        KeyCode::Char('s') | KeyCode::Char('S') => {
            config::save_view(&app.view)?;
            app.status = format!("saved {}", config::view_path().display());
        }
        KeyCode::Char('c') => {
            let next = Sort::from_label(&app.view.sort).next();
            app.view.sort = next.label().into();
        }
        KeyCode::Char('d') => app.view.desc = !app.view.desc,
        KeyCode::Up | KeyCode::Char('k') => app.cursor = app.cursor.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            if app.cursor + 1 < rows.len() {
                app.cursor += 1;
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if let Some(r) = rows.get(app.cursor) {
                app.expand.remove(&r.id);
            }
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(r) = rows.get(app.cursor)
                && r.expandable
                && !app.expand.insert(r.id.clone())
            {
                app.expand.remove(&r.id);
            }
        }
        KeyCode::PageUp => app.cursor = app.cursor.saturating_sub(20),
        KeyCode::PageDown => app.cursor = (app.cursor + 20).min(rows.len().saturating_sub(1)),
        KeyCode::Home => app.cursor = 0,
        KeyCode::End => app.cursor = rows.len().saturating_sub(1),
        KeyCode::Char('[') | KeyCode::Char('<') => app.col_off = app.col_off.saturating_sub(1),
        KeyCode::Char(']') | KeyCode::Char('>') => app.col_off = app.col_off.saturating_add(1),
        _ => {}
    }
    Ok(false)
}

fn draw(f: &mut ratatui::Frame<'_>, app: &App, rows: &[Flat]) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .split(f.area());
    let header = Line::from(vec![Span::raw(format!(
        " CPU {:>5}   RAM {} / {}   VRAM {} / {}   nproc {} ",
        fmt_pct(app.tree.cpu_pct),
        fmt_bytes(Some(app.tree.mem_used_bytes)),
        fmt_bytes(Some(app.tree.mem_total_bytes)),
        fmt_bytes(app.tree.vram_used_bytes),
        fmt_bytes(app.tree.vram_total_bytes),
        app.tree.nproc
    ))]);
    f.render_widget(
        Paragraph::new(header).block(Block::default().borders(Borders::ALL).title("heft")),
        chunks[0],
    );

    let headers = [
        "NAME", "N", "%CORE", "%MACH", "PSS", "RSS", "DISK R", "DISK W", "VRAM", "GTT", "GFX",
        "CMP",
    ];
    let skip = app.col_off as usize;
    let shown: Vec<&str> = headers
        .iter()
        .copied()
        .skip(skip.min(headers.len().saturating_sub(1)))
        .collect();
    let mut table_rows = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let mark = if r.expandable {
            if app.expand.contains(&r.id) {
                "▼ "
            } else {
                "▶ "
            }
        } else {
            "  "
        };
        let name = format!("{}{}{}", "  ".repeat(r.depth as usize), mark, r.name);
        let cells = [
            name,
            r.nproc.to_string(),
            fmt_pct(r.metrics.cpu_core_pct),
            fmt_pct(r.metrics.cpu_machine_pct),
            fmt_bytes(r.metrics.pss_bytes),
            fmt_bytes(r.metrics.rss_bytes),
            fmt_rate(r.metrics.disk_r_bps),
            fmt_rate(r.metrics.disk_w_bps),
            fmt_bytes(r.metrics.vram_bytes),
            fmt_bytes(r.metrics.gtt_bytes),
            fmt_opt_pct(r.metrics.gfx_pct),
            fmt_opt_pct(r.metrics.compute_pct),
        ];
        let shown_cells: Vec<String> = cells.into_iter().skip(skip.min(11)).collect();
        let row = Row::new(shown_cells);
        table_rows.push(if i == app.cursor {
            row.style(Style::default().add_modifier(Modifier::REVERSED))
        } else {
            row
        });
    }
    let widths: Vec<Constraint> = if skip == 0 {
        vec![
            Constraint::Min(28),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(5),
            Constraint::Length(5),
        ]
    } else {
        shown.iter().map(|_| Constraint::Length(8)).collect()
    };
    let table = Table::new(table_rows, widths)
        .header(
            Row::new(shown.iter().map(|s| (*s).to_string()))
                .style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(table, chunks[1]);

    let filter = if app.filter_edit {
        format!("filter> {}_", app.view.filter)
    } else if app.view.filter.is_empty() {
        String::new()
    } else {
        format!("filter: {}", app.view.filter)
    };
    let footer = format!(
        " q quit  / filter  c sort ({})  s save  [ ] scroll  {}  {}",
        app.view.sort, filter, app.status
    );
    f.render_widget(Paragraph::new(footer), chunks[2]);
}

fn folder_n(idents: &[IdentNode]) -> u32 {
    idents.iter().map(|i| i.nproc).sum()
}
fn sum_idents(idents: &[IdentNode]) -> Metrics {
    let mut m = Metrics::default();
    for i in idents {
        m.accumulate(&i.metrics);
    }
    m
}
fn sum_lists<const N: usize>(lists: [&[IdentNode]; N]) -> Metrics {
    let mut m = Metrics::default();
    for list in lists {
        m.accumulate(&sum_idents(list));
    }
    m
}
fn user_n(user: &crate::types::UserNode) -> u32 {
    folder_n(&user.applications) + folder_n(&user.user_services) + folder_n(&user.containers)
}
fn host_m(tree: &HostTree) -> Metrics {
    let mut m = sum_lists([&tree.containers, &tree.system]);
    for u in &tree.users {
        m.accumulate(&sum_lists([
            &u.applications,
            &u.user_services,
            &u.containers,
        ]));
    }
    m
}
