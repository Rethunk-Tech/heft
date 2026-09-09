use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{self, stdout};
use std::sync::{Arc, Mutex, OnceLock};
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
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};

use crate::config::{self, View};
use crate::cpu;
use crate::glyph;
use crate::mem::{self, MemParts};
use crate::once::{
    Column, Columns, Filter, Sort, fmt_bytes, fmt_pct, haystack, hide_column, ident_haystack,
    keep_matches, keep_top, keep_users, sort_tree, unhide_last,
};
use crate::proc;
use crate::types::{
    Error, HostTree, IdentNode, Metrics, ProcNode, folder_nproc, host_metrics, sum_idents,
    tree_host_nproc, user_metrics, user_nproc,
};

const HEADER_ROWS: u16 = 2;

struct App {
    tree: HostTree,
    cursor: usize,
    row_off: usize,
    row_vis: usize,
    expand: HashSet<String>,
    view: View,
    cols: Columns,
    filter_edit: bool,
    /// The last `/` text that compiled. Kept rather than recomputed per frame
    /// because a pattern is half-written for most of the keystrokes that make
    /// it: `app[` is not a regex, and dropping the filter for that frame would
    /// flash the whole tree back on screen between two characters. The footer
    /// says when the text on screen is not what is filtering.
    filter_re: Option<Filter>,
    /// Whether the text on screen compiles. The key handler already has the
    /// answer when it tries to replace `filter_re`, so the footer reads it
    /// instead of recompiling the pattern on every frame. Every site that
    /// edits the text goes through that one branch, which is what keeps this
    /// from going stale.
    filter_ok: bool,
    col_off: u16,
    status: String,
    help: bool,
}

