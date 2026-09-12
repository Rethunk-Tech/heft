use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::io::{self, Write, stdout};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

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
use ratatui::widgets::{Block, Borders, Cell, Clear, Padding, Paragraph, Row, Table, Wrap};

use crate::caps;
use crate::config::{self, View};
use crate::cpu;
use crate::glyph;
use crate::kgp;
use crate::mem::{self, MemParts};
use crate::once::{
    COLUMNS, Column, Columns, Filter, Sort, fmt_bytes, fmt_pct, haystack, hide_column,
    ident_haystack, keep_matches, keep_top, keep_users, sort_tree, unhide_last,
};
use crate::proc;
use crate::sixel;
use crate::types::{
    Error, HostTree, IdentNode, Metrics, ProcNode, folder_nproc, host_metrics, sum_idents,
    tree_host_nproc, user_metrics, user_nproc,
};

/// CPU and MEMORY. A machine with swap gets a third for it.
const HEADER_ROWS: u16 = 2;

/// Swap is a device of its own, not a slice of MemTotal, and its readout needs
/// the same couple of dozen columns whatever the terminal is. Sharing the
/// MEMORY row with it cost MEM half its width, and the CPU bar is sized to
/// match MEM's, so both headline bars halved to make room for it. It gets its
/// own row instead, and a swapless host -- `SwapTotal: 0` -- still draws the
/// two rows it always did.
fn header_rows(tree: &HostTree) -> u16 {
    HEADER_ROWS + u16::from(tree.swap_total_bytes > 0)
}

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
    /// The `i` overlay. Holds no data of its own: it is drawn from the selected
    /// row of the current frame, so it follows a re-sort or a new sample the
    /// way the highlight does rather than freezing a stale snapshot.
    detail: bool,
    /// When `p` froze the table, or `None` when it is live. Sampling carries on
    /// underneath, so unpausing shows the current machine rather than replaying
    /// a backlog; this only stops the swap into `tree`. Sorting, filtering,
    /// expanding and `i` all keep working on the held tree, which is the point
    /// of freezing it.
    paused: Option<Instant>,
    /// Recent values of the sort metric per row id, oldest first, for the
    /// `spark` column. Only the TUI has this: `--once` and `--json` take two
    /// walks, so there is no history for them to draw.
    ///
    /// Cleared when the sort column changes, because the buffer would
    /// otherwise hold two metrics in different units and draw them as one
    /// picture. Rows that stop appearing are dropped on the same pass that
    /// records, so an exited process does not hold a buffer for the run.
    history: HashMap<String, VecDeque<f64>>,
    /// Present only under `--trend kitty`: the image in flight, and what it
    /// costs to keep it in step. `None` is the ordinary character ramp.
    kgp: Option<kgp::Kgp>,
    /// Which of the three TREND renderings is in play.
    trend: TrendMode,
    /// A sixel image and the escape that places it, built during `draw` and
    /// written after ratatui has flushed -- sixel paints over cells rather
    /// than into them, so it has to go last.
    sixel_out: Option<String>,
    /// The sort label `history` was collected under, so a change to it can
    /// clear the buffers rather than mixing units.
    sorted_by: String,
}

/// Foreground colour the TREND cells carry under `--trend sixel`, so the
/// rectangle they occupy can be read back out of the rendered frame. The cells
/// themselves are spaces, so it is never seen; the layout puts the name column
/// on a `Constraint::Min` and only ratatui's solver knows what that absorbed.
const SIXEL_MARK: Color = Color::Rgb(0, 0, 1);

/// How many samples the trend keeps: the `spark` column's width, since a cell
/// can draw no more than that.
const TREND: usize = 9;

/// How the TREND column is drawn. Resolved in `main` from `--trend`, never
/// detected: `TERM` names a terminal, not what it implements.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrendMode {
    /// Ask the terminal, once, before the first frame.
    Auto,
    Chars,
    Kitty,
    Sixel,
}

/// Turn what the terminal answered into what to draw.
///
/// The kitty protocol ties its image to the cell grid, so it survives a
/// scroll without repainting, and where the terminal is on this machine its
/// pixels go through shared memory and cost tens of bytes a frame. That makes
/// it the first choice locally. Over ssh the same transport has to send every
/// pixel inline -- measured at about 146 KB a sample against sixel's 559 bytes
/// a frame, because a line is mostly empty and sixel run-length-encodes the
/// empty part -- so where the terminal is at the other end of a connection and
/// offers sixel, sixel wins.
fn resolve_trend(caps: caps::Caps, remote: bool) -> TrendMode {
    match (caps.kitty, caps.sixel, remote) {
        (true, true, true) | (false, true, _) => TrendMode::Sixel,
        (true, _, _) => TrendMode::Kitty,
        (false, false, _) => TrendMode::Chars,
    }
}

