# heft — agent guide

Read-only Linux process monitor. Binary name `heft`.

## Start here

@HUMANS.md — install, keys, XDG, sample cadence, live verify.

## Layout

```
src/cli.rs           clap Cli; build.rs includes it so completions/man cannot drift
src/main.rs          dispatch: TUI default, --once, --json
src/lib.rs           modules
build.rs             clap_complete + clap_mangen → OUT_DIR/assets (build-deps only); HEFT_VERSION = version~sha
.git-sha             export-subst commit for tag tarballs, which have no .git for build.rs to ask
demo.tape            vhs script for README.md's demo.gif; regenerate with `vhs demo.tape`
src/types.rs         Process, Metrics, HostTree, JSON shape
src/proc.rs          every visible PID; blank metrics on EACCES
src/cpu.rs           /proc/stat split (usr/sys/wait) + per-pid utime/stime rates
src/mem.rs           meminfo used/Shmem/kernel/cache/Swap, zram mm_stat, unified APU clip, host VRAM
src/io.rs            /proc/pid/io rates and smaps_rollup PSS + SwapPss
src/net.rs           per-netns rx/tx from /proc/pid/net/dev; container rows only
src/psi.rs           cgroup cpu/io/memory.pressure; single-cgroup rows only
src/gpu.rs           amdgpu/i915/xe fdinfo; dri/drm prefilter; no empty-prefilter walk; oversized fdinfo skipped; drm-client-id dedupe
src/classify.rs      launcher / worker / shell / terminal / compositor tables
src/identity.rs      cgroup parse, merge key + display name
src/containers.rs    GET-only docker/podman; project vs per-container
src/group.rs         Host → User → Applications | User Services | Containers, System
src/config.rs        XDG view.json (sort, filter, hide_columns, column_order; write on save) and grouping.json (read-only); both accept // and /* */
src/once.rs          columns, tree ordering, table and JSON
src/explain.rs       --explain PID: resolved placement and the grouping.json key
src/ui.rs            ratatui header + tree table
src/tty.rs           panic hook + signal handler; restores the terminal
src/glyph.rs         unicode vs ascii bar/rule/marker characters; resolved once
src/keys.rs          the one TUI key list; build.rs includes it for the man page
src/root.rs          the /proc and /sys prefix behind --proc-root, resolved once
src/caps.rs          --trend auto: one round trip asking the terminal what it draws
src/kgp.rs           --trend kitty: TREND as one graphics-protocol image; shm or inline
src/sixel.rs         --trend sixel: the same image, RLE sixel, positioned from the frame
tests/grouping.rs    integration tests over tests/fixtures/; links the library
tests/live_proc.rs   invariants over the real /proc; must hold in a bare container
tests/reconcile.rs   heft against /proc read independently, not against itself
tests/common/mod.rs  heft() and arr() for the two that drive the built binary
tests/fixtures/      GUI grouping snapshot
packaging/aur/       PKGBUILD + .SRCINFO for heft, heft-bin, heft-git; update.sh
```

No `sysinfo` crate. No `nix` unless rustix cannot do it; v1 uses `std` + `libc`.
Never read `/proc/pid/mem`. Never ptrace.

The hand-rolled helpers — the `/proc` and fdinfo field parsers,
`once::scale_1024`, `ui::share_cells`, `identity::systemd_unescape`, and the
GET-only HTTP client in `containers::unix_get` — have no equivalent in std
*or in a crate already in the tree*. Both halves are checked, not assumed, so
replacing them is not pending work. The second half is the one that matters:
`once::trunc` sat in this list on the strength of the first, and was
re-implementing `unicode-truncate`, which ratatui-core had already compiled.
`share_cells` survives it — a partial-fill allocator whose result is meant to
come up short, which no ratatui `Constraint` expresses — and `unix_get`
survives it because the tree carries no HTTP client at all.

Two further consolidations are measured and refused. The `once` and `ui`
walkers look duplicated — around 40 of 54 lines match — but the row types do
not: `Flat` carries an `id` and an `expandable` flag that `TableRow` has no
use for, and that `id` is what keys expand membership, the trend history and
the cursor's re-anchoring across a resort. Unifying them takes four parameters
that are each constant on one caller, and makes `--once` and `--follow`
`format!` an id per row and throw it away. That the row sets differ is the
consequence — it is why `--filter` is narrower than `/` — not the reason.

`containers::index_ids` keys both the full and the 12-hex id while `get` also falls back through `hex12`; the
redundancy costs two lines and the alternative silently loses a row if a
runtime ever reports a truncated id.

## Grouping invariants

- **Host** is the machine, not the compositor or a terminal.
- Bucket (`src/group.rs`): docker/libpod scope or helper that names that id →
  Containers; `identity::is_kernel` or leftover `system.slice`
  (`in_system_slice` and not `in_user_slice`) → System; else that uid's User.
- Under a User: user-instance unit `*.service` not starting with `app-` →
  User Services; else Applications. `init.scope` + `systemd --user` is a user
  service. Known compositors (`classify::is_compositor`) are user services even
  if the unit looks like an app.
- `identity::unit_line` picks the one cgroup line a unit name is read from:
  the `0::` line on v2, else `1:name=systemd:`. A v1 file is a line per
  controller ending in an empty `0::/`, so taking the leaf of the whole blob
  read that empty line and `user_unit` returned `None` — which left User
  Services empty on every v1 host, since that split needs a unit name.
