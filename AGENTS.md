# heft — agent guide

Read-only Linux process monitor. Binary name `heft`.

## Start here

[HUMANS.md](HUMANS.md) — install, keys, XDG, sample cadence, rules file format, live verify. Read it before touching a flag, column, key or rule.

## Layout

```
src/cli.rs           clap Cli and the one definition of the --trend and --glyphs choices; a library module, and build.rs includes it so completions/man cannot drift
src/main.rs          dispatch: TUI default, --once, --json
src/lib.rs           modules
build.rs             clap_complete + clap_mangen → OUT_DIR/assets (build-deps only); HEFT_VERSION = version~sha; rules.d → OUT_DIR/builtin_rules.rs
rules.d/             built-in rule files, one stage each, embedded by build.rs
.git-sha             export-subst commit for tag tarballs, which have no .git for build.rs to ask
src/types.rs         Process, Metrics, HostTree, JSON shape
src/proc.rs          every visible PID; blank metrics on EACCES
src/cpu.rs           /proc/stat split (usr/sys/wait) + per-pid utime/stime rates
src/mem.rs           meminfo used/Shmem/kernel/cache/Swap, zram mm_stat, unified APU clip, host VRAM
src/io.rs            /proc/pid/io rates and smaps_rollup PSS + SwapPss
src/net.rs           per-netns rx/tx from /proc/pid/net/dev; container rows only
src/psi.rs           cgroup cpu/io/memory.pressure; max of member `some` on multi-cgroup identity/instance rows
src/gpu.rs           amdgpu/i915/xe fdinfo; dri/drm prefilter; no empty-prefilter walk; oversized fdinfo skipped; drm-client-id dedupe; fd table rescanned only on PSS ticks
src/classify.rs      procedures over the rule classes: crash-helper owner, launcher payload hint, interactive shell
src/identity.rs      cgroup parse, merge key + display name
src/containers.rs    GET-only docker/podman; project vs per-container
src/group.rs         Host → User → Applications | User Services | Containers, System
src/rules.rs         rules.d engine: load, compile, evaluate, examples, --check-rules
src/config.rs        XDG view.json (sort, filter, hide_columns, column_order; write on save); strip_comments for it and rules.d
src/once.rs          columns, tree ordering, table and JSON
src/explain.rs       --explain PID: resolved placement, the placement key, the rule each stage matched
src/ui.rs            ratatui header + tree table
src/tty.rs           panic hook + signal handler; restores the terminal
src/glyph.rs         unicode, legacy or ascii bar/rule/marker characters; resolved once
src/keys.rs          the one TUI key list; build.rs includes it for the man page
src/root.rs          the /proc and /sys prefix behind --proc-root, resolved once; tests/live_proc.rs:a_proc_root_is_the_only_proc_heft_reads guards it
src/caps.rs          --trend auto: one round trip asking the terminal what it draws
src/kgp.rs           --trend kitty: TREND as one graphics-protocol image; shm or inline
src/sixel.rs         --trend sixel: the same image, RLE sixel, positioned from the frame
tests/grouping.rs    integration tests over tests/fixtures/; links the library
tests/live_proc.rs   invariants over the real /proc; must hold in a bare container
tests/reconcile.rs   heft against /proc read independently, not against itself
tests/common/mod.rs  heft(), arr() and pids() for the two that drive the built binary
tests/fixtures/      grouping worlds: gui/ desktop snapshot, zygote/ fallback case
packaging/aur/       PKGBUILD + .SRCINFO for heft, heft-bin, heft-git; update.sh
```

No `sysinfo` crate. No `nix` unless rustix cannot do it; heft uses `std` + `libc`,
plus `rustix` (already built for crossterm) for the dirfd-relative `/proc/<pid>` reads.
Never read `/proc/pid/mem`. Never ptrace.

## Grouping invariants