/// # Errors
///
/// Returns an error if the terminal cannot enter or leave raw mode, the sampler
/// thread cannot be spawned, a frame cannot be drawn, or a view save fails.
pub fn run(
    interval: Duration,
    pss_interval: Duration,
    view: View,
    trend: TrendMode,
) -> Result<(), Error> {
    // Resolve columns before the alternate screen: a warning about a stale
    // hide entry printed after it would be wiped on the first frame.
    let cols = Columns::for_tui(&view);
    // Before raw mode, so the guard captures the settings it will have to put
    // back, and covers the window from here to the teardown below.
    crate::tty::guard();
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, Hide)?;
    // After raw mode and inside the alternate screen: the reply would
    // otherwise be line-buffered and echoed, and any terminal that prints the
    // query instead of answering it has that wiped by the first frame.
    let trend = if trend == TrendMode::Auto {
        let resolved = resolve_trend(caps::probe(), kgp::is_remote());
        caps::drain();
        resolved
    } else {
        trend
    };
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, interval, pss_interval, view, cols, trend);
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
    trend: TrendMode,
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
        detail: false,
        paused: None,
        history: HashMap::new(),
        kgp: (trend == TrendMode::Kitty).then(kgp::Kgp::new),
        trend,
        sixel_out: None,
        sorted_by: String::new(),
    };
    // The highlight is this id, not `cursor`'s slot: PSS desc (the default)
    // reshuffles the flattened list every sample, and so do `c`/`d`, `/`,
    // `--top`, and expand/collapse.
    let mut cursor_id = String::from("host");
    let mut fresh = true;
    loop {
        // Re-sorting an already ordered tree each frame is what lets `c` and
        // `d` reorder every level without re-sampling.
        sort_tree(
            &mut app.tree,
            Sort::from_label(&app.view.sort),
            app.view.desc,
        );
        let rows = flatten(&app.tree, &app.expand, &app.view, app.filter_re.as_ref());
        // Once per published sample, not once per frame: a redraw for a
        // keypress is not a new measurement.
        if std::mem::take(&mut fresh) {
            record_history(&mut app, &rows);
        }
        app.cursor = remap_cursor(&rows, &cursor_id);
        app.row_vis = table_body_rows(terminal.size()?.height, header_rows(&app.tree));
        app.row_off = follow_viewport(app.cursor, app.row_off, app.row_vis, rows.len());
        terminal.draw(|f| draw(f, &mut app, &rows))?;
        // Last, deliberately: ratatui rewrites only the cells that changed,
        // and a rewritten cell erases the pixels over it, so the image is
        // repainted every frame rather than hashed the way the kitty one is.
        // A sparkline is mostly empty and sixel run-length-encodes the empty
        // part, so a frame is a couple of kilobytes.
        if let Some(px) = app.sixel_out.take() {
            let mut out = io::stdout();
            out.write_all(px.as_bytes())?;
            out.flush()?;
        }
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
            cursor_id.clone_from(&r.id);
        }
        // Taken even while paused, so the sampler's slot never backs up and
        // unpausing shows the machine as it is rather than as it was.
        if let Some(tree) = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            && app.paused.is_none()
        {
            app.tree = tree;
            keep_users(&mut app.tree, &app.view.users);
            fresh = true;
        }
    }
    if let Some(k) = app.kgp.as_mut() {
        k.teardown(&mut io::stdout());
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

/// Append this sample's sort-metric value for every visible row, and forget
/// the rows that are gone. Keyed by `Flat::id`, so a row keeps its history
/// across a re-sort or a filter but a genuinely different row never inherits
/// one.
fn record_history(app: &mut App, rows: &[Flat]) {
    let sort = Sort::from_label(&app.view.sort);
    // Two metrics in one buffer would be drawn as one picture, so changing
    // what is measured starts the measurement again.
    if app.sorted_by != sort.label() {
        app.history.clear();
        app.sorted_by = sort.label().to_string();
    }
    app.history.retain(|id, _| rows.iter().any(|r| &r.id == id));
    for r in rows {
        let v = sort.value(r.nproc, &r.metrics).unwrap_or(0.0);
        let buf = app.history.entry(r.id.clone()).or_default();
        if buf.len() == TREND {
            buf.pop_front();
        }
        buf.push_back(v);
    }
}

/// What a full-height mark stands for this tick: the metric's own full scale
/// where it has one, else the heaviest history among the rows that are
/// entries rather than totals.
///
/// `Flat::trimmable` is the same "is this an entry" test `--top` uses, and it
/// is what keeps Host out of it: Host is the sum of the machine, so scaling
/// against it would draw every real row flat along the bottom.
///
/// Zero when nothing has a figure yet, which `spark` and `paint` both read as
/// "put the mark on the floor".
fn trend_scale(sort: Sort, rows: &[Flat], history: &HashMap<String, VecDeque<f64>>) -> f64 {
    if let Some(full) = sort.trend_full() {
        return full;
    }
    rows.iter()
        .filter(|r| r.trimmable)
        .filter_map(|r| history.get(&r.id))
        .flat_map(|b| b.iter().copied())
        .fold(0.0_f64, f64::max)
}

/// A row's recent history as rising blocks against `full`, not against the
/// row's own peak.
///
/// Its own peak is what this did first, and it made most of the column solid:
/// a row sitting flat at 2% had every sample equal to its own maximum, so it
/// drew nine full-height blocks, while a row flat at zero drew a line along
/// the bottom. "Flat and idle" and "flat and busy" came out as opposites, and
/// no two rows could be compared at all.
///
/// Zero to the lowest step rather than to a blank: a row that has been at zero
/// throughout has a history, and it is flat. A row with no history at all --
/// one that has just appeared -- gets an empty cell, which is heft's blank:
/// no figure exists yet.
fn spark(buf: Option<&VecDeque<f64>>, full: f64) -> String {
    let Some(buf) = buf.filter(|b| !b.is_empty()) else {
        return String::new();
    };
    let ramp = glyph::spark_ramp();
    if full.is_nan() || full <= 0.0 {
        // Nothing on screen has a figure yet, so every row is on the floor
        // rather than dividing by it.
        return ramp[0].to_string().repeat(buf.len());
    }
    let top = (ramp.len() - 1) as f64;
    buf.iter()
        .map(|v| {
            if v.is_nan() || *v <= 0.0 {
                return ramp[0];
            }
            let step = ((v / full).min(1.0) * top).round();
            ramp[(step as usize).min(ramp.len() - 1)]
        })
        .collect()
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
    if code == KeyCode::Char('p') {
        app.paused = match app.paused {
            Some(_) => None,
            None => Some(Instant::now()),
        };
        return Ok(false);
    }
    if code == KeyCode::Char('i') {
        app.detail = !app.detail;
        // Only one overlay is drawn, so leaving help armed underneath would
        // make the next `?` look like it did nothing.
        app.help = false;
        return Ok(false);
    }
    if app.detail && code == KeyCode::Esc {
        app.detail = false;
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
    app.cols = Columns::for_tui(&app.view);
    let max = app.cols.len().saturating_sub(1) as u16;
    if app.col_off > max {
        app.col_off = max;
    }
}

/// How many columns, starting at `skip`, fit in `avail` at their full width.
///
/// ratatui clips a cell that runs out of room, so a 50-column terminal drew
/// `20.1G` as `2` and `548.5` as `5` with nothing to say they had been cut —
/// heft showing a figure that is wrong, which is the one thing every other
/// rule in it avoids. A column is now either drawn whole or not drawn, and
/// `[` / `]` reach the ones left off, which is what those keys are for.
///
/// At least one column always survives. The name column is a label rather than
/// a figure, so a cut name misleads nobody; `once::trunc` already ellipsises
/// it.
fn columns_that_fit(cols: &Columns, skip: usize, avail: u16, name_on_screen: bool) -> usize {
    // ratatui's default spacing between two columns.
    const SPACING: u16 = 1;
    let mut used = 0u16;
    let mut n = 0usize;
    for (i, c) in cols.iter().skip(skip).enumerate() {
        let w = if c.label == "name" || name_on_screen {
            c.width
        } else {
            8
        };
        let need = if i == 0 { w } else { w + SPACING };
        if used + need > avail {
            break;
        }
        used += need;
        n += 1;
    }
    n.max(1)
}

/// A cgroup stalled for this share of an interval is contending for the
/// resource rather than merely using it. Below it the figure is ordinary
/// scheduling noise, and marking it would train the eye to ignore the mark.
const STALL_ALARM: f64 = 20.0;

/// Whether this column's figure, on this row, says the row is in trouble.
///
/// `D` is why that column exists: a process in uninterruptible sleep gets no
/// work done and cannot be killed, and `%CORE` reads it as idle, so nothing
/// else on the row distinguishes a machine stuck on a dead NFS mount from a
/// quiet one. The stall columns are the same argument for a cgroup.
fn alarming(label: &str, m: &Metrics) -> bool {
    let over = |v: Option<f64>| v.is_some_and(|v| v >= STALL_ALARM);
    match label {
        "dstate" => m.d_state_procs > 0,
        "cpustall" => over(m.cpu_stall_pct),
        "iostall" => over(m.io_stall_pct),
        "memstall" => over(m.mem_stall_pct),
        _ => false,
    }
}

/// Red where there is colour, reverse video where there is not -- the same
/// fallback the sort header already uses, so the mark survives `NO_COLOR`,
/// a pipe, and a monochrome terminal. Reverse also costs no column width,
/// which a marker character would: `CPU ST` is six wide and `100.0` is five.
fn alarm_style() -> Style {
    if colored() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
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

/// Paint every visible row's history into one image and hand it to the
/// terminal, returning whether the placeholders may be drawn.
///
/// One image for the whole column: the protocol's row diacritics index into
/// it, so nine cells of one band cost the same escape as the whole table. The
/// row range is known here and nowhere else, which is why this runs from
/// `draw` rather than from the loop.
///
/// Any reason it cannot be done -- a terminal that reports no pixel size, an
/// absurd cell, a write that failed -- falls back to the character ramp for
/// that frame rather than leaving the column blank.
/// The image both image transports draw, or `None` where one cannot be made:
/// a terminal that reports no pixel size, an absurd cell, an empty table.
fn trend_image(
    app: &App,
    rows: &[Flat],
    start: usize,
    end: usize,
    full: f64,
) -> Option<kgp::Image> {
    let cell = kgp::cell_px()?;
    let bands: Vec<Option<&VecDeque<f64>>> = rows[start..end]
        .iter()
        .map(|r| r.trimmable.then(|| app.history.get(&r.id)).flatten())
        .collect();
    if bands.is_empty() || bands.len() > kgp::MAX_BANDS {
        return None;
    }
    kgp::paint(&bands, TREND as u32, cell, trend_colour(), full)
}

/// The cyan the CPU bar's `usr` segment already uses, and a neutral grey under
/// `NO_COLOR` -- an image is the one thing in heft that cannot fall back to a
/// fill character, so it answers the variable directly.
fn trend_colour() -> [u8; 3] {
    if colored() {
        [0x00, 0xaf, 0xd7]
    } else {
        [0xbc, 0xbc, 0xbc]
    }
}

/// The top-left cell of the marked TREND column, read back out of the frame
/// ratatui just filled. Computing it instead would mean re-deriving the layout
/// solver: the name column is a `Constraint::Min` and only the solver knows
/// what it absorbed.
fn marked_corner(buf: &ratatui::buffer::Buffer, area: Rect) -> Option<(u16, u16)> {
    let mut best: Option<(u16, u16)> = None;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if buf.cell((x, y)).is_some_and(|c| c.fg == SIXEL_MARK) {
                best = Some(match best {
                    Some((bx, by)) => (bx.min(x), by.min(y)),
                    None => (x, y),
                });
            }
        }
    }
    best
}

fn send_trend(app: &mut App, rows: &[Flat], start: usize, end: usize, full: f64) -> bool {
    let Some(img) = trend_image(app, rows, start, end, full) else {
        return false;
    };
    let Ok(rows_tall) = u32::try_from(end - start) else {
        return false;
    };
    let Some(k) = app.kgp.as_mut() else {
        return false;
    };
    k.send(&mut io::stdout(), &img, TREND as u32, rows_tall)
        .is_ok()
}

fn draw(f: &mut ratatui::Frame<'_>, app: &mut App, rows: &[Flat]) {
    let chunks = Layout::vertical([
        Constraint::Length(header_rows(&app.tree)),
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(f.area());
    draw_header(f, chunks[0], app);
    render_rule(f, chunks[1]);

    let skip = (app.col_off as usize).min(app.cols.len().saturating_sub(1));
    let name_on_screen = app.cols.iter().skip(skip).any(|c| c.label == "name");
    let fit = columns_that_fit(&app.cols, skip, chunks[2].width, name_on_screen);
    let start = app.row_off.min(rows.len());
    let end = start.saturating_add(app.row_vis.max(1)).min(rows.len());
    // Only when the column is actually on screen: an image nothing references
    // is invisible either way, and on the inline transport it is most of a
    // megabyte of escape for nothing.
    let spark_shown = app
        .cols
        .iter()
        .skip(skip)
        .take(fit)
        .any(|c| c.label == "spark");
    // One scale for every row of the frame, so the column is comparable down
    // the table and not just within a row.
    let full = trend_scale(Sort::from_label(&app.view.sort), rows, &app.history);
    let (trend_img, trend_pixels) = match app.trend {
        // `run` resolves Auto before the first frame, so it never reaches
        // here; drawing characters is the right answer if it ever did.
        TrendMode::Auto | TrendMode::Chars => (false, None),
        TrendMode::Kitty => (
            spark_shown && end > start && send_trend(app, rows, start, end, full),
            None,
        ),
        TrendMode::Sixel => {
            let img = (spark_shown && end > start)
                .then(|| trend_image(app, rows, start, end, full))
                .flatten();
            (img.is_some(), img)
        }
    };
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
        let cells: Vec<Cell> = app
            .cols
            .iter()
            .map(|c| {
                // The one column whose value is not a function of this
                // sample, so `Column::fmt` (which sees only this sample)
                // cannot produce it.
                let sixel_cell = c.label == "spark" && trend_img && app.trend == TrendMode::Sixel;
                let placed = if c.label == "spark"
                    && trend_img
                    && r.trimmable
                    && app.trend == TrendMode::Kitty
                {
                    kgp::placeholder(i, TREND)
                } else {
                    None
                };
                let text = match (&placed, c.label) {
                    (Some(p), _) => p.clone(),
                    // Spaces the image is painted over. The cells still have
                    // to be written, or ratatui would leave whatever was in
                    // them showing through a transparent sparkline.
                    (None, "spark") if sixel_cell => " ".repeat(TREND),
                    // Blank on a row that is not an entry. The scale is built
                    // from entries, so a sum has no figure on it -- Host would
                    // sit pinned to the ceiling saying only that it is the
                    // total, which the header already draws to scale.
                    (None, "spark") if !r.trimmable => String::new(),
                    (None, "spark") => spark(app.history.get(&r.id), full),
                    _ => (c.fmt)(&name, r.nproc, &r.metrics),
                };
                let cell = Cell::from(text);
                if sixel_cell {
                    cell.style(Style::default().fg(SIXEL_MARK))
                } else if placed.is_some() {
                    // The image id rides in the foreground colour, and it has
                    // to be a ratatui style: a cell's symbol is written
                    // literally, so an escape smuggled into the text would go
                    // to the screen as text.
                    let (r8, g8, b8) = kgp::id_rgb();
                    cell.style(Style::default().fg(Color::Rgb(r8, g8, b8)))
                } else if alarming(c.label, &r.metrics) {
                    cell.style(alarm_style())
                } else {
                    cell
                }
            })
            .skip(skip)
            .take(fit)
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
    let widths: Vec<Constraint> = app
        .cols
        .iter()
        .skip(skip)
        .take(fit)
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
    let table = Table::new(table_rows, widths).header(sort_header(
        app.cols.iter().skip(skip).take(fit),
        &app.view.sort,
    ));
    f.render_widget(table, chunks[2]);
    // After the widget, because the marks only exist once it has filled the
    // buffer; written to the terminal only after ratatui flushes, since sixel
    // paints over cells rather than into them.
    app.sixel_out = None;
    if app.trend == TrendMode::Sixel
        && trend_img
        && let Some(img) = trend_pixels
        && let Some((x, y)) = marked_corner(f.buffer_mut(), chunks[2])
    {
        app.sixel_out = Some(sixel::at(y, x, &sixel::encode(&img, trend_colour())));
    }
    if app.help {
        draw_help(f, chunks[2]);
    } else if app.detail {
        draw_detail(f, chunks[2], rows.get(app.cursor));
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
    // A frozen monitor that does not say it is frozen is how a stale number
    // gets read as current, so the age of the held view is on screen, not just
    // the fact of the pause.
    let paused = app.paused.map_or_else(String::new, |since| {
        format!("  PAUSED {}s", since.elapsed().as_secs())
    });
    let footer = format!(
        " q quit  / filter  c sort ({})  i detail  p pause  s save  ? help{}{}  {}  {}",
        app.view.sort,
        paused,
        crate::once::coverage_tail(&app.tree),
        filter,
        app.status
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
    // Every row's bar is drawn to MEM's, so the brackets stack in one column
    // however many rows there are.
    let mut lines = vec![cpu_header_line(&app.tree, width, mem_bar_w), mem];
    if let Some(swap) = swap_header_line(&app.tree, width, mem_bar_w) {
        lines.push(swap);
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// The SWAP row, or `None` on a host with no swap configured -- where there is
/// no swap for a page to be in, so there is no figure and no row, the same
/// blank contract the `SWAP` column keeps.
fn swap_header_line(tree: &HostTree, width: usize, bar_w: usize) -> Option<Line<'static>> {
    let total = (tree.swap_total_bytes > 0).then_some(tree.swap_total_bytes)?;
    let used = tree.swap_used_bytes.min(total);
    let prefix = bar_prefix("SWAP", label_width(tree));
    let mid = format!("] {}/{}  ", fmt_bytes(Some(used)), fmt_bytes(Some(total)));
    // Drawn to MEM's bar width and padded on the right, the same way the CPU
    // row is, so all three brackets stack in one column. Its own suffix is
    // shorter than MEM's, so there is always slack to give back.
    let fits = width.saturating_sub(prefix.len() + mid.len());
    let bar_w = bar_w.min(fits);
    let mut spans = vec![Span::raw(prefix)];
    spans.extend(stacked_bar(
        bar_w,
        &[(used, Color::Yellow, glyph::full())],
        total.max(1),
    ));
    spans.push(Span::raw(mid));
    spans.push(Span::raw(" ".repeat(fits - bar_w)));
    Some(Line::from(spans))
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
    let prefix = bar_prefix("CPU", label_width(tree));
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
/// Every bar's label, right-aligned to the longest one *this machine draws*,
/// so `[` lands in one column on every header row.
///
/// Aligned to `SWAP` unconditionally instead, a machine with neither swap nor
/// a discrete card -- the ordinary case -- paid a column of bar for a label
/// that was never on screen. The header heft draws there has to stay the one
/// it drew before either existed.
///
/// One helper rather than a literal per row: `cpu_header_line` and
/// `swap_header_line` build their own prefixes and `bar_group` builds the
/// rest, so three copies of this would drift the first time a label changed.
fn bar_prefix(label: &str, label_w: usize) -> String {
    format!(" {label:>label_w$} [")
}

/// Discrete VRAM is a second device, so it is measured against its own
/// capacity rather than MemTotal. Unified (APU) VRAM is a carve-out of
/// MemTotal and stays inside the MEM bar.
fn discrete_vram(tree: &HostTree) -> Option<u64> {
    (!tree.unified_memory)
        .then(|| tree.vram_total_bytes.filter(|v| *v > 0))
        .flatten()
}

/// The longest label on screen. `CPU` and `MEM` are always there; `VRAM` and
/// `SWAP` are a column longer and only sometimes.
fn label_width(tree: &HostTree) -> usize {
    if tree.swap_total_bytes > 0 || discrete_vram(tree).is_some() {
        4
    } else {
        3
    }
}

fn bar_group(
    label: &str,
    label_w: usize,
    width: usize,
    parts: &[(u64, Color, char)],
    used: u64,
    // `total` is both the figure printed after the bar and the scale it is
    // drawn against; they were separate until every caller passed one value
    // twice.
    total: u64,
    labels: &[(&str, Color, char)],
) -> (Vec<Span<'static>>, usize) {
    let prefix = bar_prefix(label, label_w);
    let mid = format!("] {}/{}  ", fmt_bytes(Some(used)), fmt_bytes(Some(total)));
    let (tail, legend_len) = legend(labels);
    let bar_w = width.saturating_sub(prefix.len() + mid.len() + legend_len);
    let mut spans = vec![Span::raw(prefix)];
    spans.extend(stacked_bar(bar_w, parts, total.max(1)));
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
    let discrete = discrete_vram(tree);
    let label_w = label_width(tree);
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
    // Swap has its own row: it is a device rather than a slice of MemTotal,
    // and as a tank here it took half the width, which the CPU bar matches.
    let tanks = tank_widths(width, 1 + usize::from(discrete.is_some()));
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
        label_w,
        mem_width,
        &[
            (seg.vram, Color::LightRed, glyph::quad_a()),
            (seg.gtt, Color::LightCyan, glyph::quad_b()),
            (seg.cache, Color::Blue, glyph::dark()),
            (seg.buffers, Color::Green, glyph::medium()),
            (seg.anon, Color::Gray, glyph::full()),
        ],
        tree.mem_used_bytes,
        tree.mem_total_bytes,
        &labels,
    );
    if let Some(vram_total) = discrete {
        let vram_used = tree.vram_used_bytes.unwrap_or(0).min(vram_total);
        spans.extend(
            bar_group(
                "VRAM",
                label_w,
                tanks[1],
                &[(vram_used, Color::LightRed, glyph::full())],
                vram_used,
                vram_total,
                &[],
            )
            .0,
        );
    }
    (Line::from(spans), mem_bar_w)
}

/// What a VRAM or SWAP tank is given: ` SWAP [` and `] 1.2G/8.0G  ` are about
/// 24 columns of fixed text between them, so this is that plus a bar you can
/// read.
const SIDE_TANK: usize = 32;

/// Split one header row across its bar groups so the parts still sum to
/// `width`; a `width / tanks` each would lose the remainder and shorten the
/// line, which is exactly what the rendered-width assertions catch.
///
/// MEM keeps what the others do not need, rather than an equal share. Split
/// evenly, a machine with swap gave half the row to a tank whose bar says one
/// thing, and because `cpu_header_line` matches its bar to MEM's, the CPU bar
/// was halved along with it -- two headline bars shrunk to make room for a
/// readout that needs 24 columns whatever the terminal is.
///
/// The budget is capped at half the row so a narrow terminal degrades to the
/// even split rather than starving MEM, and the remainder still lands inside
/// it, so the parts sum to `width` exactly.
fn tank_widths(width: usize, tanks: usize) -> Vec<usize> {
    let Some(extras) = tanks.checked_sub(1).filter(|e| *e > 0) else {
        return vec![width];
    };
    let budget = (SIDE_TANK * extras).min(width / 2);
    let mut out = vec![width - budget];
    out.extend((0..extras).scan(budget, |rest, i| {
        let w = *rest / (extras - i);
        *rest -= w;
        Some(w)
    }));
    out
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
    crate::keys::KEYS
        .iter()
        .map(|(k, desc)| {
            let k = k
                .replace("{up}", &up.to_string())
                .replace("{down}", &down.to_string())
                .replace("{left}", &left.to_string())
                .replace("{right}", &right.to_string());
            format!("{k:<20} {desc}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every column for one row, hidden ones included, plus what the row *is*
/// when it is a single process. A narrow terminal shows six of twenty columns
/// and `[` `]` reaches the rest one screen at a time; this reads them all at
/// once for the row under the cursor, which is the question a scroll is
/// usually standing in for. A blank stays blank here for the same reason it
/// does in the table: no figure exists, which is not a zero.
fn detail_text(row: &Flat, avail: usize) -> String {
    let mut lines = vec![row.name.clone(), String::new()];
    lines.extend(metric_grid(row, avail));
    if let Some(pid) = row_pid(&row.id) {
        lines.push(String::new());
        // One per line: these are paths and command lines, and wrapping a
        // cgroup path into a grid cell would make it unreadable.
        for (k, v) in proc::detail(pid) {
            lines.push(format!("{k:<9} {v}"));
        }
    }
    lines.join("\n")
}

/// Column width for one `LABEL value` cell, plus the gutter between cells.
/// `NETNS RX` is the longest header and a disk rate like `31.1M/s` the longest
/// value, so the pair fits in 19 columns.
const CELL: usize = 19;
const GUTTER: usize = 2;

/// The metric pairs laid across as many columns as the pane is wide.
///
/// Stacked one per line they made a twenty-row column of two-character values
/// beside an empty half-screen, and — the real fault — pushed `EXE`, `CGROUP`
/// and `CMDLINE` past the bottom of the pane, where `Paragraph` simply cuts
/// them. Those three are why the pane exists.
///
/// Filled column-major, so reading down a column keeps the compiled order
/// (`PSS` beside `RSS` beside `SWAP`) rather than scattering related columns
/// across a row.
fn metric_grid(row: &Flat, avail: usize) -> Vec<String> {
    let cells: Vec<String> = COLUMNS
        .iter()
        .filter(|c| c.label != "name")
        .map(|c| {
            let v = (c.fmt)(&row.name, row.nproc, &row.metrics);
            // The label shows even when the value is blank: a blank cell is a
            // metric heft could not read, which is a fact worth seeing.
            format!("{:<9} {v:<9}", c.header)
        })
        .collect();
    let per_row = ((avail + GUTTER) / (CELL + GUTTER)).clamp(1, 4);
    let depth = cells.len().div_ceil(per_row);
    (0..depth)
        .map(|r| {
            (0..per_row)
                .filter_map(|c| cells.get(c * depth + r))
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(&" ".repeat(GUTTER))
                .trim_end()
                .to_string()
        })
        .collect()
}

/// A process row's id ends `…/p/<pid>`; every other row is an aggregate and
/// has no single `/proc` entry to describe.
fn row_pid(id: &str) -> Option<u32> {
    id.rsplit_once("/p/")?.1.parse().ok()
}

fn draw_detail(f: &mut ratatui::Frame<'_>, area: Rect, row: Option<&Flat>) {
    let Some(row) = row else { return };
    // Two for the popup border, two for the breathing room `popup` leaves.
    let avail = usize::from(area.width.saturating_sub(4));
    let text = detail_text(row, avail);
    popup(f, area, &text, "detail  (i or Esc to close)");
}

fn draw_help(f: &mut ratatui::Frame<'_>, area: Rect) {
    popup(f, area, &help_text(), "keys");
}

/// Centred, sized to its text, never wider or taller than the pane it covers.
/// One definition for both overlays so they cannot drift apart.
fn popup(f: &mut ratatui::Frame<'_>, area: Rect, text: &str, title: &str) {
    // A border each side and a space of padding inside it: a blank metric cell
    // ends at its label, and without the padding it butts against the frame.
    const CHROME: u16 = 4;
    let cols = u16::try_from(text.lines().map(|l| l.chars().count()).max().unwrap_or(0))
        .unwrap_or(u16::MAX);
    let width = cols
        .saturating_add(CHROME)
        .min(area.width.saturating_sub(2))
        .max(3);
    // A long CMDLINE wraps rather than being cut at the frame, so the height
    // has to count the rows wrapping will actually produce.
    let inner = usize::from(width.saturating_sub(CHROME)).max(1);
    let rows = u16::try_from(
        text.lines()
            .map(|l| l.chars().count().max(1).div_ceil(inner))
            .sum::<usize>(),
    )
    .unwrap_or(u16::MAX);
    let height = rows
        .saturating_add(2)
        .min(area.height.saturating_sub(1))
        .max(3);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(text.to_string())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .padding(Padding::horizontal(1))
                    .title(title.to_string()),
            ),
        rect,
    );
}

/// Body rows that fit in the table pane: the terminal minus the header, the
/// two rules, the column header and the footer.
fn table_body_rows(term_h: u16, header: u16) -> usize {
    term_h.saturating_sub(header + 4) as usize
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

    /// The overlay's whole point is the columns the terminal is too narrow to
    /// show, so it must list every one of them regardless of `hide_columns`,
    /// and reach `/proc` only for a row that is one process.
    #[test]
    fn detail_lists_every_column_and_only_reads_proc_for_a_pid() {
        let mut row = flat(3, "firefox");
        let text = detail_text(&row, 80);
        for c in COLUMNS.iter().filter(|c| c.label != "name") {
            assert!(text.contains(c.header), "{} missing from {text}", c.header);
        }
        assert!(!text.contains("CGROUP"), "an aggregate row has no one pid");

        row.id = "host/apps/firefox/i/1/p/1".into();
        assert_eq!(row_pid(&row.id), Some(1));
        // pid 1 exists on any Linux box the suite runs on, readable or not.
        assert!(detail_text(&row, 80).contains("CGROUP"));
    }

    /// Stacked one per line the pairs ran past the bottom of the pane, cutting
    /// EXE, CGROUP and CMDLINE — the three the pane exists for. A wide pane
    /// must lay them across; a narrow one must still fall back to one column.
    #[test]
    fn detail_metrics_fill_the_width_they_are_given() {
        let row = flat(3, "firefox");
        let deep = |w| detail_text(&row, w).lines().count();
        let wide = deep(150);
        assert!(
            wide < deep(20),
            "a wide pane must be shorter than a narrow one"
        );
        // Twenty metrics over four columns is five rows, plus title and blank.
        assert!(wide <= 8, "expected a compact grid, got {wide} lines");
        for line in detail_text(&row, 150).lines() {
            assert!(line.chars().count() <= 150, "grid overflowed: {line:?}");
        }
    }

    /// The trend is drawn against a scale the whole frame shares, not against
    /// each row's own peak. A row that has been flat at zero has a history and
    /// it is flat; a row with none yet is blank, which is heft's no-figure
    /// cell.
    #[test]
    fn a_trend_draws_against_the_frames_scale() {
        let buf = |v: &[f64]| VecDeque::from(v.to_vec());
        let ramp = glyph::spark_ramp();
        let low = ramp[0];
        let high = ramp[ramp.len() - 1];

        assert_eq!(spark(None, 100.0), "");
        assert_eq!(spark(Some(&buf(&[])), 100.0), "");
        // Steadily zero is flat at the bottom, not blank.
        assert_eq!(spark(Some(&buf(&[0.0, 0.0])), 100.0), format!("{low}{low}"));
        // The reported defect: against its own peak a row flat at 4% drew
        // every sample full height, so most of the column was solid and two
        // rows could not be told apart.
        assert_eq!(
            spark(Some(&buf(&[4.0; 3])), 100.0),
            format!("{low}{low}{low}")
        );
        // 400% and 4% are now different pictures, which is the point.
        assert_eq!(
            spark(Some(&buf(&[0.0, 400.0])), 100.0),
            format!("{low}{high}")
        );
        assert_eq!(spark(Some(&buf(&[0.0, 4.0])), 100.0), format!("{low}{low}"));
        // Full scale tops out, and above it pins rather than rescaling.
        assert_eq!(spark(Some(&buf(&[100.0])), 100.0), high.to_string());
        // Nothing on screen has a figure yet: the floor, not a divide by zero.
        assert_eq!(spark(Some(&buf(&[5.0])), 0.0), low.to_string());
        // One sample per value, so the cell never outgrows the column.
        assert_eq!(
            spark(Some(&buf(&[1.0; TREND])), 100.0).chars().count(),
            TREND
        );
    }

    fn flat_row(id: &str, trimmable: bool) -> Flat {
        Flat {
            id: id.to_string(),
            depth: 0,
            name: id.to_string(),
            nproc: 1,
            metrics: Metrics::default(),
            expandable: false,
            trimmable,
            search: None,
        }
    }

    /// What the terminal answers decides what is drawn, and where two answers
    /// are possible the cheaper transport for that connection wins.
    #[test]
    fn auto_prefers_the_cell_grid_locally_and_the_wire_cost_remotely() {
        let caps = |kitty, sixel| caps::Caps { kitty, sixel };
        // Locally the kitty protocol's pixels go through shared memory, and
        // its image is tied to the cell grid rather than repainted.
        assert_eq!(resolve_trend(caps(true, true), false), TrendMode::Kitty);
        assert_eq!(resolve_trend(caps(true, false), false), TrendMode::Kitty);
        // Over ssh that transport sends every pixel inline: ~146 KB a sample
        // against sixel's 559 bytes a frame.
        assert_eq!(resolve_trend(caps(true, true), true), TrendMode::Sixel);
        // Only one on offer, so the connection does not come into it.
        assert_eq!(resolve_trend(caps(false, true), false), TrendMode::Sixel);
        assert_eq!(resolve_trend(caps(false, true), true), TrendMode::Sixel);
        assert_eq!(resolve_trend(caps(true, false), true), TrendMode::Kitty);
        // A terminal that answered neither gets the character ramp, which is
        // also what a terminal that did not answer at all gets.
        assert_eq!(resolve_trend(caps(false, false), false), TrendMode::Chars);
        assert_eq!(resolve_trend(caps::Caps::default(), true), TrendMode::Chars);
    }

    /// A sum is not on the scale the entries are drawn against, so it gets
    /// heft's blank rather than a mark pinned to the ceiling.
    #[test]
    fn an_aggregate_row_has_no_trend() {
        let history = HashMap::from([
            ("host".to_string(), VecDeque::from(vec![50.0])),
            ("a".to_string(), VecDeque::from(vec![50.0])),
        ]);
        let rows = [flat_row("host", false), flat_row("a", true)];
        let bands: Vec<Option<&VecDeque<f64>>> = rows
            .iter()
            .map(|r| r.trimmable.then(|| history.get(&r.id)).flatten())
            .collect();
        assert!(bands[0].is_none(), "Host is a sum, not an entry");
        assert!(bands[1].is_some());
    }

    /// A percentage is full at 100 whatever else is on screen. Everything else
    /// scales to the heaviest entry -- never to Host, which is the sum of the
    /// machine and would draw every real row flat along the bottom.
    #[test]
    fn the_scale_is_the_metric_s_own_or_the_heaviest_entry() {
        let mut history = HashMap::new();
        history.insert("host".to_string(), VecDeque::from(vec![9_000.0]));
        history.insert("a".to_string(), VecDeque::from(vec![10.0, 40.0]));
        history.insert("b".to_string(), VecDeque::from(vec![25.0]));
        let rows = vec![
            flat_row("host", false),
            flat_row("a", true),
            flat_row("b", true),
        ];
        assert_eq!(
            trend_scale(Sort::from_label("core"), &rows, &history),
            100.0,
            "a percentage does not depend on what else is drawn"
        );
        assert_eq!(
            trend_scale(Sort::from_label("pss"), &rows, &history),
            40.0,
            "the heaviest entry, and Host is not one"
        );
        assert_eq!(
            trend_scale(Sort::from_label("pss"), &rows, &HashMap::new()),
            0.0,
            "nothing measured yet"
        );
    }

    /// A clipped number is a wrong number: at 50 columns ratatui drew `20.1G`
    /// as `2`. Every column the layout keeps must have room for its whole
    /// width, and the ones it drops stay reachable with `[` / `]`.
    #[test]
    fn a_column_is_drawn_whole_or_not_at_all() {
        let cols = Columns::for_tui(&View::default());
        let width_of = |i: usize| cols.iter().nth(i).expect("column").width;
        for avail in [10_u16, 20, 30, 44, 50, 70, 100, 160, 400] {
            let n = columns_that_fit(&cols, 0, avail, true);
            assert!(n >= 1, "at least one column always survives");
            if n < cols.len() {
                // The next column was refused, so it genuinely did not fit.
                let used: u16 = (0..n).map(|i| width_of(i) + u16::from(i > 0)).sum();
                assert!(
                    used + width_of(n) + 1 > avail,
                    "column {n} was dropped but fits in {avail}"
                );
            }
            // Never claims room it does not have, except for the one column
            // that always survives.
            if n > 1 {
                let used: u16 = (0..n).map(|i| width_of(i) + u16::from(i > 0)).sum();
                assert!(used <= avail, "{n} columns overflow {avail}");
            }
        }
        // Wider is never fewer columns.
        let narrow = columns_that_fit(&cols, 0, 50, true);
        let wide = columns_that_fit(&cols, 0, 160, true);
        assert!(wide > narrow, "a wider table must show more columns");
    }

    /// `%CORE` reads a D-state process as idle and a stalled cgroup as quiet,
    /// which is why those columns exist -- and why a figure in them has to
    /// look different from the zeros beside it.
    #[test]
    fn only_a_figure_in_trouble_is_marked() {
        let m = |d: u32, cpu: Option<f64>| Metrics {
            d_state_procs: d,
            cpu_stall_pct: cpu,
            ..Metrics::default()
        };
        assert!(alarming("dstate", &m(1, None)));
        assert!(!alarming("dstate", &m(0, None)));
        // At the threshold, not merely past it.
        assert!(alarming("cpustall", &m(0, Some(STALL_ALARM))));
        assert!(!alarming("cpustall", &m(0, Some(STALL_ALARM - 0.1))));
        // A blank stall is no figure at all, so there is nothing to mark.
        assert!(!alarming("cpustall", &m(0, None)));
        // Every other column is left alone, including a large one.
        assert!(!alarming("core", &m(9, Some(99.0))));
        assert!(!alarming("name", &m(9, Some(99.0))));
    }

    #[test]
    fn help_text_aligns_keys() {
        let text = help_text();
        let lines: Vec<&str> = text.lines().collect();
        assert!(!lines.is_empty());
        assert!(lines.iter().all(|l| !l.is_empty()));
        for line in &lines {
            let chars: Vec<char> = line.chars().collect();
            assert!(chars.len() > 20);
            assert_eq!(chars[20], ' ');
        }
    }

    #[test]
    fn table_body_rows_two_header_no_box() {
        assert_eq!(table_body_rows(24, HEADER_ROWS), 18);
        assert_eq!(table_body_rows(6, HEADER_ROWS), 0);
        assert_eq!(table_body_rows(7, HEADER_ROWS), 1);
        // A machine with swap spends one more row on the header, so the table
        // gets one fewer -- and the arithmetic has to know, or the last row
        // would be drawn under the footer.
        assert_eq!(table_body_rows(24, HEADER_ROWS + 1), 17);
        assert_eq!(table_body_rows(7, HEADER_ROWS + 1), 0);
    }

    /// Swap earns a row of its own; a machine without it draws the two rows
    /// heft always drew, so nothing is spent on a tank that says nothing.
    #[test]
    fn the_header_grows_a_row_only_for_swap() {
        let g = 1024 * 1024 * 1024;
        let mut tree = tree_with_gpu(&mem::GpuPool::default());
        assert_eq!(header_rows(&tree), 2);
        assert!(swap_header_line(&tree, 100, 40).is_none());
        tree.swap_total_bytes = 8 * g;
        tree.swap_used_bytes = 2 * g;
        assert_eq!(header_rows(&tree), 3);
        let swap = swap_header_line(&tree, 100, 40).expect("a row").to_string();
        assert!(swap.starts_with(" SWAP ["), "{swap}");
        assert!(swap.contains("2.0G/8.0G"), "{swap}");
        assert_eq!(swap.chars().count(), 100);
        // Discrete VRAM still shares the MEMORY row: it is a second memory,
        // and its readout belongs beside the one it is being compared with.
        tree.vram_total_bytes = Some(12 * g);
        tree.vram_used_bytes = Some(6 * g);
        let mem = mem_header_line(&tree, 120).0.to_string();
        assert!(mem.contains("VRAM ["), "{mem}");
        assert!(!mem.contains("SWAP"), "swap left this row: {mem}");
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
        assert!(text.contains("▄gtt/▓cache/▒buf"), "{text}");
        assert!(
            !text.contains("vram/"),
            "discrete vram is not a MEM segment"
        );
        assert!(line.spans.iter().any(|s| s.content.contains('▄')), "{text}");
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
        assert!(text.contains("▀vram/▄gtt/▓cache/▒buf"), "{text}");
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
        // MEM keeps the remainder: a side tank needs a fixed 32 whatever the
        // terminal is, and an even split halved the CPU bar along with it.
        assert_eq!(tank_widths(101, 2), vec![69, 32]);
        // Two side tanks want 64, but the half-row cap binds first, so MEM
        // still keeps half rather than a third.
        assert_eq!(tank_widths(120, 3), vec![60, 30, 30]);
        assert_eq!(tank_widths(200, 2), vec![168, 32], "MEM takes the slack");
        // Narrow enough that the fixed budget would starve MEM: back to an
        // even split rather than a bar with nothing in it.
        assert_eq!(tank_widths(40, 2), vec![20, 20]);
        assert_eq!(tank_widths(60, 3), vec![30, 15, 15]);
        assert_eq!(tank_widths(80, 1), vec![80], "no side tanks, no split");
    }

    /// The machine this was written on has `SwapTotal: 0`, and there the header
    /// must be byte-identical to the one heft printed before swap existed.
    #[test]
    fn a_swapless_host_gets_no_swap_tank() {
        let tree = tree_with_gpu(&mem::GpuPool::default());
        assert_eq!(tree.swap_total_bytes, 0);
        let text = mem_header_line(&tree, 100).0.to_string();
        assert!(!text.contains("SWAP"), "{text}");
        assert!(text.starts_with(" MEM ["), "no swap, no padding: {text}");
        assert_eq!(text.chars().count(), 100);
    }

    /// Labels are right-aligned to the longest, so the opening bracket lands
    /// in one column on every row: a bar that starts a column further along
    /// than the one above it reads as a different scale.
    ///
    /// To the longest *on screen*, though. Most machines have neither swap nor
    /// a discrete card, and there the header must be the one heft drew before
    /// either existed rather than one carrying a column of padding for a label
    /// that is not drawn.
    #[test]
    fn a_plain_machine_pads_nothing() {
        let tree = tree_with_gpu(&mem::GpuPool::default());
        assert_eq!(tree.swap_total_bytes, 0);
        assert_eq!(label_width(&tree), 3);
        let (mem, bar_w) = mem_header_line(&tree, 120);
        assert!(mem.to_string().starts_with(" MEM ["), "{mem}");
        let cpu = cpu_header_line(&tree, 120, bar_w).to_string();
        assert!(cpu.starts_with(" CPU ["), "{cpu}");
        assert_eq!(mem.to_string().find('[').unwrap(), 5);
        assert_eq!(cpu.find('[').unwrap(), 5);
        // A discrete card brings VRAM, which is as long as SWAP, so the pad
        // arrives for that too rather than only for swap.
        let mut vram = tree.clone();
        vram.unified_memory = false;
        vram.vram_total_bytes = Some(12 * 1024 * 1024 * 1024);
        assert_eq!(label_width(&vram), 4);
        assert!(
            mem_header_line(&vram, 120)
                .0
                .to_string()
                .starts_with("  MEM [")
        );
    }

    #[test]
    fn every_header_bar_opens_in_the_same_column() {
        let g = 1024 * 1024 * 1024;
        let mut tree = tree_with_gpu(&mem::GpuPool::default());
        tree.swap_total_bytes = 8 * g;
        tree.swap_used_bytes = 2 * g;
        let (mem, bar_w) = mem_header_line(&tree, 120);
        let rows = [
            cpu_header_line(&tree, 120, bar_w).to_string(),
            mem.to_string(),
            swap_header_line(&tree, 120, bar_w)
                .expect("a row")
                .to_string(),
        ];
        let opens: Vec<usize> = rows.iter().map(|r| r.find('[').expect("a bar")).collect();
        assert_eq!(opens, vec![6, 6, 6], "{rows:#?}");
        // And the closing brackets already stacked, which is what the shared
        // bar width is for; both ends line up now.
        let closes: Vec<usize> = rows.iter().map(|r| r.find(']').expect("a bar")).collect();
        assert_eq!(closes[0], closes[1]);
        assert_eq!(closes[1], closes[2]);
    }

    /// Swap is a device, not a slice of MemTotal: painting it inside the MEM
    /// bar would put pages that are not in RAM into the RAM figure.
    #[test]
    fn swap_is_never_a_segment_of_the_mem_bar() {
        let g = 1024 * 1024 * 1024;
        let mut tree = tree_with_gpu(&mem::GpuPool::default());
        tree.swap_total_bytes = 8 * g;
        tree.swap_used_bytes = 2 * g;
        let mem = mem_header_line(&tree, 100).0.to_string();
        assert!(mem.contains("8.0G/32.0G"), "RAM only: {mem}");
        assert!(!mem.contains("SWAP"), "{mem}");
        assert_eq!(mem.chars().count(), 100);
        let swap = swap_header_line(&tree, 100, 40).expect("a row").to_string();
        assert!(swap.contains("2.0G/8.0G"), "{swap}");
        assert_eq!(swap.chars().count(), 100);
    }

    /// The reported defect: swap shared the MEMORY row, so it took half the
    /// width, and because the CPU bar is sized to match MEM's, both headline
    /// bars halved to make room for a readout that needs the same couple of
    /// dozen columns at any terminal size. It has its own row now, and the
    /// two bars above it are the width they would be on a swapless machine.
    #[test]
    fn swap_costs_the_mem_and_cpu_bars_nothing() {
        let g = 1024 * 1024 * 1024;
        let mut swapped = tree_with_gpu(&mem::GpuPool::default());
        let plain = mem_header_line(&swapped, 120).1;
        swapped.swap_total_bytes = 8 * g;
        swapped.swap_used_bytes = 2 * g;
        let with_swap = mem_header_line(&swapped, 120).1;
        // One column, and that one is the label pad that lines `[` up with
        // SWAP's -- not a share of the row. The even split made it 24.
        assert_eq!(with_swap, plain - 1, "MEM went {plain} -> {with_swap}");
        // All three brackets stack in one column.
        let cpu = cpu_header_line(&swapped, 120, with_swap).to_string();
        let swap = swap_header_line(&swapped, 120, with_swap).expect("a row");
        let bar_of = |s: &str| {
            s.split_once('[')
                .unwrap()
                .1
                .split_once(']')
                .unwrap()
                .0
                .chars()
                .count()
        };
        assert_eq!(bar_of(&cpu), with_swap);
        assert_eq!(bar_of(&swap.to_string()), with_swap);
        assert_eq!(cpu.chars().count(), 120);
        assert_eq!(swap.to_string().chars().count(), 120);
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
            detail: false,
            paused: None,
            history: HashMap::new(),
            kgp: None,
            trend: TrendMode::Chars,
            sixel_out: None,
            sorted_by: String::new(),
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
                ..UserNode::default()
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
                ..UserNode::default()
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