- Display name is `classify::name_of` (`exe` basename else `comm`), not the
  inherited cgroup. The exception is an `exe` of `tdeinit`, which runs
  programs as in-process modules, so there `comm` names the program. TDE
  session processes merge as `tdeinit` (`classify::trinity_session`, checked
  before Plasma because both ship `kded` and `ksmserver`). An editor's
  install or extension tree bills its binaries to that editor
  (`classify::bundled_helper_app`), by `exe` path rather than PPID so a real
  app started from its terminal keeps its row. `identity::lying_unit` skips terminal transients, Chromium
  toolkit scopes (`org.chromium.chromium`), `dbus:` activation, `run-u*`, and
  `flatpak-session-helper` for unit-based identity. `instance_key` uses a real
  user unit only when it is not lying; otherwise `pgid`.
- Launchers (`classify.rs` `LAUNCHERS`, plus names ending `.appimage`) have no
  top-level row; cost bills to the unique payload identity; they still appear
  inside the expanded process list. Nested bwrap folds into the payload. `cat`
  under a launcher or app bills to that parent; it does not break unique-payload
  folding and does not become its own row.
- Workers (`classify::is_worker`) fold into that app. Walk ancestors skipping
  launchers and other generics; do not invent a script-basename identity
  (`context7-mcp`) when a launching agent (`claude`, `cursor`) is above.
  Processes stay visible on expand. `crash_helper_app` matches exe/cmdline
  even when PPID is user systemd. The mount directory itself under `/tmp`,
  `/var/tmp`, or `/run` (`/tmp/mount`, `/tmp/.mount_cursorAb12Cd`) is per-run
  and never an identity; a stable directory nested under it
  (`…/usr/share/cursor/chrome_crashpad_handler`) names the app the same way
  `/opt/cursor/…` does. A helper whose parent dir is the mount falls back
  to the ancestor walk.
- User Services grouping is one identity for processes that share a systemd
  unit family, RPM/package family, D-Bus well-known name family, or documented
  process architecture — not a comm prefix. Mappings live in
  `classify::session_helper_ident`. Exceptions: prefix lookalikes with a
  different product stay out (`gsd-disk-utility-notify`, independent `wsdd`,
  `wireplumber`, `krunner`, `plasma-discover`, `kwindowprop`); independent apps
  never fold into gnome-shell, plasmashell, kwin or these service identities; an arbitrary user CLI is Applications;
  `p11-kit` must not fold into `flatpak-session-helper` (Cursor shares that
  cgroup); user-session `dbus-broker` is User Services, never Applications
  (`lying_unit` matches `dbus:` activation, not `dbus-broker.service`);
  app-bound `xdg-dbus-proxy` bills to that app, unbound folds into `flatpak`;
  `gcr-ssh-agent` absorbs `ssh-agent` only in that unit.
- Split when the child's resolved identity differs and the child is a real app.
  Idle interactive shells fold into their terminal (`classify::is_terminal`);
  a unique payload child (claude, dstat) takes the owning shell, the same
  walk as a launcher. A shell with no terminal parent and no unique payload
  stays Applications.
- Generic interpreters (`classify.rs` `GENERICS`) fall back to the user unit or
  a distinctive script basename (`identity::generic_fallback`) so they do not
  collapse into one interpreter row. Non-distinctive script basenames live in
  that function.
- Containers: never System, never `dockerd`/`containerd`. Project key is
  `com.supabase.cli.project` → `supabase:<name>`, else
  `supabase_<role>_<project>` names, else `com.docker.compose.project`. No
  project → one row per container name; a vendor-specific label is not a merge
  key. `containerd-shim-runc-v2 -id`, `docker-proxy -container-ip`, `conmon`,
  `runc`/`crun` bill to that container. Owner:
  workdir path uid, else the first non-root uid owning an `Inspect.Mounts`
  bind source (named volumes are root-owned and skipped), else Host →
  Containers.
- NETNS RX/TX is the one metric a container row carries and no other row can.
  `/proc/pid/net/dev` is per network namespace, so `net::netns_pids` reads it
  only through the lowest pid in that container's own cgroup scope: the shim,
  `conmon` and `docker-proxy` are billed to the container but run in the root
  namespace. A container is skipped when `HostConfig.NetworkMode` is `host` or
  absent (`Inspect::owns_netns`), because that namespace is the machine's.
  `Metrics::accumulate` never sums the pair, so a folder, User or Host row
  stays blank rather than reporting one namespace as its own. The three
  `*_stall_pct` fields are left out of `accumulate` for the same reason;
  `types.rs` guards both in one test.

## Surfaces and flags

`proc::sample_stream` primes once and then publishes every `--interval`,
honouring `--pss-interval` where the one-shot surfaces force PSS: a stream is
continuous, so it is the TUI's cadence question, not `--once`'s. `once::
follow_table` and `once::follow_json` share their rendering with the one-shot
pair through `render_table` and `json_text`; `--json --follow` is compact
NDJSON, one document per line, because a record that spans lines is not a
record.

