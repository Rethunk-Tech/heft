# heft — agent guide

Read-only Linux process monitor. Binary name `heft`.

## Start here

@HUMANS.md — install, keys, XDG, sample cadence, live verify.

## Layout

```
src/cli.rs           clap Cli; build.rs includes it so completions/man cannot drift
src/main.rs          dispatch: TUI default, --once, --json
src/lib.rs           modules
build.rs             clap_complete + clap_mangen → OUT_DIR/assets (build-deps only)
demo.tape            vhs script for README.md's demo.gif; regenerate with `vhs demo.tape`
src/types.rs         Process, Metrics, HostTree, JSON shape
src/proc.rs          every visible PID; blank metrics on EACCES
src/cpu.rs           /proc/stat split (usr/sys/wait) + per-pid utime/stime rates
src/mem.rs           meminfo used/Buffers/Cached/Swap, unified APU clip, host VRAM
src/io.rs            /proc/pid/io rates and smaps_rollup PSS + SwapPss
src/net.rs           per-netns rx/tx from /proc/pid/net/dev; container rows only
src/psi.rs           cgroup cpu/io/memory.pressure; single-cgroup rows only
src/gpu.rs           amdgpu/i915/xe fdinfo; dri/drm prefilter; full walk on PSS/--once; drm-client-id dedupe
src/classify.rs      launcher / worker / shell / terminal / compositor tables
src/identity.rs      cgroup parse, merge key + display name
src/containers.rs    GET-only docker/podman; project vs per-container
src/group.rs         Host → User → Applications | User Services | Containers, System
src/config.rs        XDG view.json (sort, filter, hide_columns, column_order; write on save) and grouping.json (read-only)
src/once.rs          columns, tree ordering, table and JSON
src/ui.rs            ratatui header + tree table
src/tty.rs           panic hook + signal handler; restores the terminal
src/glyph.rs         unicode vs ascii bar/rule/marker characters; resolved once
tests/grouping.rs    integration tests over tests/fixtures/
tests/live_proc.rs   invariants over the real /proc; must hold in a bare container
tests/fixtures/      GUI grouping snapshot
```

No `sysinfo` crate. No `nix` unless rustix cannot do it; v1 uses `std` + `libc`.
Never read `/proc/pid/mem`. Never ptrace.

The hand-rolled helpers — the `/proc` and fdinfo field parsers,
`once::scale_1024`, `once::trunc`, `ui::share_cells`,
`identity::systemd_unescape`, and the GET-only HTTP client in
`containers::unix_get` — have no std equivalent at the 1.98 floor. That is
checked, not assumed, so replacing them is not pending work.

Three further consolidations are measured and refused. The `once` and `ui`
walkers look duplicated but emit deliberately different row sets, which is
what makes `--filter` narrower than `/`. The test-module node builders are
shared by nothing because sharing them needs a production `#[cfg(test)] pub
mod` to serve four eight-line helpers. `containers::index_ids` keys both the
full and the 12-hex id while `get` also falls back through `hex12`; the
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
- Display name is `classify::name_of` (`exe` basename else `comm`), not the
  inherited cgroup. `identity::lying_unit` skips terminal transients, Chromium
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
  even when PPID is user systemd, but never derives an identity from a path
  under `/tmp`, `/var/tmp`, or `/run`: an AppImage mount directory is per-run,
  so those fall back to the ancestor walk.
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
  Idle interactive shells stay their own Applications row.
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
`--user`, `--top`, `--hide` and `--order` overwrite whatever the saved view
held, and only `sort`, `desc`, `filter`, `hide_columns` and `column_order` are
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
runs. Every ASCII substitute is one column wide: the header lines are built to
land on an exact width and `once::trunc` cuts to an exact column count, so a
three-character `...` for `…` would overflow both. `Cli::Glyphs` lives in
`src/cli.rs` because `build.rs` compiles that file standalone.

## Columns

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

Visibility is a **view** preference, so it lives in `view.json` beside sort and
filter, never in the read-only `grouping.json`, which is about identity. It
never reaches sampling: heft reads `/proc` files, not columns, so the roll-up
invariants in `tests/live_proc.rs` and `tests/reconcile.rs` are untouched.
`--json` ignores it — a consumer parsing the tree did not ask for a human's
column preference, and the JSON shape is a contract.

## Grouping overrides

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

## Sampler

