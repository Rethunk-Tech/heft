use std::collections::HashSet;
use std::io::{self, stdout};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table};

use crate::config::{self, View};
use crate::cpu;
use crate::mem::{self, MemParts};
use crate::once::{COLUMNS, fmt_bytes, fmt_pct};
use crate::proc;
use crate::types::{
    Error, HostTree, IdentNode, Metrics, ProcNode, folder_nproc, host_metrics, sum_idents,
    tree_host_nproc, user_metrics, user_nproc,
};

const HEADER_ROWS: u16 = 2;

/// Index into `once::COLUMNS`; the saved view stores that column's label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sort(usize);

impl Sort {
    /// Only the tests enumerate the sorts; `next` and `from_label` index
    /// `COLUMNS` directly.
    #[cfg(test)]
    fn all() -> Vec<Sort> {
        (0..COLUMNS.len()).map(Sort).collect()
    }

    fn label(self) -> &'static str {
        COLUMNS[self.0].label
    }

    fn from_label(s: &str) -> Self {
        let find = |l: &str| COLUMNS.iter().position(|c| c.label == l);
        // An unknown label means a saved view from another column set; PSS is
        // the documented default.
        Sort(find(s).or_else(|| find("pss")).unwrap_or(0))
    }

    fn next(self) -> Self {
        Sort((self.0 + 1) % COLUMNS.len())
    }
}

struct App {
    tree: HostTree,
    cursor: usize,
    row_off: usize,
    row_vis: usize,
    expand: HashSet<String>,
    view: View,
    filter_edit: bool,
    col_off: u16,
    status: String,
    help: bool,
}

/// # Errors
///
/// Returns an error if the terminal cannot enter or leave raw mode, the sampler
/// thread cannot be spawned, a frame cannot be drawn, or a view save fails.
pub fn run(interval: Duration, pss_interval: Duration) -> Result<(), Error> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, Hide)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, interval, pss_interval);
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, Show)?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    interval: Duration,
    pss_interval: Duration,
) -> Result<(), Error> {
    let view = config::load_view();
    let slot = Arc::new(Mutex::new(None));
    let _sampler = proc::spawn_sampler(interval, pss_interval, slot.clone())?;
    let tree = proc::placeholder_tree();
    let mut app = App {
        expand: default_expand(),
        tree,
        cursor: 0,
        row_off: 0,
        row_vis: 1,
        view,
        filter_edit: false,
        col_off: 0,
        status: String::new(),
        help: false,
    };
    loop {
        let rows = flatten(&app.tree, &app.expand, &app.view);
        if app.cursor >= rows.len() {
            app.cursor = rows.len().saturating_sub(1);
        }
        app.row_vis = table_body_rows(terminal.size()?.height);
        app.row_off = follow_viewport(app.cursor, app.row_off, app.row_vis, rows.len());
        terminal.draw(|f| draw(f, &app, &rows))?;
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(k) = event::read()?
            && k.kind == KeyEventKind::Press
            && handle_key(&mut app, k.code, k.modifiers, &rows)?
        {
            break;
        }
        if let Some(tree) = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            app.tree = tree;
        }
    }
    Ok(())
}