`main` resolves one `View` and hands it to whichever surface runs, so
precedence lives in one place: `--sort`, `--asc` / `--desc`, `--filter`,
`--user`, `--top` and `--order` overwrite whatever the saved view held,
`--hide` adds to its list, and only `sort`, `desc`, `filter`, `hide_columns` and `column_order` are
ever read back from it — `users` and `top` are `#[serde(skip)]`. `--once`
starts from `config::load_view()`; `--json` starts from `View::default()` and
never reads the file, because a human's saved preference must not reshape a
documented contract. `--filter` and `--top` are `conflicts_with = "json"`
because the JSON tree has no folder rows, so "keep the ancestors" has nothing
to mean there. `--hide` and `--order` are too: a column preference is not
part of the contract.

An unknown `--sort` label is a clap `InvalidValue` exit, not
`Sort::from_label`'s fallback — a stale `view.json` must not stop the monitor,
an argument just typed can still be corrected. `once::sort_labels` feeds that
error; `src/cli.rs` cannot reach `COLUMNS` because `build.rs` includes it
standalone to generate the completions and man page.

`main` rejects a non-terminal stdout before sampling: the TUI cannot open a
terminal it has not got, and reporting that after a `/proc` walk would burn an
`--interval` first. Non-zero, and never a silent fall back to `--once`. The
`--follow` usage check runs ahead of even that, because a flag combination that
cannot mean anything is wrong wherever stdout points.

`once::keep_users` prunes User nodes before rows are built, on all three
surfaces, so the Host row totals what survived; it is not `conflicts_with =
"json"` the way `--filter` is, because the JSON tree does have User nodes.
`View::users` is `#[serde(skip)]` — it rides the existing plumbing without ever
reaching `view.json`.

`once::Filter` compiles the pattern once — `regex-lite`, not `regex`, measured:
the full engine takes the stripped binary from 1.53 MB to 2.93 MB and adds four
crates for a SIMD literal search that matches a few hundred names a tick.
`(?i)` is prefixed so a saved substring filter keeps behaving as it did.
`Filter::new` returns `None` rather than an error, and each caller decides what
that means: clap `InvalidValue` for `--filter`, warn-and-ignore for a stale
`view.json`, and in the TUI the last compiling pattern stays live behind a `?`
in the footer.

`once::keep_top` trims to `--top` after `keep_matches`, generic over the row
type for the same reason: the TUI trims its flattened rows and `--once` trims
the ones it prints. Row-level rather than tree-level so an ancestor keeps the
total it was built with, and `TableRow::trimmable` / `Flat::trimmable` is false
on Host, User and folder rows — sort never orders `tree.users` or the top-level
folders, so a "top" of them would cut arbitrarily. `conflicts_with = "json"`,
same reasoning as `--filter`.

`--filter` and `/` match a row's `search` haystack when one is built: the row
title followed by the argv of every process beneath it, one per line so `$`
cannot run off one process's arguments into the next. `TableRow::search` and
`Flat::search` are `None` on a tick with no filter and on rows the argv cannot
reach (Host, User, folder headers), so `keep_matches` falls back to `name` and
an unfiltered tick allocates nothing for it. `ProcNode::cmdline` carries the
argv `proc` already read; it is `#[serde(skip)]` because the JSON shape is a
contract and a consumer wanting argv can read `/proc/<pid>/cmdline` itself.
The haystack is built for a collapsed identity too, or `/` would reach only
what happens to be expanded.

`once::keep_matches` is the one filter for every surface — the TUI over its
flattened rows, `--once` over `once::table_rows`. Rows are built from the whole
tree and filtered afterwards, so an ancestor row keeps the total it was built
with rather than the total of what survived.

The two walkers do not emit the same rows, deliberately: `ui::push_folder`
descends folder → identity → instance → process, gated on what is expanded,
while `once::push_folder` stops at folder → identity → member container. So
`--filter` searches strictly fewer rows than `/` does, and the filter is the
same function over two different row sets rather than one behaviour on two
surfaces.

`glyph` resolves the character set once in `main` into a `OnceLock` rather
than threading it through every render site, since it cannot change while heft
runs. `Set::Legacy` differs from `Set::Unicode` in `spark_ramp` and nowhere
else: every other glyph heft draws is one a legacy font carries, which is what
keeps the third set a branch rather than a second table. That holds only
because the bar fills are the shade ramp `█▓▒` and `collapsed` is `►` (U+25BA) rather than `▶` (U+25B6) — U+25B6 is the
play-button emoji base, so a terminal resolving emoji presentation draws it
double-width and shifts a row whose every column is exact. `glyph::tests`
asserts that separation, and caught that U+2584 is both a half block and the
ramp's midpoint, so the gap a legacy font leaves is six steps, not seven.
`detect` never returns `Legacy`: a locale says the terminal can encode UTF-8,
never what the font can draw. Every ASCII substitute is one column wide: the header lines are built to
land on an exact width and `once::trunc` cuts to an exact column count, so a
three-character `...` for `…` would overflow both. `Cli::Glyphs` lives in
`src/cli.rs` because `build.rs` compiles that file standalone.

`--trend kitty` draws TREND as a kitty-graphics-protocol image instead of the
ramp (`src/kgp.rs`). One image for the whole column, never one per row: the
protocol's row diacritics index into an image, so nine cells of one band cost
the same transmission as the whole table, and there is no per-row image
lifecycle to leak. `a=T,U=1` transmits and creates the virtual placement in
one escape; each row's cell is `U+10EEEE` plus its row and column diacritics,
with the image id in the foreground colour as a ratatui style rather than an
escape in the text, because a cell's symbol is written literally. Only the
first of the nine cells spells out its position; the rest inherit from the
left, which the protocol allows when the colours match.