- Bucket (`src/group.rs`): docker/libpod scope or helper that names that id →
  Containers; `identity::is_kernel` or leftover `system.slice`
  (`in_system_slice` and not `in_user_slice`) → System; else that uid's User,
  where a user-instance `*.service` not starting with `app-` is User Services
  and the rest Applications. `init.scope` + `systemd --user` is a user
  service; so is the `compositor` class whatever its unit looks like.
- `identity::unit_line` picks the one cgroup line a unit name is read from:
  the `0::` line on v2, else `1:name=systemd:`. The v1 trap is on that function.
- An `app-…` scope owns everything in it when a process in it carries the app
  the scope names (`identity::app_scope_names` reads systemd's
  `app[-<launcher>]-<AppID>[-<random>]` as both the full stem and the stem
  without its launcher component; `group::Ctx::scope_app` resolves one app per
  scope per tick). Steam's `srt-logger`, `pv-adverb`, `srt-bwrap` and
  `steamwebhelper` share only that scope, and that is enough. A scope naming a
  desktop id no process carries (`app-com.vivaldi.Vivaldi-…`) names nothing, so
  a command launched from a browser or a terminal keeps its own row; a `lying`
  unit is skipped before the question is asked.
- Display name is `classify::name_of` (`exe` basename else `comm`), not the
  inherited cgroup. The exception is an `exe` of `tdeinit`, which runs
  programs as in-process modules, so there `comm` names the program. TDE
  session processes merge as `tdeinit` (`rules.d/40-trinity.json`, which sorts
  before `50-plasma.json` because both ship `kded` and `ksmserver`). An editor's
  install or extension tree bills its binaries to that editor
  (`rules.d/70-editors.json`, the `app` stage), by `exe` path rather than PPID so a real
  app started from its terminal keeps its row. The `lying` unit flag (`rules.d/05-units.json`) skips terminal transients, Chromium
  toolkit scopes (`org.chromium.chromium`), `dbus:` activation, `run-u*`, and
  `flatpak-session-helper` for unit-based identity. `instance_key` uses a real
  user unit only when it is not lying; otherwise `pgid`.
- Launchers (the `launchers` class rule, names ending `.appimage` included) have no
  top-level row; cost bills to the unique payload identity; they still appear
  inside the expanded process list. An `.appimage` launcher with no payload
  child bills to the app running from its `.mount_<first six bytes of the file
  name>` mount (`classify::appimage_mount_prefix`, which carries the
  shared-prefix ceiling). Nested bwrap folds into the payload. `cat`
  under a launcher or app bills to that parent; it does not break unique-payload
  folding and does not become its own row.
- Workers (the `worker` class) fold into that app. Walk ancestors skipping
  launchers and other generics, except one an `app` rule names (Cursor's
  bundled `node` agent), which owns the chain; do not invent a script-basename
  identity (`context7-mcp`) when a launching agent (`claude`, `cursor`) is above.
  Processes stay visible on expand. `crash_helper_app` matches exe/cmdline
  even when PPID is user systemd. The mount directory itself under `/tmp`,
  `/var/tmp`, or `/run` (`/tmp/mount`, `/tmp/.mount_cursorAb12Cd`) is per-run
  and never an identity; a stable directory nested under it
  (`…/usr/share/cursor/chrome_crashpad_handler`) names the app the same way
  `/opt/cursor/…` does. When no process carries that directory name, the
  helper bills to the display name of a non-helper process of its own uid,
  outside `system.slice` and container or machine scopes, whose `exe` sits in
  the same directory, lowest pid first
  (`/opt/vivaldi/chrome_crashpad_handler` → `vivaldi-bin`). A helper whose
  parent dir is the mount takes that same-directory non-helper, and with none
  falls back to the ancestor walk.
- A generic orphaned to `systemd --user` bills to the lowest-pid process in its
  exact cgroup that names an app (a `bun` a Claude Code daemon left behind).
  Any other orphan bills to its live process-group leader when that leader is
  an Applications row (a backgrounded `wl-copy` to `claude`); the comment in
  `group::compute_place` carries why not User Services.