/// # Errors
///
/// Returns an error if the terminal cannot enter or leave raw mode, the sampler
/// thread cannot be spawned, a frame cannot be drawn, or a view save fails.
pub fn run(interval: Duration, pss_interval: Duration, view: View) -> Result<(), Error> {
    // Resolve columns before the alternate screen: a warning about a stale
    // hide entry printed after it would be wiped on the first frame.
    let cols = Columns::from_view(&view);
    // Before raw mode, so the guard captures the settings it will have to put
    // back, and covers the window from here to the teardown below.
    crate::tty::guard();
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, Hide)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, interval, pss_interval, view, cols);
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, Show)?;
    // Disarm only once the terminal is genuinely back, or a signal arriving
    // during teardown would write escape codes over a shell prompt.
    crate::tty::released();
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    interval: Duration,
    pss_interval: Duration,
    view: View,
    cols: Columns,
) -> Result<(), Error> {
    let slot = Arc::new(Mutex::new(None));
    let _sampler = proc::spawn_sampler(interval, pss_interval, slot.clone())?;
    let tree = proc::placeholder_tree();
    let filter_re = Filter::new(&view.filter);
    let filter_ok = filter_re.is_some();
    let mut app = App {
        expand: default_expand(),
        tree,
        cursor: 0,
        row_off: 0,
        row_vis: 1,
        view,
        cols,
        filter_edit: false,
        filter_re,
        filter_ok,
        col_off: 0,
        status: String::new(),
        help: false,
    };
    // The highlight is this id, not `cursor`'s slot: PSS desc (the default)
    // reshuffles the flattened list every sample, and so do `c`/`d`, `/`,
    // `--top`, and expand/collapse.
    let mut cursor_id = String::from("host");
    loop {
        // Re-sorting an already ordered tree each frame is what lets `c` and
        // `d` reorder every level without re-sampling.
        sort_tree(
            &mut app.tree,
            Sort::from_label(&app.view.sort),
            app.view.desc,
        );
        let rows = flatten(&app.tree, &app.expand, &app.view, app.filter_re.as_ref());
        app.cursor = remap_cursor(&rows, &cursor_id);
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
        // After `j`/`k` (and after a gone row landed on its parent) pin the
        // id we will look up on the next flatten, not the one we arrived with.
        if let Some(r) = rows.get(app.cursor) {
            cursor_id = r.id.clone();
        }
        if let Some(tree) = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            app.tree = tree;
            keep_users(&mut app.tree, &app.view.users);
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
    /// See `TableRow::trimmable`: Host, Users and folder headers are the shape
    /// of the tree, not candidates for `--top`.
    trimmable: bool,
    /// See `TableRow::search`. A collapsed row has no process rows beneath it
    /// to match, so an identity carries the argv of its whole subtree or `/`
    /// would only reach what happens to be expanded.
    search: Option<String>,
}

fn flatten(
    tree: &HostTree,
    expand: &HashSet<String>,
    view: &View,
    filter: Option<&Filter>,
) -> Vec<Flat> {
    // Resolved before the walk: the argv haystack is built only for a tick
    // that searches it.
    let filter = filter.filter(|_| !view.filter.is_empty());
    let deep = filter.is_some();
    let mut rows = Vec::new();
    let host_n = tree_host_nproc(tree);
    rows.push(Flat {
        id: "host".into(),
        depth: 0,
        name: "Host".into(),
        nproc: host_n,
        metrics: host_metrics(tree),
        expandable: true,
        trimmable: false,
        search: None,
    });
    if expand.contains("host") {
        for user in &tree.users {
            let uid = user.uid;
            let id = format!("user:{uid}");
            rows.push(Flat {
                id: id.clone(),
                depth: 1,
                name: user.name.clone(),
                nproc: user_nproc(user),
                metrics: user_metrics(user),
                expandable: true,
                trimmable: false,
                search: None,
            });
            if expand.contains(&id) {
                for (slug, title, idents) in [
                    ("apps", "Applications", &user.applications),
                    ("services", "User Services", &user.user_services),
                    ("containers", "Containers", &user.containers),
                ] {
                    push_folder(
                        &mut rows,
                        expand,
                        2,
                        &format!("user:{uid}/{slug}"),
                        title,
                        idents,
                        deep,
                    );
                }
            }
        }
        for (id, title, idents) in [
            ("host/containers", "Containers", &tree.containers),
            ("host/system", "System", &tree.system),
        ] {
            push_folder(&mut rows, expand, 1, id, title, idents, deep);
        }
    }
    if let Some(filter) = filter {
        keep_rows(&mut rows, filter);
    }
    if let Some(n) = view.top {
        keep_top(&mut rows, n, |r| (r.depth, r.trimmable));
    }
    rows
}

fn keep_rows(rows: &mut Vec<Flat>, filter: &Filter) {
    keep_matches(rows, filter, |r| {
        (r.depth, r.search.as_deref().unwrap_or(r.name.as_str()))
    });
}

fn push_folder(
    rows: &mut Vec<Flat>,
    expand: &HashSet<String>,
    depth: u16,
    id: &str,
    title: &str,
    idents: &[IdentNode],
    deep: bool,
) {
    rows.push(Flat {
        id: id.to_string(),
        depth,
        name: format!("{title} ({})", idents.len()),
        nproc: folder_nproc(idents),
        metrics: sum_idents(idents),
        expandable: !idents.is_empty(),
        trimmable: false,
        search: None,
    });
    if idents.is_empty() || !expand.contains(id) {
        return;
    }
    for ident in idents {
        let iid = format!("{id}/{}", ident.id);
        rows.push(Flat {
            id: iid.clone(),
            depth: depth + 1,
            name: ident.title.clone(),
            nproc: ident.nproc,
            metrics: ident.metrics.clone(),
            expandable: true,
            trimmable: true,
            search: deep.then(|| ident_haystack(ident)),
        });
        if !expand.contains(&iid) {
            continue;
        }
        for member in &ident.containers {
            let mid = format!("{iid}/m/{}", member.id);
            rows.push(Flat {
                id: mid.clone(),
                depth: depth + 2,
                name: member.title.clone(),
                nproc: member.nproc,
                metrics: member.metrics.clone(),
                expandable: true,
                trimmable: true,
                search: deep.then(|| haystack(&member.title, &member.processes)),
            });
            if expand.contains(&mid) {
                push_procs(rows, expand, depth + 3, &mid, &member.processes, deep);
            }
        }
        for inst in &ident.instances {
            let sid = format!("{iid}/i/{}", inst.key);
            rows.push(Flat {
                id: sid.clone(),
                depth: depth + 2,
                name: inst.key.clone(),
                nproc: inst.nproc,
                metrics: inst.metrics.clone(),
                expandable: true,
                trimmable: true,
                search: deep.then(|| haystack(&inst.key, &inst.processes)),
            });
            if expand.contains(&sid) {
                push_procs(rows, expand, depth + 3, &sid, &inst.processes, deep);
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
    deep: bool,
) {
    for p in procs {
        let id = format!("{prefix}/p/{}", p.pid);
        let name = format!("{} [{}]", p.name, p.pid);
        rows.push(Flat {
            id: id.clone(),
            depth,
            search: deep.then(|| haystack(&name, std::slice::from_ref(p))),
            name,
            nproc: 1,
            metrics: p.metrics.clone(),
            expandable: !p.children.is_empty(),
            trimmable: true,
        });
        if expand.contains(&id) {
            push_procs(rows, expand, depth + 1, &id, &p.children, deep);
        }
    }
}

fn handle_key(
    app: &mut App,
    code: KeyCode,
    mods: KeyModifiers,
    rows: &[Flat],
) -> Result<bool, Error> {
    // Raw mode turns ISIG off, so the terminal never raises SIGINT and heft
    // has to answer Ctrl-C itself. Before this guard every modified key fell
    // through to its bare binding, which was not a near miss: Ctrl-C cycled
    // the sort column, Ctrl-D reversed the direction, and Ctrl-S wrote
    // view.json without the user ever pressing `s`. One guard rather than a
    // check per arm, so a binding added later cannot reintroduce it.
    //
    // SHIFT is deliberately not here: a capital is how you type one.
    if mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) {
        return Ok(mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c'));
    }
    if app.filter_edit {
        let before = app.view.filter.clone();
        match code {
            KeyCode::Esc => {
                app.filter_edit = false;
                app.view.filter.clear();
            }
            KeyCode::Enter => app.filter_edit = false,
            KeyCode::Backspace => {
                app.view.filter.pop();
            }
            KeyCode::Char(c) => {
                app.view.filter.push(c);
            }
            _ => {}
        }
        // Only replace it when the new text compiles, so a half-written
        // pattern keeps filtering with the last one that worked instead of
        // flashing the whole tree back between two keystrokes.
        if app.view.filter != before {
            let compiled = Filter::new(&app.view.filter);
            app.filter_ok = compiled.is_some();
            if let Some(f) = compiled {
                app.filter_re = Some(f);
            }
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
            let next = Sort::from_label(&app.view.sort).next(&app.cols);
            app.view.sort = next.label().into();
        }
        KeyCode::Char('d') => app.view.desc = !app.view.desc,
        KeyCode::Char('H') => {
            let label = app.view.sort.clone();
            let next = Sort::from_label(&label).next(&app.cols);
            if hide_column(&mut app.view, &label) {
                refresh_columns(app);
                if app.cols.iter().all(|c| c.label != app.view.sort) {
                    app.view.sort = next.label().into();
                }
                app.status = format!("hidden {label}");
            } else {
                app.status = "name cannot be hidden".into();
            }
        }
        KeyCode::Char('u' | 'U') => {
            if let Some(label) = unhide_last(&mut app.view) {
                refresh_columns(app);
                app.status = format!("shown {label}");
            }
        }
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

fn refresh_columns(app: &mut App) {
    app.cols = Columns::from_view(&app.view);
    let max = app.cols.len().saturating_sub(1) as u16;
    if app.col_off > max {
        app.col_off = max;
    }
}

/// Every header stays bold; the sort column is reversed so it is still marked
/// when `NO_COLOR` drops hue. The cursor row already uses reverse, so this is
/// the same highlight, on the one cell `c` is talking about.
fn sort_header<'a>(cols: impl Iterator<Item = &'a Column>, sort: &str) -> Row<'static> {
    Row::new(cols.map(|c| {
        let cell = Cell::from(c.header);
        if c.label == sort {
            cell.style(Style::default().add_modifier(Modifier::REVERSED))
        } else {
            cell
        }
    }))
    .style(Style::default().add_modifier(Modifier::BOLD))
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

    let skip = (app.col_off as usize).min(app.cols.len().saturating_sub(1));
    let start = app.row_off.min(rows.len());
    let end = start.saturating_add(app.row_vis.max(1)).min(rows.len());
    let mut table_rows = Vec::new();
    for (i, r) in rows[start..end].iter().enumerate() {
        let mark = if r.expandable {
            if app.expand.contains(&r.id) {
                glyph::expanded()
            } else {
                glyph::collapsed()
            }
        } else {
            "  "
        };
        let name = format!("{}{}{}", "  ".repeat(r.depth as usize), mark, r.name);
        let cells: Vec<String> = app
            .cols
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
    // Name keeps Min so the tree can use leftover width. Once it is scrolled
    // off, every remaining column is numeric and shares one width.
    let name_on_screen = app.cols.iter().skip(skip).any(|c| c.label == "name");
    let widths: Vec<Constraint> = app
        .cols
        .iter()
        .skip(skip)
        .map(|c| {
            if c.label == "name" {
                Constraint::Min(c.width)
            } else if name_on_screen {
                Constraint::Length(c.width)
            } else {
                Constraint::Length(8)
            }
        })
        .collect();
    let table = Table::new(table_rows, widths)
        .header(sort_header(app.cols.iter().skip(skip), &app.view.sort));
    f.render_widget(table, chunks[2]);
    if app.help {
        draw_help(f, chunks[2]);
    }

    // `?` says the text on screen is not a usable pattern yet, so what is on
    // the table is still the last one that compiled.
    let stale = if app.filter_ok { "" } else { " ?" };
    let filter = if app.filter_edit {
        format!("filter> {}_{stale}", app.view.filter)
    } else if app.view.filter.is_empty() {
        String::new()
    } else {
        format!("filter: {}{stale}", app.view.filter)
    };
    let footer = format!(
        " q quit  / filter  c sort ({})  s save  [ ] scroll  ? help  {}  {}",
        app.view.sort, filter, app.status
    );
    render_rule(f, chunks[3]);
    f.render_widget(Paragraph::new(footer), chunks[4]);
}

fn render_rule(f: &mut ratatui::Frame<'_>, area: Rect) {
    f.render_widget(
        Paragraph::new(glyph::rule().to_string().repeat(area.width as usize)),
        area,
    );
}

fn draw_header(f: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let width = area.width as usize;
    let (mem, mem_bar_w) = mem_header_line(&app.tree, width);
    f.render_widget(
        Paragraph::new(vec![cpu_header_line(&app.tree, width, mem_bar_w), mem]),
        area,
    );
}

fn pct_weight(p: f64) -> u64 {
    (p.clamp(0.0, 100.0) * 100.0).round() as u64
}

/// `bar_w` is the MEMORY row's first tank, not this row's own slack. The MEM
/// group's suffix is the longer of the two — `] 78.2G/125.1G  ` and a
/// four-label legend against `] 10.3%  ` and three — so left to itself this bar
/// overruns the one below it by about thirteen columns. Drawing both to the
/// same width and padding this row on the right is what makes the two brackets
/// stack. Where the MEMORY row splits into tanks (a discrete card, a host with
/// swap) it is the first tank that is matched: those are the two brackets in
/// the same place on the screen, and the pad simply runs under the tanks
/// beside it.
fn cpu_header_line(tree: &HostTree, width: usize, bar_w: usize) -> Line<'static> {
    let prefix = " CPU [";
    let mid = format!("] {:>5}%  ", fmt_pct(tree.cpu_pct));
    let (tail, legend_len) = legend(&[
        ("usr", Color::Cyan, glyph::full()),
        ("sys", Color::Magenta, glyph::dark()),
        ("wait", Color::Yellow, glyph::medium()),
    ]);
    // No host `psi` tail here: it is text on the row whose point is a bar.
    // `--once` and `--json` still carry the figures, where nothing is drawn to
    // scale. Nor is a tail here a way to line the two bars up: measured, one
    // cancels most of the suffix difference above by accident, and with no
    // tail the bars sit fourteen columns apart rather than four. `bar_w` is
    // what closes them.
    let fits = width.saturating_sub(prefix.len() + mid.len() + legend_len);
    let bar_w = bar_w.min(fits);
    let parts = [
        (pct_weight(tree.cpu_user_pct), Color::Cyan, glyph::full()),
        (
            pct_weight(tree.cpu_system_pct),
            Color::Magenta,
            glyph::dark(),
        ),
        (
            pct_weight(tree.cpu_wait_pct),
            Color::Yellow,
            glyph::medium(),
        ),
    ];
    let mut spans = vec![Span::raw(prefix)];
    spans.extend(stacked_bar(bar_w, &parts, 10_000));
    spans.push(Span::raw(mid));
    spans.extend(tail);
    spans.push(Span::raw(" ".repeat(fits - bar_w)));
    Line::from(spans)
}

/// `LABEL [bar] used/total  legend` sized to exactly `width` columns (the bar
/// absorbs the slack), so two of these can share one header row. Returns the
/// width the bar settled on as well as the spans, because the CPU row above is
/// drawn to the same scale.
fn bar_group(
    label: &str,
    width: usize,
    parts: &[(u64, Color, char)],
    cap: u64,
    used: u64,
    total: u64,
    labels: &[(&str, Color, char)],
) -> (Vec<Span<'static>>, usize) {
    let prefix = format!(" {label} [");
    let mid = format!("] {}/{}  ", fmt_bytes(Some(used)), fmt_bytes(Some(total)));
    let (tail, legend_len) = legend(labels);
    let bar_w = width.saturating_sub(prefix.len() + mid.len() + legend_len);
    let mut spans = vec![Span::raw(prefix)];
    spans.extend(stacked_bar(bar_w, parts, cap.max(1)));
    spans.push(Span::raw(mid));
    spans.extend(tail);
    (spans, bar_w)
}

/// The line, and the first tank's bar width for `cpu_header_line` to match.
fn mem_header_line(tree: &HostTree, width: usize) -> (Line<'static>, usize) {
    let host = host_metrics(tree);
    // Discrete VRAM is a second device, so measuring it against MemTotal is
    // meaningless; it gets its own capacity instead. Unified (APU) VRAM/GTT are
    // carve-outs of MemTotal and stay inside the MEM bar.
    let discrete = (!tree.unified_memory)
        .then(|| tree.vram_total_bytes.filter(|v| *v > 0))
        .flatten();
    // GTT is pinned system RAM on a discrete card too, already counted in
    // `used`, so it paints inside MEM either way. Both figures sum the drm
    // clients heft can see, not the device totals: sysfs has only
    // `mem_info_gtt_total`, a capacity, which as a usage would be a lie.
    let vram = if tree.unified_memory {
        host.vram_bytes.unwrap_or(0)
    } else {
        0
    };
    let gtt = host.gtt_bytes.unwrap_or(0);
    let seg = mem::clip_used(MemParts {
        used: tree.mem_used_bytes,
        total: tree.mem_total_bytes,
        vram,
        gtt,
        cache: tree.mem_cached_bytes,
        buffers: tree.mem_buffers_bytes,
    });
    // Swap is a device, not a slice of MemTotal, so painting it inside the MEM
    // bar would be the same lie discrete VRAM was moved out for. A swapless
    // host (`SwapTotal: 0`, the common case) gets no tank and no empty segment:
    // the MEM bar keeps the whole row exactly as it did before swap existed.
    let swap = (tree.swap_total_bytes > 0).then_some(tree.swap_total_bytes);
    // Rejected a third header row: AGENTS.md fixes the header at two unbordered
    // rows with rules above and below, so extra tanks split this row.
    let tanks = tank_widths(
        width,
        1 + usize::from(discrete.is_some()) + usize::from(swap.is_some()),
    );
    let mem_width = tanks[0];
    // `anon` is the unlabelled bulk of the bar, so it keeps the full block and
    // the carve-outs take the distinguishable glyphs. The two quadrant glyphs
    // are East Asian *narrow*, unlike the shade ramp, so they cannot double up
    // in a terminal that widens ambiguous characters.
    let mut labels: Vec<(&str, Color, char)> = Vec::new();
    if tree.unified_memory {
        labels.push(("vram", Color::LightRed, glyph::quad_a()));
    }
    labels.push(("gtt", Color::LightCyan, glyph::quad_b()));
    labels.push(("cache", Color::Blue, glyph::dark()));
    labels.push(("buf", Color::Green, glyph::medium()));
    let (mut spans, mem_bar_w) = bar_group(
        "MEM",
        mem_width,
        &[
            (seg.vram, Color::LightRed, glyph::quad_a()),
            (seg.gtt, Color::LightCyan, glyph::quad_b()),
            (seg.cache, Color::Blue, glyph::dark()),
            (seg.buffers, Color::Green, glyph::medium()),
            (seg.anon, Color::Gray, glyph::full()),
        ],
        tree.mem_total_bytes,
        tree.mem_used_bytes,
        tree.mem_total_bytes,
        &labels,
    );
    let mut next = 1;
    if let Some(vram_total) = discrete {
        let vram_used = tree.vram_used_bytes.unwrap_or(0).min(vram_total);
        spans.extend(
            bar_group(
                "VRAM",
                tanks[next],
                &[(vram_used, Color::LightRed, glyph::full())],
                vram_total,
                vram_used,
                vram_total,
                &[],
            )
            .0,
        );
        next += 1;
    }
    if let Some(swap_total) = swap {
        let swap_used = tree.swap_used_bytes.min(swap_total);
        spans.extend(
            bar_group(
                "SWAP",
                tanks[next],
                &[(swap_used, Color::Yellow, glyph::full())],
                swap_total,
                swap_used,
                swap_total,
                &[],
            )
            .0,
        );
    }
    (Line::from(spans), mem_bar_w)
}

/// Split one header row across its bar groups so the parts still sum to
/// `width`; a `width / tanks` each would lose the remainder and shorten the
/// line, which is exactly what the rendered-width assertions catch.
fn tank_widths(width: usize, tanks: usize) -> Vec<usize> {
    (0..tanks)
        .scan(width, |rest, i| {
            let w = *rest / (tanks - i);
            *rest -= w;
            Some(w)
        })
        .collect()
}

/// Slash-joined coloured labels plus the columns they occupy. The bar width
/// subtracts that count, so deriving it here is what keeps a renamed label
/// from overflowing the line.
fn legend(labels: &[(&str, Color, char)]) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut cols = 0;
    for (i, (text, color, glyph)) in labels.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("/"));
            cols += 1;
        }
        spans.push(Span::styled(format!("{glyph}{text}"), fg(*color)));
        cols += text.chars().count() + 1;
    }
    (spans, cols)
}