Two transports, chosen from `SSH_CONNECTION` / `SSH_TTY` rather than from
`TERM`: `t=s` hands over a POSIX shared memory object and costs tens of bytes
a frame, and `t=d` sends the pixels inline for the case the terminal is not on
this machine. Measured on a 24-row terminal with 10x20 cells, 19 visible rows:
~146 KB per transmission inline, once per sample. Per *frame* it would be
twenty times that, which is why `Kgp::send` hashes the pixels and returns
without writing when nothing changed — the loop draws on every poll timeout,
not once per sample.

`tty::hold_shm` / `release_shm` exist for that transport alone: the terminal
unlinks the object once it has read it, but a terminal that never reads one
leaves it in `/dev/shm`, and a signal would otherwise kill heft before
`teardown` ran. The handler calls `unlink`, which is async-signal-safe, on a
path built ahead of time, because `shm_unlink` is not on that list and on
Linux is this call anyway.

`--trend auto` is the default and does the handshake heft otherwise avoids,
because this is the one rendering question with no free answer: `TERM` names a
terminal and not what it implements, and a multiplexer or ssh hop can remove a
capability underneath it. `caps::probe` writes the kitty `a=q` query and a DA1
request together, after raw mode and inside the alternate screen so a reply is
neither echoed nor left on the user's scrollback. DA1 is the sentinel that
makes the read terminable — universally answered, and answered last, so its
arrival proves the graphics query has been dealt with; without it every start
on a non-graphics terminal would wait out the timeout. Only a terminal that
answers neither does, at 400ms. `caps::drain` then swallows a late reply,
because a DA1 answer carries `?` and `?` opens the help overlay.
`ui::resolve_trend` prefers kitty locally (cell-grid placement, shm transport)
and sixel when `kgp::is_remote`, on the measured 146 KB-a-sample against
559-bytes-a-frame gap. An explicit `--trend` value asks nothing. It is also
`conflicts_with` `--once` and `--json`, which have no history to draw and are
not drawing to a terminal. A terminal that reports no `ws_xpixel` cannot have
an image sized for it, so `cell_px` returns `None` and the frame falls back to
the ramp rather than blanking the column.

`--trend sixel` (`src/sixel.rs`) sends the same `kgp::paint` image to the
terminals the kitty protocol misses. Two things differ. Sixel has no
placeholder mechanism, so the image must be positioned: the TREND cells are
rendered as spaces carrying `SIXEL_MARK` and `ui::marked_corner` reads the
rectangle back out of the frame buffer after `render_widget` — computing it
would be a second copy of ratatui's column layout, and TREND's width moves
with the table's slack. And it is written after `terminal.draw` returns rather than
inside the closure, because sixel paints over cells instead of into them, so a
cell ratatui rewrites erases the pixels; it is repainted every frame rather
than hashed. That is affordable where the kitty inline transport was not:
measured on an 18-row column, 559 bytes a frame against roughly 146 KB a
sample for `f=32` inline, because a line is mostly empty and sixel
run-length-encodes the empty part. `P2=1` in the introducer is what keeps the
zero pixels transparent so the cursor highlight still shows.

`src/keys.rs` is the one TUI key list. `build.rs` includes it the way it
includes `src/cli.rs`, so the man page's KEYS section and the `?` overlay are
the same table — they had already drifted once, when `i` reached only the
overlay. `{up}`/`{down}`/`{left}`/`{right}` are placeholders: the TUI
substitutes the resolved `glyph` arrows, the man page spells them out. The man
page is rendered piecewise in `build.rs` rather than through `generate_to`, so
KEYS, FILES and ENVIRONMENT land between OPTIONS and VERSION.

`p` sets `App::paused` to an `Instant`; the loop still drains the sampler's
slot but skips the swap into `tree`, so the slot never backs up and unpausing
shows the current machine. The footer prints how long the view has been held,
because a frozen monitor that does not say so reads as a live one.

`i` toggles `ui::draw_detail`, which renders `COLUMNS` in full for the selected
row — hidden columns included, since the pane exists to answer what the table
is too narrow to show — and then, when `ui::row_pid` finds a `…/p/<pid>` id,
the seven `/proc` facts from `proc::detail`. The metrics go through
`ui::metric_grid`, column-major across as many 19-column cells as the pane is
wide: stacked one per line they were a twenty-row column of two-character
values beside an empty half-screen, and pushed `EXE`, `CGROUP` and `CMDLINE`
past the bottom, where `Paragraph` cuts them. `proc::detail` caps the command
line at 240 chars for the same reason. Those are read on the keypress
rather than carried on `ProcNode`: five more strings per process per tick would
be paid on every tick to serve one row of one keystroke. `ui::popup` is shared
with `draw_help` so the two overlays cannot drift, and only one draws at a
time, which is why `i` clears `help`.

## Columns