fn default_expand() -> HashSet<String> {
    let mut s = HashSet::new();
    s.insert("host".into());
    let me = cpu::euid();
    s.insert(format!("user:{me}"));
    s.insert(format!("user:{me}/apps"));
    s.insert(format!("user:{me}/containers"));
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
    let host_n = tree_host_nproc(tree);
    rows.push(Flat {
        id: "host".into(),
        depth: 0,
        name: "Host".into(),
        nproc: host_n,
        metrics: host_metrics(tree),
        expandable: true,
    });
    if expand.contains("host") {
        for user in &tree.users {
            let uid = user.uid;
            let id = format!("user:{uid}");
            rows.push(Flat {
                id: id.clone(),
                depth: 1,
                name: format!("{} ({uid})", user.name),
                nproc: user_nproc(user),
                metrics: user_metrics(user),
                expandable: true,
            });
            if expand.contains(&id) {
                for (slug, title, idents) in [
                    ("apps", "Applications", &user.applications),
                    ("services", "User Services", &user.user_services),
                    ("containers", "Containers", &user.containers),
                ] {
                    push_folder(
                        &mut rows,
                        &FolderPush {
                            expand,
                            sort,
                            desc: view.desc,
                            depth: 2,
                            id: &format!("user:{uid}/{slug}"),
                            title,
                            idents,
                        },
                    );
                }
            }
        }
        for (id, title, idents) in [
            ("host/containers", "Containers", &tree.containers),
            ("host/system", "System", &tree.system),
        ] {
            push_folder(
                &mut rows,
                &FolderPush {
                    expand,
                    sort,
                    desc: view.desc,
                    depth: 1,
                    id,
                    title,
                    idents,
                },
            );
        }
    }
    if !filter.is_empty() {
        keep_matches(&mut rows, &filter);
    }
    rows
}