| metric | formula / source |
| --- | --- |
| `%core` | `100 * Δ(utime+stime) / (CLK_TCK * dt)` (can exceed 100) |
| `%machine` | `%core / nproc` |
| RSS | `/proc/pid/statm` |
| PSS | `/proc/pid/smaps_rollup` — cadence in [HUMANS.md](HUMANS.md) |
| SWAP | `SwapPss:` from that same rollup read, so it costs no extra file and shares the PSS cadence. `SwapPss`, never `Swap`: a shared swapped page must be apportioned or a summed tree reports it once per mapper. Blank on a `SwapTotal: 0` host |
| Host swap | `SwapTotal` − `SwapFree` from `/proc/meminfo` (`SwapCached` is neither, so it is not subtracted) |
| `D` | processes whose `/proc/pid/stat` state (field 3) is `D`, off the line already parsed for utime/stime. A count, so it sums up the tree the way `THR` does, and `0` is a figure rather than the blank a percentage would need |
| THR | `num_threads`, field 20 of the `/proc/pid/stat` already parsed for utime/stime. Sums up the tree the way `nproc` does |
| AGE | `now - (btime + starttime / CLK_TCK)`; `starttime` is field 22 of that same `stat`, `btime` is read from `/proc/stat` once per run and pinned. Aggregates take the OLDEST, never a sum: a duration summed is meaningless, and a max cannot be read as a total |
| Disk R/W | Δ `read_bytes` / `write_bytes` from `/proc/pid/io` |
| GPU mem | prefer `drm-resident-*` over `drm-total-*`; regions `vram`/`gtt` (amdgpu), `local0`/`system0` (i915), `vram0`/`gtt` (xe) |
| NETNS RX/TX | Δ non-`lo` bytes from `/proc/<container-scope-pid>/net/dev`; a new pid or a counter that went backwards discards the interval |
| CPU/IO/MEM ST | Δ `some ... total=` microseconds from that cgroup's `{cpu,io,memory}.pressure` over wall clock. `some`, not `full`: `full` on a one-process cgroup is the same number twice. A counter that went backwards (a recreated cgroup) discards the interval |
| Host `psi` | `some avg10` from `/proc/pressure/{cpu,io,memory}`, on the `--once` and `--json` host line only. Deliberately a different time base from the columns — a machine-wide trend reads better smoothed, a row's rate has to match the `%core` and disk rates beside it. `psi::header_tail` is not in the TUI header: `ui::cpu_header_line` sized its bar around the tail, so the CPU bar came up short of the MEMORY bar on every PSI-capable kernel |
| gfx% / compute% | `drm-engine-gfx`/`-render` and `-compute` ns deltas over wall clock. xe has no ns key: `drm-cycles-rcs`/`-ccs` delta over the `drm-total-cycles-*` GPU-clock delta, each divided by `drm-engine-capacity-*`. Two formulas, deliberately not unified |

| surface | rule |
| --- | --- |
| CPU bar | `/proc/stat` Δ user+nice / system+irq+softirq / iowait; idle+steal unfilled |
| MEM bar | one MemTotal width when APU VRAM is unified; VRAM (unified only) / GTT resident, then Cached/Buffers, then anon; clip so the stack never exceeds `used.min(MemTotal)` (`mem::clip_used`) |
| Discrete VRAM | own tank against `mem_info_vram_total`, sharing the MEMORY row with the MEM bar; only `vram` drops from that legend — GTT is pinned system RAM and stays in MEM |
| Swap | own tank against `SwapTotal`, never a MEM segment: swapped pages are not in RAM. Absent entirely when `SwapTotal` is 0, so a swapless host renders as it did before swap existed |
| Layout | 2 unbordered header rows (extra tanks split the MEMORY row via `ui::tank_widths`, never add a third row); persistent rules: header↔tree and tree↔footer. Both rows draw their bar to one width so the two brackets stack: `mem_header_line` returns its first tank's width and `cpu_header_line` takes it, clamped to its own slack and padded on the right. The MEM group spends more of its row on `] used/total` and a fourth legend label, so the CPU row is normally the one with columns to spare — only a tree with no memory inverts that, and there the clamp wins |
| Disk R/W | table columns only (formatted rates change width every tick); after compute, before the stall trio |
| `D` | left of `%CORE`, which reads D-state as idle; a count, so folder, User and Host rows carry a figure the stall columns must leave blank |
| THR / AGE | beside `N`, before the metric columns: all three say what the row *is* rather than what it is currently costing |
| CPU/IO/MEM ST | a row carries a figure only when every process under it is in one non-root cgroup; a process row only when it is alone in its cgroup. Folder, User, Host and multi-cgroup rows are blank — a percentage of an interval cannot be summed, and `user-<uid>.slice` is not the User row (a rootful container is billed to its owner from `system.slice`) nor `system.slice` the System row (kernel threads are in the root cgroup). Root-cgroup rows are blank because that pressure is the machine's, the same rule as a `--network=host` container |
| NETNS RX/TX | last two columns, named for the namespace and not the resource: a blank cell means the row owns no namespace, not that it moved no bytes |
| Ordering | one comparator in `once.rs` for every level; a `None` metric sorts last in either direction, name breaks ties, stable over `group::proc_forest` pid order |

TUI sampling runs on a background thread; the ratatui loop only swaps in the
last complete tree and never blocks on `/proc` I/O. `proc::collect` then splits
the pid list across a `thread::scope` sized by `available_parallelism`, because
the walk is latency-bound on procfs rather than compute-bound. Measured numbers
and which tick actually gains live on that function. Sample cadence (`--interval`,
`--pss-interval`, `--once` / `--json`): [HUMANS.md](HUMANS.md).

JSON shape (`src/types.rs` `HostTree`): `host.users[].applications|user_services|containers`,
`host.containers`, `host.system`. Project identities include
`containers[].processes[]`.

## Gates

Contributor commands: [CONTRIBUTING.md](CONTRIBUTING.md) (same as CI / lefthook).
Suite stays under 30s. No live GPU in CI. Docker sock is optional in CI;
grouping tests use `tests/fixtures/` via `tests/grouping.rs`.

Release profile: LTO, `codegen-units = 1`, strip, `panic = abort`.

## Git

Remote is `Rethunk-Tech/heft`, public, Apache-2.0.