`ui::trend_scale` gives a frame one scale for every row: `Sort::trend_full`
where the metric is a percentage, else the largest history among `trimmable`
rows — the same "is this an entry" test `--top` uses, which is what keeps Host
out of it, since scaling against the machine's own sum draws every real row
flat on the floor. Aggregate rows still draw and pin to the top. Against each
row's own peak instead, which is what this did first, a row flat at 2% had
every sample equal to its own maximum and drew nine full-height marks, so most
of the column was solid and two rows could not be compared. `kgp::sample_y`
and `ui::spark` share that scale so the image and the ramp say the same thing;
`kgp` draws a line joined to the previous sample rather than a filled bar.

`spark` is the one column whose cell is not a function of the current sample,
so its `Column::fmt` returns empty and `ui::draw` substitutes `ui::spark` from
`App::history` — a `VecDeque` per row id, `App::trend_w` deep (`name` first
grows to the longest row title in the tree, TREND takes the slack after that up to
`TREND_MAX`, and `name` absorbs whatever is left), appended once per
published sample rather than once per frame. `Sort::value` gives the buffer the
same number the sort uses, so the two cannot disagree. The buffers clear when
the sort label changes (two units in one picture) and rows that stop appearing
are dropped on the same pass. `Columns::for_tui` is the only constructor that
includes it: `--once` and `--json` take two walks, so a permanently blank
column there would be noise rather than heft's blank contract. `key: None`, and
`Sort::next` skips a key-less column that is not `name` — a trend has no order.
`--order` validates against `column_labels()` rather than `sort_labels()`,
since moving a column is presentation and not everything movable is sortable.

`once::COLUMNS` is the one column model; `once::Columns` is that list with
`view.hide_columns` and `view.column_order` applied. The TUI and `--once`
resolve it at start; the TUI rebuilds it when `H` / `u` change the list, so
no render site branches on visibility. Listed order labels come first, in that
sequence; unlisted keep compiled order after them; `name` stays first unless
the list names it. `name` is refused for hide and an unknown label in
`view.json` warns (`config::view_path()` named); an unknown `--hide` or
`--order` is a clap `InvalidValue`, the same split as `--sort`. `Sort::next`
cycles over the visible list only, which is also how `H` picks the next sort
so hiding PSS lands on RSS in the default table rather than `name`.
`View::default` and an absent `hide_columns` key hide the stall trio
(`config::default_hidden`); `--hide` extends the list rather than replacing
it, so `--hide vram` cannot bring them back.

Visibility is a **view** preference, so it lives in `view.json` beside sort and
filter, never in the read-only `grouping.json`, which is about identity. It
never reaches sampling: heft reads `/proc` files, not columns, so the roll-up
invariants in `tests/live_proc.rs` and `tests/reconcile.rs` are untouched.
`--json` ignores it — a consumer parsing the tree did not ask for a human's
column preference, and the JSON shape is a contract.

## Grouping overrides

`config::strip_comments` blanks `//` and `/* */` out of both config files
before serde sees them, hand-rolled rather than a JSON5 crate for the reason
the base64 encoder and the `/proc` parsers are. Comment bytes become spaces
and a block comment keeps its newlines, so a serde error still names the line
and column of the file the user is editing, and a `//` inside a string is left
alone — a saved filter is a regex, and `https?://` is a pattern. `save_view`
writes `view_header()` above the object, whose label list comes from
`once::column_labels()` so it cannot drift; a save rewrites the file whole, so
only `grouping.json`, which heft never writes, keeps a user's own comments.

`$XDG_CONFIG_HOME/heft/grouping.json` moves local names out of the compiled
tables. Heft never writes it and never creates the directory for it; absent
means today's behaviour exactly. Keys are tree identities — the row title, not
a pid, comm, or unit: `applications` / `user_services` pin an identity to a
folder, `fold` re-keys one identity onto another, `container_owners` maps a
container name to a uid.

Order is container and System bucketing, then every built-in table, then the
user's `fold`, then the user's folder pin (`group::override_place`, run on the
finished `Place`). So an override beats any built-in table, and cannot reach a
container or a kernel thread: an override naming a System or Containers row is
**ignored with no message**, because that check runs per process per tick and a
warning there would repeat every second. `container_owners` is consulted before
workdir and bind-mount inference in `containers::insert_resolved`.

Malformed JSON or an unknown key (`serde(deny_unknown_fields)`) warns once on
stderr from `config::load_overrides` and grouping continues built-in.

`--explain <PID>` (`src/explain.rs`) reports the resolved placement and the
identity that is the override key, because a wrong key is silent: an identity
matching nothing is never consulted, and nothing else in heft shows what a
process resolved to. It reads the verdict off a built tree rather than
narrating which rule fired — a reason string threaded through grouping would
be paid per process per tick to serve one invocation. `locate` recurses
through `ProcNode::children`, since the pid asked about is usually a folded
worker rather than a top-level entry, and it offers no key for a container or
System row because `override_place` cannot move one. It is exempt from
`main`'s stdout-is-a-terminal check for the same reason `--once` and `--json`
are.

`--fixture` (`proc::print_fixture`) is the other half: a grouping report from
a desktop heft has never run on arrives as the `tests/grouping.rs` fixture
shape, so the fix lands with that machine as its test.
`a_fixture_dump_loads_as_a_fixture` holds the output to that loader. Add a
field there when grouping starts reading one, or reports stop reproducing.

## Sampler