/// `NO_COLOR` (no-color.org): set to anything that is not the empty string
/// disables hue, whatever the value -- `0` and `false` disable it too. Kept
/// separate from the read so the rule can be checked without touching the
/// process environment.
fn no_color(var: Option<&OsStr>) -> bool {
    var.is_some_and(|v| !v.is_empty())
}

/// Read once. The environment cannot change while heft runs, the same reason
/// `glyph` resolves its character set once.
fn colored() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| !no_color(std::env::var_os("NO_COLOR").as_deref()))
}

/// Hue is the redundant half of every distinction heft draws, so honouring
/// `NO_COLOR` costs a reader nothing: the fill glyph stays either way.
fn fg(color: Color) -> Style {
    if colored() {
        Style::default().fg(color)
    } else {
        Style::default()
    }
}

/// Each segment carries its own fill glyph as well as its own colour, and the
/// legend prints that glyph beside the label. Hue alone cannot carry the
/// distinction: cyan against magenta is the pair deuteranopia collapses, and a
/// piped or recorded frame keeps the characters and loses the styling. Drawing
/// the glyphs unconditionally keeps one render path rather than a colour one
/// and a monochrome one that drift -- and it is what makes `NO_COLOR` a
/// styling question rather than a second layout.
fn stacked_bar(width: usize, parts: &[(u64, Color, char)], capacity: u64) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let weights: Vec<u64> = parts.iter().map(|(w, _, _)| *w).collect();
    let cells = share_cells(&weights, capacity, width);
    let filled: usize = cells.iter().sum();
    let mut out = Vec::new();
    for (n, (_, color, glyph)) in cells.into_iter().zip(parts.iter()) {
        if n > 0 {
            out.push(Span::styled(glyph.to_string().repeat(n), fg(*color)));
        }
    }
    let rest = width.saturating_sub(filled);
    if rest > 0 {
        out.push(Span::styled(
            glyph::light().to_string().repeat(rest),
            fg(Color::DarkGray),
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
    let (up, down, left, right) = glyph::arrows();
    let rows: [(String, &str); 12] = [
        ("q  Esc  Ctrl-C".to_string(), "quit"),
        (format!("{up} {down}  j k"), "move"),
        (format!("{left} {right}  h l"), "expand / collapse"),
        ("Enter  Space".to_string(), "expand / collapse"),
        ("/".to_string(), "filter by regex (Enter apply, Esc cancel)"),
        ("c".to_string(), "cycle sort column"),
        ("d".to_string(), "reverse sort"),
        ("H".to_string(), "hide sort column"),
        ("u".to_string(), "unhide last column"),
        ("s".to_string(), "save view"),
        ("[ ]".to_string(), "scroll columns"),
        ("?  F1".to_string(), "toggle this help"),
    ];
    rows.iter()
        .map(|(k, desc)| format!("{k:<16} {desc}"))
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

/// Body rows that fit in the table pane: the terminal minus the header, the
/// two rules, the column header and the footer.
fn table_body_rows(term_h: u16) -> usize {
    term_h.saturating_sub(HEADER_ROWS + 4) as usize
}

/// Index of `id` after the flattened list was rebuilt. A raw cursor followed
/// PSS rank (and every other reorder), so the highlight jumped identities each
/// sample. Missing id: the nearest remaining ancestor, walking `a/b/c` → `a/b`
/// → `a` so `firefox` does not match `firefox-esr`, or 0 if none remain.
fn remap_cursor(rows: &[Flat], id: &str) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let mut probe = id;
    loop {
        if let Some(i) = rows.iter().position(|r| r.id == probe) {
            return i;
        }
        match probe.rsplit_once('/') {
            Some((parent, _)) => probe = parent,
            None => return 0,
        }
    }
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

    fn row_id<'a>(rows: &'a [Flat], id: &str) -> &'a str {
        &rows[remap_cursor(rows, id)].id
    }

    /// Two trees, identities in opposite order: the cursor is firefox's id,
    /// not whichever row currently occupies firefox's old slot.
    #[test]
    fn remap_cursor_stays_on_the_same_id_when_the_list_reorders() {
        let expand = default_expand();
        let view = View::default();
        let me = cpu::euid();
        let id = format!("user:{me}/apps/firefox");
        let desc = flatten(
            &apps_tree(vec![ident("firefox"), ident("cursor")]),
            &expand,
            &view,
            None,
        );
        let asc = flatten(
            &apps_tree(vec![ident("cursor"), ident("firefox")]),
            &expand,
            &view,
            None,
        );
        assert_eq!(row_id(&desc, &id), id);
        assert_eq!(row_id(&asc, &id), id);
        assert_ne!(remap_cursor(&desc, &id), remap_cursor(&asc, &id));
    }

    #[test]
    fn remap_cursor_stays_on_the_same_id_when_a_sibling_is_inserted_above() {
        let expand = default_expand();
        let view = View::default();
        let me = cpu::euid();
        let id = format!("user:{me}/apps/firefox");
        let before = flatten(&apps_tree(vec![ident("firefox")]), &expand, &view, None);
        let after = flatten(
            &apps_tree(vec![ident("chrome"), ident("firefox")]),
            &expand,
            &view,
            None,
        );
        assert_eq!(row_id(&before, &id), id);
        assert_eq!(row_id(&after, &id), id);
        assert_eq!(remap_cursor(&before, &id) + 1, remap_cursor(&after, &id));
    }

    #[test]
    fn remap_cursor_survives_a_filter_that_drops_other_rows() {
        let mut rows = filter_rows();
        keep_rows(&mut rows, &f("firefox"));
        assert_eq!(row_id(&rows, "firefox"), "firefox");
        assert_eq!(names(&rows), ["Host", "alice", "Applications", "firefox"]);
    }

    #[test]
    fn remap_cursor_lands_on_the_parent_when_the_row_is_gone() {
        let expand = default_expand();
        let view = View::default();
        let me = cpu::euid();
        let rows = flatten(&apps_tree(vec![ident("cursor")]), &expand, &view, None);
        let gone = format!("user:{me}/apps/firefox");
        assert_eq!(row_id(&rows, &gone), format!("user:{me}/apps"));
        let gone_pid = format!("user:{me}/apps/firefox/i/pgid/p/123");
        assert_eq!(row_id(&rows, &gone_pid), format!("user:{me}/apps"));
        assert_eq!(remap_cursor(&[], "host"), 0);
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
    }

    #[test]
    fn table_body_rows_two_header_no_box() {
        assert_eq!(table_body_rows(24), 18);
        assert_eq!(table_body_rows(6), 0);
        assert_eq!(table_body_rows(7), 1);
    }

    #[test]
    fn the_sort_column_header_is_the_one_on_screen() {
        let cols = Columns::from_view(&View::default());
        assert_eq!(sort_header_title(&cols, 0, "pss"), Some("PSS"));
        assert_eq!(sort_header_title(&cols, 0, "name"), Some("NAME"));
        assert_eq!(sort_header_title(&cols, 1, "name"), None);
        let hidden = Columns::from_view(&View {
            hide_columns: vec!["pss".into()],
            ..View::default()
        });
        assert_eq!(sort_header_title(&hidden, 0, "pss"), None);
        assert_eq!(sort_header_title(&hidden, 0, "rss"), Some("RSS"));
        let _ = sort_header(cols.iter(), "pss");
    }

    fn sort_header_title(cols: &Columns, skip: usize, sort: &str) -> Option<&'static str> {
        cols.iter()
            .skip(skip)
            .find_map(|c| (c.label == sort).then_some(c.header))
    }

    /// Every ASCII substitute has to be one column, because the header lines
    /// are built to land on an exact width. `stacked_bar` takes its glyphs as
    /// arguments, so this checks the real render path rather than the table.
    /// no-color.org: presence and non-emptiness decide, never the value, so a
    /// shell that exports `NO_COLOR=0` still means it.
    #[test]
    fn no_color_reads_presence_not_value() {
        assert!(!no_color(None));
        assert!(!no_color(Some(OsStr::new(""))));
        for v in ["1", "0", "false", "no", "yes"] {
            assert!(no_color(Some(OsStr::new(v))), "{v}");
        }
    }

    #[test]
    fn an_ascii_bar_fills_exactly_as_many_cells_as_a_unicode_one() {
        let parts = |a: char, b: char, c: char| {
            [
                (30_u64, Color::Cyan, a),
                (30, Color::Magenta, b),
                (20, Color::Yellow, c),
            ]
        };
        for width in [1_usize, 7, 40, 137] {
            let uni: String = stacked_bar(width, &parts('█', '▓', '▒'), 100)
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            let asc: String = stacked_bar(width, &parts('#', '=', '+'), 100)
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            assert_eq!(uni.chars().count(), width, "unicode at {width}");
            assert_eq!(asc.chars().count(), width, "ascii at {width}");
            // Only the segments are arguments; the unfilled tail comes from
            // `glyph::light()`, which is process-wide and defaults to Unicode
            // here. Which character it is was checked in a pty against the
            // real binary; that it occupies one column is what this asserts.
        }
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
            // A host with memory: the MEMORY row's `] used/total  ` is what
            // makes its suffix the longer of the two, and a zeroed tree prints
            // `] 0/0  ` instead, which no machine does.
            mem_used_bytes: 80 * 1024 * 1024 * 1024,
            mem_total_bytes: 125 * 1024 * 1024 * 1024,
            mem_cached_bytes: 20 * 1024 * 1024 * 1024,
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
        let mut tree = tree;
        let width = 80;
        // The two rows as `draw_header` builds them: the CPU bar takes the
        // MEMORY row's first tank width, so the pair is checked together.
        let rows = |tree: &HostTree, width: usize| {
            let (mem, bar_w) = mem_header_line(tree, width);
            (
                cpu_header_line(tree, width, bar_w).to_string(),
                mem.to_string(),
            )
        };
        // The bar's closing bracket, which is the character a reader sees
        // stacked on the row below.
        let bracket = |line: &str| line.chars().position(|c| c == ']');

        let (text, mem) = rows(&tree, width);
        assert!(text.contains("usr"));
        assert!(text.contains("wait"));
        assert!(!text.contains("/s R"));
        assert!(!text.contains("/s W"));
        // The only check that catches a legend whose labels disagree with the
        // width the bar subtracts: the rendered line must land on `width`.
        assert_eq!(text.chars().count(), width);
        assert_eq!(mem.chars().count(), width);
        // Given its own slack the CPU bar runs about thirteen columns past the
        // MEMORY bar, which spends more of its row on `] used/total  ` and a
        // fourth legend label.
        assert_eq!(bracket(&text), bracket(&mem), "{text}\n{mem}");

        let (wide, wide_mem) = rows(&tree, 200);
        assert_eq!(wide.chars().count(), 200);
        assert_eq!(wide_mem.chars().count(), 200);
        assert_eq!(bracket(&wide), bracket(&wide_mem), "{wide}\n{wide_mem}");

        // A host with pressure renders no differently. The `psi` tail used to
        // sit on this row and was cancelling most of that suffix difference by
        // accident, which is why removing it alone widened the gap instead of
        // closing it.
        tree.psi_cpu_avg10 = Some(12.75);
        tree.psi_io_avg10 = None;
        tree.psi_mem_avg10 = Some(0.0);
        let (withpsi, psi_mem) = rows(&tree, width);
        assert!(!withpsi.contains("psi"), "{withpsi}");
        assert_eq!(withpsi.chars().count(), width);
        assert_eq!(bracket(&withpsi), bracket(&psi_mem), "{withpsi}\n{psi_mem}");

        // The one case that does not align, and the ceiling on `bar_w`: with
        // no memory at all the MEMORY suffix is the shorter of the two, and
        // this row stops at its own slack rather than overrunning the line to
        // reach the bracket below.
        let (cpu0, mem0) = rows(&HostTree::default(), width);
        assert_eq!(cpu0.chars().count(), width);
        assert_eq!(mem0.chars().count(), width);
        assert!(bracket(&cpu0) <= bracket(&mem0), "{cpu0}\n{mem0}");
    }

    fn tree_with_gpu(gpu: &mem::GpuPool) -> HostTree {
        let mem_total = 32 * 1024 * 1024 * 1024;
        HostTree {
            mem_used_bytes: 8 * 1024 * 1024 * 1024,
            mem_total_bytes: mem_total,
            vram_used_bytes: gpu.vram_used,
            vram_total_bytes: gpu.vram_total,
            unified_memory: mem::is_unified(mem_total, gpu),
            ..HostTree::default()
        }
    }

    #[test]
    fn discrete_vram_gets_its_own_capacity() {
        let g = 1024 * 1024 * 1024;
        let tree = tree_with_gpu(&mem::GpuPool {
            vram_used: Some(6 * g),
            vram_total: Some(12 * g),
            gtt_total: Some(4 * g),
        });
        assert!(!tree.unified_memory);
        let text = mem_header_line(&tree, 100).0.to_string();
        // The bug this guards: VRAM measured against MemTotal instead of the
        // card's own 12G.
        assert!(text.contains("VRAM ["), "{text}");
        assert!(text.contains("6.0G/12.0G"), "{text}");
        assert!(text.contains("8.0G/32.0G"), "{text}");
        assert_eq!(text.chars().count(), 100);
    }

    #[test]
    fn discrete_gtt_still_paints_in_the_mem_bar() {
        let g = 1024 * 1024 * 1024;
        let mut tree = tree_with_gpu(&mem::GpuPool {
            vram_used: Some(6 * g),
            vram_total: Some(12 * g),
            gtt_total: Some(4 * g),
        });
        tree.system = vec![IdentNode {
            id: "sys".into(),
            title: "sys".into(),
            nproc: 1,
            metrics: Metrics {
                gtt_bytes: Some(2 * g),
                ..Metrics::default()
            },
            instances: Vec::new(),
            containers: Vec::new(),
        }];
        let (line, _) = mem_header_line(&tree, 100);
        let text = line.to_string();
        // The bug this guards: GTT dropped entirely once VRAM moved to its own
        // tank, even though it is system RAM sitting inside the MEM bar's used.
        assert!(text.contains("▙gtt/▓cache/▒buf"), "{text}");
        assert!(
            !text.contains("vram/"),
            "discrete vram is not a MEM segment"
        );
        assert!(line.spans.iter().any(|s| s.content.contains('▙')), "{text}");
        assert_eq!(text.chars().count(), 100);
    }

    #[test]
    fn unified_vram_stays_in_the_mem_bar() {
        let g = 1024 * 1024 * 1024;
        let tree = tree_with_gpu(&mem::GpuPool {
            vram_used: Some(g / 2),
            vram_total: Some(g),
            gtt_total: Some(30 * g),
        });
        assert!(tree.unified_memory);
        let text = mem_header_line(&tree, 100).0.to_string();
        assert!(!text.contains("VRAM ["), "{text}");
        assert!(text.contains("▚vram/▙gtt/▓cache/▒buf"), "{text}");
        assert_eq!(text.chars().count(), 100);
    }

    #[test]
    fn tank_widths_always_sum_to_the_row() {
        for width in [0, 1, 79, 80, 100, 201] {
            for tanks in 1..=3 {
                let w = tank_widths(width, tanks);
                assert_eq!(w.len(), tanks);
                assert_eq!(w.iter().sum::<usize>(), width, "{width}/{tanks}");
            }
        }
        // The pre-swap split, unchanged: MEM first, the second tank the rest.
        assert_eq!(tank_widths(101, 2), vec![50, 51]);
    }

    /// The machine this was written on has `SwapTotal: 0`, and there the header
    /// must be byte-identical to the one heft printed before swap existed.
    #[test]
    fn a_swapless_host_gets_no_swap_tank() {
        let tree = tree_with_gpu(&mem::GpuPool::default());
        assert_eq!(tree.swap_total_bytes, 0);
        let text = mem_header_line(&tree, 100).0.to_string();
        assert!(!text.contains("SWAP"), "{text}");
        assert!(text.starts_with(" MEM ["), "{text}");
        assert_eq!(text.chars().count(), 100);
    }

    #[test]
    fn swap_gets_its_own_tank_beside_mem() {
        let g = 1024 * 1024 * 1024;
        let mut tree = tree_with_gpu(&mem::GpuPool::default());
        tree.swap_total_bytes = 8 * g;
        tree.swap_used_bytes = 2 * g;
        let text = mem_header_line(&tree, 100).0.to_string();
        // The bug this guards: swap painted as a segment of MemTotal, which
        // would put pages that are not in RAM inside the RAM bar.
        assert!(text.contains("SWAP ["), "{text}");
        assert!(text.contains("2.0G/8.0G"), "{text}");
        assert!(text.contains("8.0G/32.0G"), "{text}");
        assert_eq!(text.chars().count(), 100);

        // Discrete VRAM and swap together still fit the same two-row header.
        tree.vram_total_bytes = Some(12 * g);
        tree.vram_used_bytes = Some(6 * g);
        let text = mem_header_line(&tree, 120).0.to_string();
        assert!(text.contains("VRAM [") && text.contains("SWAP ["), "{text}");
        assert_eq!(text.chars().count(), 120);
    }

    fn flat(depth: u16, name: &str) -> Flat {
        Flat {
            search: None,
            id: name.to_string(),
            depth,
            name: name.to_string(),
            nproc: 1,
            metrics: Metrics::default(),
            expandable: false,
            trimmable: true,
        }
    }

    fn test_app() -> App {
        let view = View::default();
        App {
            cols: Columns::from_view(&view),
            filter_re: Filter::new(&view.filter),
            filter_ok: true,
            tree: HostTree::default(),
            cursor: 0,
            row_off: 0,
            row_vis: 10,
            expand: HashSet::new(),
            view,
            filter_edit: false,
            col_off: 0,
            status: String::new(),
            help: false,
        }
    }

    /// Reproduced against the real binary in a pty before this existed: from a
    /// default view, one Ctrl-C moved the sort pss -> rss and a second moved it
    /// to swap, exactly as pressing `c` twice would; Ctrl-D flipped `desc`; and
    /// Ctrl-S wrote view.json with no `s` ever pressed. Raw mode turns ISIG
    /// off, so nothing else was ever going to catch these.
    #[test]
    fn a_modified_key_never_reaches_its_bare_binding() {
        let ctrl = KeyModifiers::CONTROL;
        let rows = filter_rows();

        for (key, what) in [
            ('c', "sort cycle"),
            ('d', "sort direction"),
            ('s', "view save"),
            ('H', "hide column"),
            ('u', "unhide column"),
        ] {
            let mut app = test_app();
            let before = (
                app.view.sort.clone(),
                app.view.desc,
                app.view.hide_columns.clone(),
            );
            let quit = handle_key(&mut app, KeyCode::Char(key), ctrl, &rows).unwrap();
            assert_eq!(
                (
                    app.view.sort.clone(),
                    app.view.desc,
                    app.view.hide_columns.clone()
                ),
                before,
                "ctrl-{key} reached the {what} binding"
            );
            // Ctrl-C is the one that means something, and it means quit.
            assert_eq!(quit, key == 'c', "ctrl-{key} quit = {quit}");
        }

        // Alt and Super are dropped outright; neither quits nor acts.
        let mut app = test_app();
        assert!(!handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::ALT, &rows).unwrap());
        assert_eq!(app.view.sort, View::default().sort);

        // A capital is how you type one: SHIFT must still reach the bindings.
        let mut app = test_app();
        app.filter_edit = true;
        handle_key(&mut app, KeyCode::Char('A'), KeyModifiers::SHIFT, &rows).unwrap();
        assert_eq!(app.view.filter, "A");

        // And Ctrl-C quits out of the filter editor too, rather than typing.
        let mut app = test_app();
        app.filter_edit = true;
        assert!(handle_key(&mut app, KeyCode::Char('c'), ctrl, &rows).unwrap());
        assert!(app.view.filter.is_empty(), "ctrl-c must not type a `c`");
    }

    #[test]
    fn hide_hides_the_sort_column_and_u_puts_it_back() {
        let rows = filter_rows();
        let none = KeyModifiers::NONE;
        let mut app = test_app();
        assert_eq!(app.view.sort, "pss");
        handle_key(&mut app, KeyCode::Char('H'), none, &rows).unwrap();
        assert_eq!(app.view.hide_columns, ["pss"]);
        assert_eq!(app.view.sort, "rss");
        handle_key(&mut app, KeyCode::Char('u'), none, &rows).unwrap();
        assert!(app.view.hide_columns.is_empty());
        assert!(app.cols.iter().any(|c| c.label == "pss"));
    }

    #[test]
    fn the_footer_marker_tracks_every_edit_to_the_pattern() {
        let rows: Vec<Flat> = Vec::new();
        let none = KeyModifiers::NONE;
        let mut app = test_app();
        app.filter_edit = true;

        // Half-written: the table keeps the last pattern that worked, and the
        // footer has to say so.
        for c in ['a', 'p', 'p', '['] {
            handle_key(&mut app, KeyCode::Char(c), none, &rows).unwrap();
        }
        assert!(!app.filter_ok, "`app[` is not a pattern");

        // Completing it clears the marker.
        handle_key(&mut app, KeyCode::Char('a'), none, &rows).unwrap();
        handle_key(&mut app, KeyCode::Char(']'), none, &rows).unwrap();
        assert!(app.filter_ok, "`app[a]` is one");

        // Backspacing back into a broken pattern brings it back.
        handle_key(&mut app, KeyCode::Backspace, none, &rows).unwrap();
        assert!(!app.filter_ok, "backspace put it back to `app[a`");

        // Esc empties the pattern, and an empty one filters nothing and
        // compiles fine.
        handle_key(&mut app, KeyCode::Esc, none, &rows).unwrap();
        assert!(app.view.filter.is_empty());
        assert!(app.filter_ok, "an empty pattern is not a broken one");
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

    fn f(pattern: &str) -> Filter {
        Filter::new(pattern).expect("test patterns compile")
    }

    #[test]
    fn keep_matches_keeps_the_path_to_a_leaf() {
        let mut rows = filter_rows();
        keep_rows(&mut rows, &f("firefox"));
        assert_eq!(names(&rows), ["Host", "alice", "Applications", "firefox"]);
    }

    #[test]
    fn keep_matches_drops_shallower_rows_that_are_not_ancestors() {
        // A match under bob keeps bob's chain only; alice's folders are
        // shallower than the match but sit on another branch, so they must not
        // survive as empty headers.
        let mut rows = filter_rows();
        keep_rows(&mut rows, &f("vim"));
        assert_eq!(names(&rows), ["Host", "bob", "Applications", "vim"]);
    }

    #[test]
    fn keep_matches_drops_children_of_a_matching_folder() {
        let mut rows = filter_rows();
        keep_rows(&mut rows, &f("applications"));
        assert_eq!(
            names(&rows),
            ["Host", "alice", "Applications", "bob", "Applications"]
        );
    }

    /// A bare substring has always matched regardless of case, and every saved
    /// `view.json` was written against that, so the regex is case-insensitive
    /// unless the pattern turns it off.
    #[test]
    fn a_filter_ignores_case_unless_the_pattern_says_otherwise() {
        let mut rows = filter_rows();
        keep_rows(&mut rows, &f("user services"));
        assert_eq!(names(&rows), ["Host", "alice", "User Services"]);

        let mut rows = filter_rows();
        keep_rows(&mut rows, &f("Firefox"));
        assert_eq!(names(&rows), ["Host", "alice", "Applications", "firefox"]);

        let mut rows = filter_rows();
        keep_rows(&mut rows, &f("(?-i)Firefox"));
        assert!(rows.is_empty(), "(?-i) turns the default back off");
    }

    /// The reason for the engine at all: a substring cannot say "these two and
    /// nothing else", and `--filter chrome` matching `chrome-sandbox` is the
    /// case that made the column noisy.
    #[test]
    fn a_filter_can_anchor_and_alternate() {
        let mut rows = filter_rows();
        keep_rows(&mut rows, &f("^(firefox|vim)$"));
        assert_eq!(
            names(&rows),
            [
                "Host",
                "alice",
                "Applications",
                "firefox",
                "bob",
                "Applications",
                "vim"
            ]
        );

        // An anchored name that only appears as a substring elsewhere.
        let mut rows = filter_rows();
        keep_rows(&mut rows, &f("^app"));
        assert_eq!(
            names(&rows),
            ["Host", "alice", "Applications", "bob", "Applications"]
        );
    }

    #[test]
    fn an_unusable_pattern_compiles_to_nothing_rather_than_panicking() {
        assert!(Filter::new("[").is_none());
        assert!(Filter::new("a(").is_none());
        assert!(Filter::new("").is_some(), "an empty pattern is legal");
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

    fn apps_tree(apps: Vec<IdentNode>) -> HostTree {
        use crate::types::UserNode;
        HostTree {
            users: vec![UserNode {
                uid: cpu::euid(),
                name: "me".into(),
                applications: apps,
                user_services: Vec::new(),
                containers: Vec::new(),
            }],
            ..HostTree::default()
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
        let rows = flatten(&tree, &default_expand(), &View::default(), None);
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

    /// Reproduced on a user with no containers: `user:{uid}/containers` is in
    /// the default expand set, so the empty folder drew the expanded marker
    /// over nothing. A folder with no identities is not expandable; the id
    /// stays in the set so the first container that appears still opens.
    #[test]
    fn empty_folders_do_not_draw_as_expanded() {
        use crate::types::UserNode;
        let me = cpu::euid();
        let tree = HostTree {
            users: vec![UserNode {
                uid: me,
                name: "me".into(),
                applications: vec![ident("app")],
                user_services: Vec::new(),
                containers: Vec::new(),
            }],
            ..HostTree::default()
        };
        let rows = flatten(&tree, &default_expand(), &View::default(), None);
        let find = |suffix: &str| {
            let id = format!("user:{me}/{suffix}");
            rows.iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        assert!(find("apps").expandable);
        assert_eq!(find("apps").name, "Applications (1)");
        assert!(!find("services").expandable);
        assert_eq!(find("services").name, "User Services (0)");
        assert!(!find("containers").expandable);
        assert_eq!(find("containers").name, "Containers (0)");
        assert_eq!(
            rows.iter()
                .find(|r| r.id == format!("user:{me}"))
                .unwrap()
                .name,
            "me"
        );
        assert_eq!(
            rows.iter()
                .find(|r| r.id == "host/containers")
                .unwrap()
                .name,
            "Containers (0)"
        );
        assert_eq!(
            rows.iter().find(|r| r.id == "host/system").unwrap().name,
            "System (0)"
        );
        assert!(
            !rows
                .iter()
                .any(|r| r.id.starts_with(&format!("user:{me}/containers/"))),
            "an empty Containers folder must not grow child rows"
        );
        assert!(default_expand().contains(&format!("user:{me}/containers")));
    }
}