- User Services grouping is one identity for processes that share a systemd
  unit family, RPM/package family, D-Bus well-known name family, or documented
  process architecture — not a comm prefix. Mappings live in the `session`
  stage files, `rules.d/20-session-bus.json` to `80-apps.json`. Exceptions: prefix lookalikes with a
  different product stay out (`gsd-disk-utility-notify`, independent `wsdd`,
  `wireplumber`, `krunner`, `plasma-discover`, `kwindowprop`); independent apps
  never fold into gnome-shell, plasmashell, kwin or these service identities; an arbitrary user CLI is Applications;
  `p11-kit` must not fold into `flatpak-session-helper` (Cursor shares that
  cgroup); user-session `dbus-broker` is User Services, never Applications
  (the `lying` rule matches `dbus:` activation, not `dbus-broker.service`);
  app-bound `xdg-dbus-proxy` bills to that app, unbound folds into `flatpak`;
  `gcr-ssh-agent` absorbs `ssh-agent` only in that unit.
- Placement walks (`resolve_one`, `compute_place`, `owning_app_ancestor`,
  `unique_descendant_ident`) share one depth budget, `group::MAX_WALK`; past
  it a process is placed on its own facts. The process forest nests at most
  `group::MAX_PROC_DEPTH`, whose doc carries the JSON depth arithmetic.
- Split when the child's resolved identity differs and the child is a real app.
  Idle interactive shells fold into their terminal (the `terminal` class);
  a unique payload child (claude, dstat) takes the owning shell, the same
  walk as a launcher. A shell with no terminal parent and no unique payload
  stays Applications.
- Generic interpreters (the `generics` class rule) fall back to the user unit or
  a distinctive script basename (`identity::generic_fallback`) so they do not
  collapse into one interpreter row. Non-distinctive script basenames are the
  `anonymous-scripts` class rule.
- Containers: never System, never `dockerd`/`containerd`. Project key is
  `com.supabase.cli.project` → `supabase:<name>`, else
  `supabase_<role>_<project>` names, else `com.docker.compose.project`. No
  project → one row per container name; a vendor-specific label is not a merge
  key. `containerd-shim-runc-v2 -id`, `docker-proxy -container-ip`, `conmon`,
  `runc`/`crun` bill to that container. Owner:
  workdir path uid, else the first non-root uid owning an `Inspect.Mounts`
  bind source (named volumes are root-owned and skipped), else Host →
  Containers.
- NETNS RX/TX is the one metric only a container row carries: `net::netns_pids`
  reads it through the lowest pid in the container's own scope,
  `Inspect::owns_netns` skips host networking, and `Metrics::accumulate` sums
  neither the pair nor the three `*_stall_pct` fields (`types.rs` guards both).

## Surfaces, flags and columns

Each of these has one home; a second copy is the defect.