| metric | formula / source |
| --- | --- |
| `%core` | `100 * Δ(utime+stime) / (CLK_TCK * dt)` (can exceed 100) |
| `%machine` | `%core / nproc` |
| RSS | `/proc/pid/statm` |
| PSS | `/proc/pid/smaps_rollup` — cadence in [HUMANS.md](HUMANS.md) |
| SWAP | `SwapPss:` from that same rollup read, so it costs no extra file and shares the PSS cadence. `SwapPss`, never `Swap`: a shared swapped page must be apportioned or a summed tree reports it once per mapper. Blank on a `SwapTotal: 0` host |
| Host swap | `SwapTotal` − `SwapFree` from `/proc/meminfo` (`SwapCached` is neither, so it is not subtracted) |
| THR | `num_threads`, field 20 of the `/proc/pid/stat` already parsed for utime/stime. Sums up the tree the way `nproc` does |
| AGE | `now - (btime + starttime / CLK_TCK)`; `starttime` is field 22 of that same `stat`, `btime` is read from `/proc/stat` once per run and pinned. Aggregates take the OLDEST, never a sum: a duration summed is meaningless, and a max cannot be read as a total |
| Disk R/W | Δ `read_bytes` / `write_bytes` from `/proc/pid/io` |
| GPU mem | first tier the client publishes of `drm-resident-*`, `drm-total-*`, `drm-memory-*` (`gpu::MEM_PREFIXES`); regions `vram`/`gtt` (amdgpu), `local0`/`system0` (i915), `vram0`/`gtt` (xe). `drm-memory-*` is amdgpu's pre-standard pair and the only memory key a kernel older than its `drm_show_memory_stats` switch prints (measured on a 4750G), while `drm-engine-gfx` is there either way — matching only `drm-resident-*` left those hosts with gfx%/compute% beside a blank VRAM and GTT |
| NETNS RX/TX | Δ non-`lo` bytes from `/proc/<container-scope-pid>/net/dev`; a new pid or a counter that went backwards discards the interval |
| CPU/IO/MEM ST | Δ `some ... total=` microseconds from that cgroup's `{cpu,io,memory}.pressure` over wall clock. `some`, not `full`: `full` on a one-process cgroup is the same number twice. A counter that went backwards (a recreated cgroup) discards the interval |
| Host `psi` | `some avg10` from `/proc/pressure/{cpu,io,memory}`, on the `--once` and `--json` host line only. Deliberately a different time base from the columns — a machine-wide trend reads better smoothed, a row's rate has to match the `%core` and disk rates beside it. `psi::header_tail` is not in the TUI header: `ui::cpu_header_line` sized its bar around the tail, so the CPU bar came up short of the MEMORY bar on every PSI-capable kernel |
| gfx% / compute% | `drm-engine-gfx`/`-render` and `-compute` ns deltas over wall clock. xe has no ns key: `drm-cycles-rcs`/`-ccs` delta over the `drm-total-cycles-*` GPU-clock delta, each divided by `drm-engine-capacity-*`. Two formulas, deliberately not unified |

| surface | rule |
| --- | --- |
| CPU bar | `/proc/stat` Δ user+nice / system+irq+softirq / iowait; idle+steal unfilled |
| MEM bar | one MemTotal width when APU VRAM is unified; `ui::mem_key` order inside `used`: VRAM (unified only) and GTT from the kernel's `mem_info_*_used` where the driver has one, else the visible drm clients; zram `mem_used_total`, Shmem, kernel (`SUnreclaim` + `PageTables` + `KernelStack`), `AnonPages`, then `other` for the unitemised rest; then page cache (`Cached` − `Shmem`), `SReclaimable` and `Buffers` beyond `used`, never inside it (`mem::clip_used` has the measurement); unique colour per segment, full-height fills alternating `█▓▒`, no legend on the row — `ui::bar_key` draws the swatches in `?`; clip so the stack never exceeds `used.min(MemTotal)` (`mem::clip_used`) |
| Discrete VRAM | own tank against `mem_info_vram_total`, sharing the MEMORY row with the MEM bar; only `vram` drops out of the MEM segments — GTT is pinned system RAM and stays in MEM |
| Swap | own tank against `SwapTotal`, never a MEM segment: swapped pages are not in RAM. Absent entirely when `SwapTotal` is 0, so a swapless host renders as it did before swap existed |
| Layout | CPU and MEMORY, unbordered, plus a SWAP row on a machine that has swap (`ui::header_rows`). Swap shared the MEMORY row once and cost MEM half its width, which the CPU bar matches, so both headline bars halved for a readout needing ~24 columns at any terminal size: 84 columns of bar became 24. A swapless host still draws the two rows it always did. Discrete VRAM does still share the MEMORY row, since it is the memory MEM is being compared against, and `ui::tank_widths` gives it a fixed `SIDE_TANK` rather than an equal share, capped at half the row so a narrow terminal degrades to the even split instead of starving MEM; persistent rules: header↔tree and tree↔footer. `ui::bar_prefix` right-aligns every label to the longest one `ui::label_width`
says is on screen -- 4 where swap or a discrete card brings `SWAP` or `VRAM`,
3 otherwise -- so the opening bracket lands in one column too. Aligned to
`SWAP` unconditionally, a machine with neither (the ordinary case: most hosts
have no swap) spent a column of bar padding a label it never draws — a bar starting further along
than the one above it reads as a different scale. One helper rather than a
literal per row: `cpu_header_line` and `swap_header_line` build their own
prefixes while `bar_group` builds MEM's and VRAM's, so three copies would
drift the first time a label changed. Every row draws its bar to one width so
the closing brackets stack: `mem_header_line` returns its first tank's width and `cpu_header_line` and `swap_header_line` take it, clamped to their own slack and padded on the right. `mem_header_line` caps its own bar to the CPU row's slack (`ui::cpu_parts`) as well, so the brackets stack whichever suffix is longer |
| Disk R/W | table columns only (formatted rates change width every tick); after compute, before the stall trio |
| THR / AGE | beside `N`, before the metric columns: all three say what the row *is* rather than what it is currently costing |
| CPU/IO/MEM ST | a row carries a figure only when every process under it is in one non-root cgroup; a process row only when it is alone in its cgroup. Folder, User, Host and multi-cgroup rows are blank — a percentage of an interval cannot be summed, and `user-<uid>.slice` is not the User row (a rootful container is billed to its owner from `system.slice`) nor `system.slice` the System row (kernel threads are in the root cgroup). Root-cgroup rows are blank because that pressure is the machine's, the same rule as a `--network=host` container |
| NETNS RX/TX | last two columns, named for the namespace and not the resource: a blank cell means the row owns no namespace, not that it moved no bytes |
| Ordering | one comparator in `once.rs` for every level; a `None` metric sorts last in either direction, name breaks ties, stable over `group::proc_forest` pid order |