fn keep_matches(rows: &mut Vec<Flat>, filter: &str) {
    // `want` is the depth still needed to complete the ancestor chain of the
    // nearest match below. Tightening it to each kept row's own depth is what
    // limits the walk to that chain: a shallower row on another branch is
    // always preceded by the deeper rows of its own subtree, which do not
    // match and do not lower `want`, so it never becomes an empty header.
    let mut want = 0;
    let mut keep = vec![false; rows.len()];
    for (i, row) in rows.iter().enumerate().rev() {
        let d = row.depth;
        if row.name.to_ascii_lowercase().contains(filter) || d < want {
            keep[i] = true;
            want = d;
        }
    }
    let mut flags = keep.into_iter();
    rows.retain(|_| flags.next() == Some(true));
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

fn push_folder(rows: &mut Vec<Flat>, p: &FolderPush<'_>) {
    rows.push(Flat {
        id: p.id.to_string(),
        depth: p.depth,
        name: p.title.to_string(),
        nproc: folder_nproc(p.idents),
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
    let Some(key) = COLUMNS[sort.0].key else {
        let ord = a.title.cmp(&b.title);
        return if desc { ord } else { ord.reverse() };
    };
    let ord = key(a).total_cmp(&key(b));
    if desc { ord.reverse() } else { ord }
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
    if matches!(code, KeyCode::Char('?') | KeyCode::F(1)) {
        app.help = !app.help;
        return Ok(false);
    }
    if app.help && code == KeyCode::Esc {
        app.help = false;
        return Ok(false);
    }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('/') => app.filter_edit = true,
        KeyCode::Char('s' | 'S') => {
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
        KeyCode::Right | KeyCode::Char('l' | ' ') | KeyCode::Enter => {
            if let Some(r) = rows.get(app.cursor)
                && r.expandable
                && !app.expand.insert(r.id.clone())
            {
                app.expand.remove(&r.id);
            }
        }
        KeyCode::PageUp => app.cursor = app.cursor.saturating_sub(app.row_vis.max(1)),
        KeyCode::PageDown => {
            app.cursor = (app.cursor + app.row_vis.max(1)).min(rows.len().saturating_sub(1));
        }
        KeyCode::Home => app.cursor = 0,
        KeyCode::End => app.cursor = rows.len().saturating_sub(1),
        KeyCode::Char('[' | '<') => app.col_off = app.col_off.saturating_sub(1),
        KeyCode::Char(']' | '>') => app.col_off = app.col_off.saturating_add(1),
        _ => {}
    }
    Ok(false)
}

fn draw(f: &mut ratatui::Frame<'_>, app: &App, rows: &[Flat]) {
    let chunks = Layout::vertical([
        Constraint::Length(HEADER_ROWS),
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(f.area());
    draw_header(f, chunks[0], app);
    render_rule(f, chunks[1]);

    let skip = (app.col_off as usize).min(COLUMNS.len().saturating_sub(1));
    let shown: Vec<&str> = COLUMNS.iter().map(|c| c.header).skip(skip).collect();
    let start = app.row_off.min(rows.len());
    let end = start.saturating_add(app.row_vis.max(1)).min(rows.len());
    let mut table_rows = Vec::new();
    for (i, r) in rows[start..end].iter().enumerate() {
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
        let cells: Vec<String> = COLUMNS
            .iter()
            .map(|c| (c.fmt)(&name, r.nproc, &r.metrics))
            .skip(skip)
            .collect();
        let row = Row::new(cells);
        table_rows.push(if start + i == app.cursor {
            row.style(Style::default().add_modifier(Modifier::REVERSED))
        } else {
            row
        });
    }
    // Only the unscrolled view keeps the declared widths; once the name column
    // is off-screen every remaining column is numeric and shares one width.
    let widths: Vec<Constraint> = if skip == 0 {
        COLUMNS
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i == 0 {
                    Constraint::Min(c.width)
                } else {
                    Constraint::Length(c.width)
                }
            })
            .collect()
    } else {
        shown.iter().map(|_| Constraint::Length(8)).collect()
    };
    let table = Table::new(table_rows, widths).header(
        Row::new(shown.iter().map(|s| (*s).to_string()))
            .style(Style::default().add_modifier(Modifier::BOLD)),
    );
    f.render_widget(table, chunks[2]);
    if app.help {
        draw_help(f, chunks[2]);
    }

    let filter = if app.filter_edit {
        format!("filter> {}_", app.view.filter)
    } else if app.view.filter.is_empty() {
        String::new()
    } else {
        format!("filter: {}", app.view.filter)
    };
    let footer = format!(
        " q quit  / filter  c sort ({})  s save  [ ] scroll  ? help  {}  {}",
        app.view.sort, filter, app.status
    );
    render_rule(f, chunks[3]);
    f.render_widget(Paragraph::new(footer), chunks[4]);
}

fn render_rule(f: &mut ratatui::Frame<'_>, area: Rect) {
    f.render_widget(Paragraph::new("─".repeat(area.width as usize)), area);
}

fn draw_header(f: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let width = area.width as usize;
    f.render_widget(
        Paragraph::new(vec![
            cpu_header_line(&app.tree, width),
            mem_header_line(&app.tree, width),
        ]),
        area,
    );
}

fn pct_weight(p: f64) -> u64 {
    (p.clamp(0.0, 100.0) * 100.0).round() as u64
}

fn cpu_header_line(tree: &HostTree, width: usize) -> Line<'static> {
    let prefix = " CPU [";
    let mid = format!("] {:>5}%  ", fmt_pct(tree.cpu_pct));
    let (tail, legend_len) = legend(&[
        ("usr", Color::Cyan),
        ("sys", Color::Magenta),
        ("wait", Color::Yellow),
    ]);
    let bar_w = width.saturating_sub(prefix.len() + mid.len() + legend_len);
    let parts = [
        (pct_weight(tree.cpu_user_pct), Color::Cyan),
        (pct_weight(tree.cpu_system_pct), Color::Magenta),
        (pct_weight(tree.cpu_wait_pct), Color::Yellow),
    ];
    let mut spans = vec![Span::raw(prefix)];
    spans.extend(stacked_bar(bar_w, &parts, 10_000));
    spans.push(Span::raw(mid));
    spans.extend(tail);
    Line::from(spans)
}

fn mem_header_line(tree: &HostTree, width: usize) -> Line<'static> {
    let host = host_metrics(tree);
    let (vram, gtt) = if tree.unified_memory {
        (host.vram_bytes.unwrap_or(0), host.gtt_bytes.unwrap_or(0))
    } else {
        (0, 0)
    };
    let seg = mem::clip_used(MemParts {
        used: tree.mem_used_bytes,
        total: tree.mem_total_bytes,
        vram,
        gtt,
        cache: tree.mem_cached_bytes,
        buffers: tree.mem_buffers_bytes,
    });
    let prefix = " MEM [";
    let mid = format!(
        "] {}/{}  ",
        fmt_bytes(Some(tree.mem_used_bytes)),
        fmt_bytes(Some(tree.mem_total_bytes))
    );
    let (tail, legend_len) = legend(&[
        ("vram", Color::LightRed),
        ("gtt", Color::LightCyan),
        ("cache", Color::Blue),
        ("buf", Color::Green),
    ]);
    let bar_w = width.saturating_sub(prefix.len() + mid.len() + legend_len);
    let cap = tree.mem_total_bytes.max(1);
    let parts = [
        (seg.vram, Color::LightRed),
        (seg.gtt, Color::LightCyan),
        (seg.cache, Color::Blue),
        (seg.buffers, Color::Green),
        (seg.anon, Color::Gray),
    ];
    let mut spans = vec![Span::raw(prefix)];
    spans.extend(stacked_bar(bar_w, &parts, cap));
    spans.push(Span::raw(mid));
    spans.extend(tail);
    Line::from(spans)
}

/// Slash-joined coloured labels plus the columns they occupy. The bar width
/// subtracts that count, so deriving it here is what keeps a renamed label
/// from overflowing the line.
fn legend(labels: &[(&str, Color)]) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut cols = 0;
    for (i, (text, color)) in labels.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("/"));
            cols += 1;
        }
        spans.push(Span::styled(
            (*text).to_string(),
            Style::default().fg(*color),
        ));
        cols += text.chars().count();
    }
    (spans, cols)
}