| what | where | rule |
| --- | --- | --- |
| `--follow` loop | `proc::sample_stream` | honours `--pss-interval`; `once::follow_table` / `follow_json` share `render_table` and `json_text` with the one-shot pair |
| flag over `view.json` precedence | `main::resolve_view` | `--hide` adds to the saved list, every other flag overwrites; only `sort`, `desc`, `filter`, `hide_columns`, `column_order` are read back (`users`, `top` are `#[serde(skip)]`). `--once` starts from `config::load_view()`, `--json` from `View::default()` and never the file. `--filter`, `--top`, `--hide`, `--order` are `conflicts_with = "json"` because the JSON shape is a contract |
| unknown `--sort` label | clap `InvalidValue` via `once::sort_labels` | not `Sort::from_label`'s fallback: a stale `view.json` must not stop the monitor, an argument just typed can be corrected. `src/cli.rs` cannot reach `COLUMNS` because `build.rs` includes it standalone for the completions and man page |
| bad regex | `once::Filter::new` returns `None` (`regex-lite`) | each caller decides: clap `InvalidValue`, a `view.json` warning, the TUI's `?` footer |
| escaping process text for stdout | `once::printable` | the tree and `--json` keep raw strings; the TUI relies on ratatui dropping control characters |
| `--filter` and `/` haystack | `TableRow::search` / `Flat::search` (`once::haystack`) when built, else `name` | `keep_matches`, `keep_top` and `Filter` carry the rest; `ProcNode::cmdline` is the argv `proc` already read, serialized in `types.rs` |
| `Set::Legacy` vs `Set::Unicode` | `glyph::spark_ramp`, nowhere else | `glyph.rs` says why, and why `detect` never returns it |
| `--trend kitty` / `sixel` / `auto` | `src/kgp.rs` / `src/sixel.rs` / `src/caps.rs` | each module doc carries its protocol decisions; `ui::resolve_trend` the kitty-versus-sixel choice; `tty::hold_shm` the signal-safe shm cleanup |
| `i` and `?` overlays | `ui::popup`, `ui::Overlay` | `i` is `ui::draw_detail`, `ui::metric_grid`, `proc::detail`, plus `explain::placement_lines` / `explain::identity_placement` for the row; `Overlay` holds which is open, so only one draws |
| TREND scale | `ui::trend_scale`, one per frame | says why it is not each row's own peak; `ui::spark` and `kgp::paint` both draw against it |
| `spark` column | `Column::fmt` returns empty; `ui::draw` substitutes `ui::spark` from `App::history`, `App::trend_w` deep | the one column not a function of the current sample: only `Columns::for_tui` includes it, `Sort::step` skips it (`key: None`), and `--order` validates against `column_labels()` rather than `sort_labels()` since not everything movable is sortable |
| column model | `once::COLUMNS`; `once::Columns` is it with `view.hide_columns` and `view.column_order` applied | resolved at start, rebuilt on `H` / `u`, so no render site branches on visibility and nothing reaches sampling; `Sort::step` cycles the visible list; `config::default_hidden` hides `rss` and the stall trio when `hide_columns` is absent. Visibility is a view preference: `view.json`, never `rules.d`, and `--json` ignores it |

## Rules

`rules.d/*.json` holds every table-shaped grouping decision and `src/rules.rs`
evaluates it. Procedures stay Rust: the ancestor walks in `group.rs`,
`classify::crash_helper_app`, `classify::launcher_payload_hint`,
`classify::is_interactive_shell`, `identity::generic_fallback`, and container,
machine and System bucketing. `build.rs` embeds the directory through a
generated `include_str!` table, sorted because `read_dir` order is
machine-dependent, so a file added there cannot be left out of the binary.

The contract, which a change may extend but not alter:

- Stages run `unit`, `class`, `session`, `app`, `placement`; the first two
  union every match, the rest take the first. `session` runs before
  `classify::crash_helper_app` and `app` after it, both in
  `group::direct_place`; no built-in example can prove that order, so
  `tests/grouping.rs:a_user_session_rule_runs_before_the_crash_helper_and_an_app_rule_after_it`
  holds it.
- ASCII case folds on both sides: patterns are lowercased once at compile,
  haystacks never. Unicode case is not folded.
- Source order (XDG, `/etc`, built-ins; `HEFT_RULES_PATH` in place of the
  first two) outranks file name, so moving a table into a built-in file
  cannot outrank a user rule.
- `disable` entries are collected from every file that compiled before any
  rule is kept, applied to that file name in every source, and never undone,
  so the result is order independent.
- Ids are `[a-z0-9-]+`, unique per file. An empty list, `all`, `any` or
  string pattern fails the file (`name_prefix ""` matches every process), as does `script` outside a class rule whose
  classes are exactly `[anonymous_script]`: `judged` never sets
  `Facts::script`, so it would be a silent miss. Half a file loaded is a rule
  set nobody wrote.