TUI sampling runs on a background thread; the ratatui loop only swaps in the
last complete tree and never blocks on `/proc` I/O. `proc::collect` splits the
pid list across a long-lived pool sized by `available_parallelism` and reused
every sample (the pool lives on the Sampler, so `--once` / `--json` still pool
their two walks and `sample_stream` drop joins them), because the walk is
latency-bound on procfs rather than compute-bound and a `thread::scope` per
tick grew glibc arenas (~15 MiB every 5s PSS tick to ~488 MiB). Measured
numbers and which tick actually gains live on that function. PSS/`--once`
fdinfo reads skip files above 64 KiB (a 16 MiB fanotify dump, not drm) and do
not walk every fdinfo when the dri/drm prefilter is empty — GPU clients whose
fd names omit dri/drm stay blank. Sample cadence (`--interval`,
`--pss-interval`, `--once` / `--json`): [HUMANS.md](HUMANS.md).

`src/root.rs` holds the `/proc` and `/sys` prefix in a `OnceLock`, resolved
once in `main` for the same reason `glyph` is. Empty by default, so every path
is the literal it always was. `tests/live_proc.rs`
`a_proc_root_is_the_only_proc_heft_reads` points heft at a root holding an
empty `proc` and asserts the tree comes back empty: a reader still using a
literal path would find the real machine through it and fill the tree. The
container socket and `/etc/passwd` are deliberately not prefixed — one is live
IPC rather than a file in the tree, the other is the host's.

`ui::alarming` marks the cells that say a row is in trouble — a stall column at or over `STALL_ALARM` (20%) — with `ui::alarm_style`: red
where there is colour, `REVERSED` where there is not, which is the fallback
`sort_header` already uses. Reverse rather than a marker character because
`CPU ST` is six columns wide and `100.0` is five, so a marker would overflow
into the clipping `columns_that_fit` exists to prevent. Nothing else is marked:
a large `%CORE` is work, not trouble.

`once::coverage_tail` compares `HostTree::kernel_threads` (field 4 of
`/proc/loadavg`, a global counter that `hidepid` cannot hide) against the Host
row's summed `THR`, and reports below `VISIBLE_OK`. `kernel_threads` is
`#[serde(skip)]` and set in `cpu::header_from` beside `sampled_at`: it
qualifies what the tree reports rather than being part of it.

`main::check_interval` refuses a typed interval below `proc::MIN_INTERVAL`,
negated so a `NaN` — which `Duration::from_secs_f64` panics on — is refused
too. `clamp_intervals` still clamps, for library callers and because `max`
absorbs a NaN. `--pss-interval` is checked against the floor only, never
against `--interval`: it is documented as "at least `--interval`", so
`--interval 10` alone would otherwise fail against the default of 5.

`ui::columns_that_fit` lays out only columns whose full width fits the pane.
ratatui clips a cell that runs out of room, so a 50-column terminal drew
`20.1G` as `2`; a column is now drawn whole or dropped, and `←` / `→` reach the
rest, past a `name` that `ui::scrolled` keeps out of the scroll. At least one column always survives, and the name column is a label
rather than a figure, so cutting it misleads nobody.

JSON shape (`src/types.rs` `HostTree`): `host.sampled_at`,
`host.users[].applications|user_services|containers`, `host.containers`,
`host.system`. Project identities include `containers[].processes[]`, each process node carrying `cmdline` so a reader
can identify a process from the record rather than going back to a `/proc`
that may have lost the pid.
`sampled_at` is set in `cpu::header_from`, the one `HostTree` constructor, off
the `now_epoch` the AGE column already reads; a `--json --follow` line carries
no other clock, so without it two records cannot be placed in time.

## Packaging