fn stacked_bar(width: usize, parts: &[(u64, Color)], capacity: u64) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let weights: Vec<u64> = parts.iter().map(|(w, _)| *w).collect();
    let cells = share_cells(&weights, capacity, width);
    let filled: usize = cells.iter().sum();
    let mut out = Vec::new();
    for (n, (_, color)) in cells.into_iter().zip(parts.iter()) {
        if n > 0 {
            out.push(Span::styled("█".repeat(n), Style::default().fg(*color)));
        }
    }
    let rest = width.saturating_sub(filled);
    if rest > 0 {
        out.push(Span::styled(
            "░".repeat(rest),
            Style::default().fg(Color::DarkGray),
        ));
    }
    out
}

/// Largest-remainder allocation so segment cells sum to `floor(sum(parts)/capacity * width)`.
fn share_cells(parts: &[u64], capacity: u64, width: usize) -> Vec<usize> {
    let n = parts.len();
    if n == 0 || width == 0 || capacity == 0 {
        return vec![0; n];
    }
    let mut cells: Vec<usize> = parts
        .iter()
        .map(|w| ((u128::from(*w) * width as u128) / u128::from(capacity)) as usize)
        .collect();
    let assigned: usize = cells.iter().sum();
    let target = ((u128::from(parts.iter().copied().sum::<u64>()) * width as u128)
        / u128::from(capacity)) as usize;
    let mut extra = target
        .saturating_sub(assigned)
        .min(width.saturating_sub(assigned));
    let mut order: Vec<(u128, usize)> = parts
        .iter()
        .enumerate()
        .map(|(i, w)| (u128::from(*w) * width as u128 % u128::from(capacity), i))
        .collect();
    order.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, i) in order {
        if extra == 0 {
            break;
        }
        cells[i] += 1;
        extra -= 1;
    }
    cells
}

fn help_text() -> String {
    const ROWS: &[(&str, &str)] = &[
        ("q  Esc", "quit"),
        ("↑ ↓  j k", "move"),
        ("← →  h l", "expand / collapse"),
        ("Enter  Space", "expand / collapse"),
        ("/", "filter (Enter apply, Esc cancel)"),
        ("c", "cycle sort column"),
        ("d", "reverse sort"),
        ("s", "save view"),
        ("[ ]", "scroll columns"),
        ("?  F1", "toggle this help"),
    ];
    ROWS.iter()
        .map(|(k, d)| format!("{k:<16} {d}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn draw_help(f: &mut ratatui::Frame<'_>, area: Rect) {
    let text = help_text();
    let cols = u16::try_from(text.lines().map(|l| l.chars().count()).max().unwrap_or(0))
        .unwrap_or(u16::MAX);
    let rows = u16::try_from(text.lines().count()).unwrap_or(u16::MAX);
    let width = (cols + 2).min(area.width.saturating_sub(2)).max(3);
    let height = (rows + 2).min(area.height.saturating_sub(1)).max(3);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("keys")),
        popup,
    );
}