- Placement is two lists by output kind (`Rules::deciding`): a container
  subject sees `owner_uid` rules, an identity sees `fold_to` and `folder`
  rules. `group::override_place` runs only on Applications and User Services.
  `owner_uid` is consulted before workdir and bind-mount inference in
  `containers::insert_resolved`.

`group::Ctx::new` builds `judged` once per pid per tick (unit, flags, class
set, and the positional-argv launcher test in `group::judge`); every class and
unit question in `group.rs`, `identity::instance_key` and
`identity::generic_fallback` reads it. It is keyed on pid and valid for one
`Ctx`, which lives one `build_tree`, so `exec` needs no invalidation. Session
and app rules evaluate on a borrowed `Facts` in `direct_place` and allocate
nothing (`rules::tests::evaluation_allocates_nothing`, counted per thread
because other tests allocate on other threads). `build_tree_timing` in
`tests/grouping.rs` and `rules_timing` in `src/rules.rs` are `--ignored` timing
harnesses; `HEFT_BENCH_FIXTURE` names a `--fixture` dump.

`Rules::load`, `rules::load_dir` and `Rules::builtin` carry the load contract.
`tests/common::heft` points
`HEFT_RULES_PATH` at a missing directory so a developer's `/etc/heft/rules.d`
cannot reach a binary-driven test.

`--check-rules` (`rules::print_check`, judged in `Rules::report`) never
samples, so it runs in the bare container. An example tests one process's
facts against the merged set, so anything an ancestor walk decides stays a
fixture test.

`--explain <PID>` is `src/explain.rs`, whose module doc carries the design.
`proc::sample_placement` is one walk and grouping, not `sample_world`'s
sleep-and-second-tick: the report never prints rates. `locate` recurses
through `ProcNode::children`, since the pid asked about is usually a folded
worker, and offers no key for a container or System row because
`override_place` cannot move one.

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
| SWAP | `SwapPss:` from the PSS rollup read (`Metrics::swap_bytes`) |
| Host swap | `SwapTotal` − `SwapFree` from `/proc/meminfo` (`SwapCached` is neither, so it is not subtracted) |
| THR | `num_threads`, field 20 of the `/proc/pid/stat` already parsed for utime/stime |
| AGE | `now - (btime + starttime / CLK_TCK)`; `starttime` is field 22 of that `stat`, `btime` read once per run; aggregates in `Metrics::age_secs` |
| Disk R/W | Δ `read_bytes` / `write_bytes` from `/proc/pid/io` |
| GPU mem | first tier the client publishes of `gpu::MEM_PREFIXES`; regions `vram`/`gtt` (amdgpu), `local0`/`system0` (i915), `vram0`/`gtt` (xe) |
| NETNS RX/TX | Δ non-`lo` bytes from `/proc/<container-scope-pid>/net/dev`; a new pid or a counter that went backwards discards the interval |
| CPU/IO/MEM ST | Δ `some ... total=` microseconds from that cgroup's `{cpu,io,memory}.pressure` over wall clock; several cgroups on a row: max of members' `some`, per resource (`psi.rs` says why `some` and why max). A counter that went backwards discards the interval |
| Host `psi` | `some avg10` from `/proc/pressure/{cpu,io,memory}`, `--once` and `--json` host line only (`psi::header_tail`) |
| gfx% / compute% | `drm-engine-gfx`/`-render` and `-compute` ns deltas over wall clock. xe: `drm-cycles-rcs`/`-ccs` delta over the `drm-total-cycles-*` delta, each divided by `drm-engine-capacity-*` (`cpu::cycles_pct`). Two formulas, deliberately not unified |