`packaging/aur/` holds the three AUR packages and is the source of truth for
them; the AUR repositories are push targets, never edited in place. `heft`
builds from the release tarball, `heft-bin` installs the release musl binaries
(`provides`/`conflicts` heft, and no `depends` at all because they are
static), and `heft-git` builds from main. All three carry
`options=('!strip' '!debug')`: `[profile.release]` already strips, so makepkg
otherwise fails to index a binary with no symbols and ships an empty debug
package.

`packaging/aur/LICENSE` is 0BSD and covers the packaging sources only, not
heft itself: the AUR submission guidelines require a package source licence in
each AUR repository, and one that is not 0BSD makes the package ineligible for
promotion to the official repositories. The push copies it in beside PKGBUILD
and .SRCINFO.

Editing a PKGBUILD means re-running `update.sh`: a `.SRCINFO` is generated
metadata, and a stale one publishes the wrong dependencies and version to
every AUR consumer while the PKGBUILD beside it looks right. The AUR accepts
pushes to `master` only.

`update.sh <version>` sets `pkgver`, refreshes the checksums and regenerates
every `.SRCINFO`. The binary sums are read from the `.sha256` files the
release publishes rather than from a re-download, because `makepkg -g` hashes
only the current architecture's sources — `updpkgsums` on an x86_64 machine
leaves `sha256sums_aarch64` stale and still looking right. It needs makepkg,
so it runs inside `archlinux:base-devel`.

The `aur` job in `release.yml` runs it after the release exists, since the
checksums are of assets that did not exist before, commits the refresh back to
main, and pushes `heft` and `heft-bin` to the AUR when the `AUR_SSH_KEY`
secret is set (it skips with a notice when it is not). The secret is
deliberately not set, so that step always skips and exits green: once the
release and the job's refresh commit exist, pull main and push both by hand —
clone `ssh://aur@aur.archlinux.org/<pkg>.git`, copy `PKGBUILD`, `.SRCINFO` and
`packaging/aur/LICENSE` in, commit `heft <version>`, push `master`. Check the
result with a fresh clone, not the RPC or cgit, which lag by minutes. `heft-git` is pushed by
hand: a tag changes nothing in a package whose `pkgver()` is `git describe`.

## Gates

Contributor commands: [CONTRIBUTING.md](CONTRIBUTING.md) (same as CI / lefthook).
Suite stays under 30s. No live GPU in CI.
`live_proc::heft_does_not_grow_while_it_follows` watches heft's own reported
RSS across a `--follow` window with several PSS ticks in it: growth, not a
ceiling, because the ceiling that catches a leak in seconds is below what heft
legitimately uses on a busy host. It guards the `0a9ee0a` class of regression,
which was found by a person noticing rather than by a gate. Docker sock is optional in CI;
grouping tests use `tests/fixtures/` via `tests/grouping.rs`.

The gate is `cargo clippy --locked --all-targets -- -D warnings` at the
default level plus the `[lints.clippy]` list in `Cargo.toml`. The wider groups
are measured, not assumed, and the measurement is against that same
`--all-targets` gate: `pedantic` + `nursery` + `cargo` reported 397 warnings
across 29 lints, and 266 across 20 once the list below was adopted. 174 of
what is left is the one nursery lint `redundant_pub_crate` objecting to a
visibility style this crate keeps deliberately; the rest is
`option_if_let_else`, `too_many_lines` over render functions that are one
piece on purpose, and `multiple_crate_versions` for two `hashbrown` majors
ratatui pulls. Adopting those wholesale is a mechanical rewrite buying style,
and is refused. Re-measure before quoting a number here; this paragraph
carried 216/87 for long enough to be wrong by 181.

Nothing in the wider groups is a latent bug, which is what makes the refusal
safe rather than lucky: every lint that looked like one is a false positive.
`literal_string_with_formatting_args` fires eight times on the `{up}`/`{down}`
KEYS placeholders, which are literal on purpose; `match_same_arms` wants
`gpu.rs`'s explicit `KiB` list folded into its own catch-all; `float_cmp`
points into rustc's `assert!` expansion; `suboptimal_flops` wants `mul_add` in
three test assertions.

Twelve lints are denied in `Cargo.toml`'s `[lints]` because each found
something real. `needless_pass_by_ref_mut`, `assigning_clones`,
`redundant_clone` and `format_push_string` came first — a `&mut self` on
`psi::set_row` and `proc::WalkPool::collect` that never mutated, a clone
assigned over a live `String` once a frame, a temporary formatted once a frame
in `sixel::encode`. `use_self`, `missing_const_for_fn`, `doc_markdown` and
`map_unwrap_or` followed, all of them machine-applicable. The four cast lints
— `cast_precision_loss`, `cast_possible_truncation`, `cast_possible_wrap`,
`cast_sign_loss` — are the ones that earn their place: this file used to claim
the casts were "bounded by the code around them", and denying the lints turned
that prose into an `#[expect(..., reason = ...)]` at each site naming the
bound, which the compiler now checks is still firing. `cpu.rs` takes one
module-level expect because every cast in it is the same widening of a kernel
counter into the f64 a rate is divided in. Each lint is at zero, which is what
makes denying it free.

Release profile: LTO, `codegen-units = 1`, strip, `panic = abort`.

## Git

Remote is `Rethunk-Tech/heft`, public, Apache-2.0. `publish = false` is a
decision, not an oversight: the release binaries are static musl, so the
install path costs no toolchain and no compile. Do not drop it without asking.