/// Body rows that fit in the table pane: term minus header(2), two rules,
/// column header(1), and footer(1).
fn table_body_rows(term_h: u16) -> usize {
    term_h.saturating_sub(6) as usize
}

/// First visible index so `selected` stays in `[offset, offset+visible)`.
fn follow_viewport(selected: usize, offset: usize, visible: usize, n: usize) -> usize {
    if visible == 0 || n == 0 {
        return 0;
    }
    let max_off = n.saturating_sub(visible);
    if selected < offset {
        selected.min(max_off)
    } else if selected >= offset.saturating_add(visible) {
        selected
            .saturating_add(1)
            .saturating_sub(visible)
            .min(max_off)
    } else {
        offset.min(max_off)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_viewport_keeps_selection_visible() {
        assert_eq!(follow_viewport(10, 0, 5, 20), 6);
        assert_eq!(follow_viewport(2, 5, 5, 20), 2);
        assert_eq!(follow_viewport(6, 5, 5, 20), 5);
        assert_eq!(follow_viewport(4, 0, 5, 20), 0);
        assert_eq!(follow_viewport(1, 8, 5, 3), 0);
        assert_eq!(follow_viewport(0, 0, 5, 0), 0);
        assert_eq!(follow_viewport(3, 0, 0, 10), 0);
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
    fn help_text_aligns_keys() {
        let text = help_text();
        let lines: Vec<&str> = text.lines().collect();
        assert!(!lines.is_empty());
        assert!(lines.iter().all(|l| !l.is_empty()));
        for line in &lines {
            let chars: Vec<char> = line.chars().collect();
            assert!(chars.len() > 16);
            assert_eq!(chars[16], ' ');
        }
        assert!(!text.contains("Observe only"));
    }

    #[test]
    fn table_body_rows_two_header_no_box() {
        assert_eq!(table_body_rows(24), 18);
        assert_eq!(table_body_rows(6), 0);
        assert_eq!(table_body_rows(7), 1);
    }

    #[test]
    fn share_cells_sums_to_fill() {
        let cells = share_cells(&[25, 25, 50], 100, 10);
        assert_eq!(cells.iter().sum::<usize>(), 10);
        assert_eq!(cells, vec![3, 2, 5]);
    }

    #[test]
    fn share_cells_leaves_remainder_for_idle() {
        let cells = share_cells(&[18, 10, 5], 100, 20);
        assert_eq!(cells.iter().sum::<usize>(), 6);
    }

    #[test]
    fn cpu_header_omits_disk_rates_and_fills_width() {
        let tree = HostTree {
            cpu_pct: 9.6,
            cpu_user_pct: 5.0,
            cpu_system_pct: 3.0,
            cpu_wait_pct: 1.6,
            system: vec![IdentNode {
                id: "sys".into(),
                title: "sys".into(),
                nproc: 1,
                metrics: Metrics {
                    disk_r_bps: Some(96.4e6),
                    disk_w_bps: Some(833.2e3),
                    ..Metrics::default()
                },
                instances: Vec::new(),
                containers: Vec::new(),
            }],
            ..HostTree::default()
        };
        let width = 80;
        let text = cpu_header_line(&tree, width).to_string();
        assert!(text.contains("usr"));
        assert!(text.contains("wait"));
        assert!(!text.contains("/s R"));
        assert!(!text.contains("/s W"));
        // The only check that catches a legend whose labels disagree with the
        // width the bar subtracts: the rendered line must land on `width`.
        assert_eq!(text.chars().count(), width);
        assert_eq!(cpu_header_line(&tree, 200).to_string().chars().count(), 200);
        assert_eq!(
            mem_header_line(&tree, width).to_string().chars().count(),
            width
        );
    }

    fn flat(depth: u16, name: &str) -> Flat {
        Flat {
            id: name.to_string(),
            depth,
            name: name.to_string(),
            nproc: 1,
            metrics: Metrics::default(),
            expandable: false,
        }
    }

    fn names(rows: &[Flat]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    /// Host / alice / {Applications / [firefox, cursor], User Services / pipewire}
    /// and Host / bob / Applications / vim.
    fn filter_rows() -> Vec<Flat> {
        vec![
            flat(0, "Host"),
            flat(1, "alice"),
            flat(2, "Applications"),
            flat(3, "firefox"),
            flat(3, "cursor"),
            flat(2, "User Services"),
            flat(3, "pipewire"),
            flat(1, "bob"),
            flat(2, "Applications"),
            flat(3, "vim"),
        ]
    }

    #[test]
    fn keep_matches_keeps_the_path_to_a_leaf() {
        let mut rows = filter_rows();
        keep_matches(&mut rows, "firefox");
        assert_eq!(names(&rows), ["Host", "alice", "Applications", "firefox"]);
    }

    #[test]
    fn keep_matches_drops_shallower_rows_that_are_not_ancestors() {
        // A match under bob keeps bob's chain only; alice's folders are
        // shallower than the match but sit on another branch, so they must not
        // survive as empty headers.
        let mut rows = filter_rows();
        keep_matches(&mut rows, "vim");
        assert_eq!(names(&rows), ["Host", "bob", "Applications", "vim"]);
    }

    #[test]
    fn keep_matches_drops_children_of_a_matching_folder() {
        let mut rows = filter_rows();
        keep_matches(&mut rows, "applications");
        assert_eq!(
            names(&rows),
            ["Host", "alice", "Applications", "bob", "Applications"]
        );
    }

    #[test]
    fn keep_matches_lowercases_the_name_but_not_the_filter() {
        let mut rows = filter_rows();
        keep_matches(&mut rows, "user services");
        assert_eq!(names(&rows), ["Host", "alice", "User Services"]);
        // Callers hand in an already-lowercased needle; a capital drops everything.
        let mut rows = filter_rows();
        keep_matches(&mut rows, "Firefox");
        assert!(rows.is_empty());
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

    fn ident(id: &str) -> IdentNode {
        IdentNode {
            id: id.into(),
            title: id.into(),
            nproc: 1,
            metrics: Metrics::default(),
            instances: Vec::new(),
            containers: Vec::new(),
        }
    }

    /// Every folder row exists for every user and for the host, but only the
    /// set HUMANS.md documents opens on a fresh start.
    #[test]
    fn default_expand_opens_only_the_documented_folders() {
        use crate::types::UserNode;
        let me = cpu::euid();
        let user = |uid: u32, name: &str| UserNode {
            uid,
            name: name.into(),
            applications: vec![ident("app")],
            user_services: vec![ident("svc")],
            containers: vec![ident("ctr")],
        };
        let tree = HostTree {
            users: vec![user(me, "me"), user(me + 1, "other")],
            containers: vec![ident("hostctr")],
            system: vec![ident("kthread")],
            ..HostTree::default()
        };
        let rows = flatten(&tree, &default_expand(), &View::default());
        let got: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        assert_eq!(
            got,
            [
                "host".to_string(),
                format!("user:{me}"),
                format!("user:{me}/apps"),
                format!("user:{me}/apps/app"),
                format!("user:{me}/services"),
                format!("user:{me}/containers"),
                format!("user:{me}/containers/ctr"),
                format!("user:{}", me + 1),
                "host/containers".to_string(),
                "host/system".to_string(),
            ]
        );
    }
}