| surface | rule |
| --- | --- |
| CPU bar | `/proc/stat` Δ user+nice / system+irq+softirq / iowait; idle+steal unfilled |
| MEM bar | one MemTotal width when APU VRAM is unified; segment order `ui::mem_key`, contents and clipping `mem::clip_used` (reclaimable cache sits beyond `used`, never inside it), swatches `ui::bar_key` |
| Discrete VRAM | own tank against `mem_info_vram_total` on the MEMORY row; only `vram` leaves the MEM segments, GTT being pinned system RAM |
| Swap | own row against `SwapTotal`, never a MEM segment (`ui::header_rows`) |
| Layout | CPU and MEMORY, unbordered, plus a SWAP row where there is swap. Discrete VRAM shares the MEMORY row (`ui::tank_widths`). `ui::bar_prefix` aligns the labels and every row draws its bar to `mem_header_line`'s width so the brackets stack; persistent rules header↔tree and tree↔footer |
| Disk R/W | table columns only (formatted rates change width every tick); after compute, before the stall trio |
| THR / AGE | beside `N`, before the metric columns: all three say what the row *is* rather than what it is currently costing |
| CPU/IO/MEM ST | max rather than sum or average across a row's cgroups: sum exceeds 100% on overlap, average hides a fully-stalled member. Folder, User and Host rows are blank because `user-<uid>.slice` is not the User row (a rootful container bills to its owner from `system.slice`) nor `system.slice` the System row (kernel threads are in the root cgroup); root-cgroup pressure is the machine's, the same rule as a `--network=host` container |
| Ordering | one comparator in `once.rs` for every level; a `None` metric sorts last in either direction, name breaks ties, stable over `group::proc_forest` pid order |

TUI sampling runs on a background thread; the ratatui loop only swaps in the
last complete tree and never blocks on `/proc` I/O. `proc::WalkPool` says
why the walk is a pool; `gpu.rs` carries the oversized-fdinfo skip and the
empty-prefilter rule.

`ui::alarming` marks the cells that say a row is in trouble — a stall column at or over `STALL_ALARM` — with `ui::alarm_style`: red
where there is colour, `REVERSED` where there is not, which is the fallback
`sort_header` already uses. Reverse rather than a marker character because
`CPU ST` is six columns wide and `100.0` is five, so a marker would overflow
into the clipping `columns_that_fit` exists to prevent. Nothing else is marked:
a large `%CORE` is work, not trouble.

JSON shape (`src/types.rs` `HostTree`): `host.sampled_at`,
`host.users[].applications|user_services|containers`, `host.containers`,
`host.system`. Project identities include `containers[].processes[]`, each
process node carrying `cmdline`.

## Packaging

`packaging/aur/` is the source of truth for the three AUR packages; the AUR
repositories are push targets, never edited in place. Editing a PKGBUILD means
re-running `update.sh`, or the generated `.SRCINFO` publishes stale metadata
while the PKGBUILD beside it looks right. Release procedure, the 0BSD
packaging licence and why `AUR_SSH_KEY` stays unset:
[packaging/aur/README.md](packaging/aur/README.md).

## Gates

Contributor commands:
[CONTRIBUTING.md](CONTRIBUTING.md) (same as CI / lefthook). Docker sock is
optional in CI; grouping tests use `tests/fixtures/` via `tests/grouping.rs`.

`live_proc::heft_does_not_grow_while_it_follows` watches heft's own RSS
growth across a `--follow` window with several PSS ticks, not a ceiling: one
tight enough to catch a leak in seconds is below what heft legitimately uses
on a busy host. It guards the walk pool: a fresh thread per walk leaves glibc
arenas climbing.

The four cast lints denied in `Cargo.toml` are at zero, which is what makes
denying them free. A claim that a cast is bounded lives as an
`#[expect(..., reason = ...)]` at that site naming the bound, never as prose,
so the compiler checks the expect is still firing. `cpu.rs` takes one
module-level expect because every cast in it has the same bound: one
interval's `saturating_sub` delta of a kernel counter, widened into the f64 a
rate is divided in.

## Git

Remote is `Rethunk-Tech/heft`, public, Apache-2.0. `publish = false` is a
decision (HUMANS.md, Install); do not drop it without asking.
