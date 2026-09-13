# heft — agent guide

Read-only Linux process monitor. Binary name `heft`.

## Start here

@HUMANS.md — install, keys, XDG, sample cadence, live verify.

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
src/psi.rs           cgroup cpu/io/memory.pressure; single-cgroup rows only
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
src/root.rs          the /proc and /sys prefix behind --proc-root, resolved once
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

The hand-rolled helpers (the `/proc` and fdinfo field parsers,
`once::scale_1024`, `ui::share_cells`, `identity::systemd_unescape`, and the
GET-only HTTP client in `containers::unix_get`) have no equivalent in std
*or in a crate already in the tree*. Both halves are checked, not assumed, so
replacing them is not pending work. `share_cells` is a partial-fill allocator
whose result is meant to come up short, which no ratatui `Constraint`
expresses, and `unix_get` stays because the tree carries no HTTP client at
all.

Two further consolidations are measured and refused. The `once` and `ui`
walkers look duplicated, but the row types do not: `Flat` carries an `id` and
an `expandable` flag that `TableRow` has no use for, and that `id` is what
keys expand membership, the trend history and the cursor's re-anchoring
across a resort. Unifying them takes four parameters that are each constant on
one caller, and makes `--once` and `--follow` `format!` an id per row and
throw it away. That the row sets differ is the consequence (it is why
`--filter` is narrower than `/`), not the reason.

`containers::id_key` is the one `by_id` key, on insert and lookup alike: the
normalized id cut to 12 hex digits, so a full id and a truncated one reach the
same row. The ceiling is that two running containers sharing a 12-hex prefix
bill to one row. Keying the full id alone is refused: a runtime that reports a
truncated id would silently lose its row.

## Grouping invariants

- **Host** is the machine, not the compositor or a terminal.
- Bucket (`src/group.rs`): docker/libpod scope or helper that names that id →
  Containers; `identity::is_kernel` or leftover `system.slice`
  (`in_system_slice` and not `in_user_slice`) → System; else that uid's User.
- Under a User: user-instance unit `*.service` not starting with `app-` →
  User Services; else Applications. `init.scope` + `systemd --user` is a user
  service. Known compositors (the `compositor` class) are user services even
  if the unit looks like an app.
- `identity::unit_line` picks the one cgroup line a unit name is read from:
  the `0::` line on v2, else `1:name=systemd:`. The v1 trap is on that function.
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
  inside the expanded process list. Nested bwrap folds into the payload. `cat`
  under a launcher or app bills to that parent; it does not break unique-payload
  folding and does not become its own row.
- Workers (the `worker` class) fold into that app. Walk ancestors skipping
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

## Surfaces and flags

`proc::sample_stream` is the `--follow` loop and honours `--pss-interval`;
`once::follow_table` and `once::follow_json` share rendering with the one-shot
pair through `render_table` and `json_text`.

`main::resolve_view` builds one `View` and `main` hands it to whichever surface runs, so
precedence lives in one place: `--sort`, `--asc` / `--desc`, `--filter`,
`--user`, `--top` and `--order` overwrite whatever the saved view held,
`--hide` adds to its list, and only `sort`, `desc`, `filter`, `hide_columns`
and `column_order` are ever read back (`users` and `top` are
`#[serde(skip)]`). `--once` starts from `config::load_view()`; `--json` starts
from `View::default()` and never reads the file. `--filter`, `--top`, `--hide`
and `--order` are `conflicts_with = "json"`: the JSON shape is a contract.

An unknown `--sort` label is a clap `InvalidValue` exit, not
`Sort::from_label`'s fallback — a stale `view.json` must not stop the monitor,
an argument just typed can still be corrected. `once::sort_labels` feeds that
error; `src/cli.rs` cannot reach `COLUMNS` because `build.rs` includes it
standalone to generate the completions and man page.

`once::Filter` (`regex-lite`; the measurement is on it) returns `None` from
`new` rather than an error, and each caller decides what that means: clap
`InvalidValue` for `--filter`, warn-and-ignore for a stale `view.json`, and in
the TUI the last compiling pattern stays live behind a `?` in the footer.

`--filter` and `/` match `TableRow::search` / `Flat::search` (`once::haystack`)
when one is built, else `name`; `keep_matches`, `keep_top` and `Filter` carry
the rest. `ProcNode::cmdline` is the argv `proc` already read, and is
serialized (`types.rs`).

`Set::Legacy` differs from `Set::Unicode` in `glyph::spark_ramp` and nowhere
else; `glyph.rs` carries why that holds and why `detect` never returns it.

`--trend kitty`, `--trend sixel` and `--trend auto` live in `src/kgp.rs`,
`src/sixel.rs` and `src/caps.rs`; each module doc carries its protocol
decisions, `ui::resolve_trend` the kitty-versus-sixel choice with its
measurement, and `tty::hold_shm` the signal-safe shm cleanup.

`i` (`ui::draw_detail`, `ui::metric_grid`, `proc::detail`) and `?` share
`ui::popup`; `ui::Overlay` holds which one is open, so only one draws.

## Columns

`ui::trend_scale` is the one TREND scale per frame, and says why it is not
each row's own peak; `ui::spark` and `kgp::paint` both draw against it.

`spark` is the one column whose cell is not a function of the current sample:
its `Column::fmt` returns empty and `ui::draw` substitutes `ui::spark` from
`App::history`, `App::trend_w` deep. `Columns::for_tui` is the only
constructor that includes it, and `Sort::next` skips it (`key: None`).
`--order` validates against `column_labels()` rather than `sort_labels()`,
since not everything movable is sortable.

`once::COLUMNS` is the one column model; `once::Columns` is that list with
`view.hide_columns` and `view.column_order` applied, resolved at start and
rebuilt when `H` / `u` change it, so no render site branches on visibility
and nothing reaches sampling. `Sort::next` cycles over the visible list only.
`config::default_hidden` hides the stall trio when `hide_columns` is absent.
Visibility is a view preference, so it lives in `view.json`, never in
`rules.d`, and `--json` ignores it.

## Rules

`rules.d/*.json` holds every table-shaped grouping decision and `src/rules.rs`
evaluates it. Procedures stay Rust: the ancestor walks in `group.rs`,
`classify::crash_helper_app`, `classify::launcher_payload_hint`,
`classify::is_interactive_shell`, `identity::generic_fallback`, and container,
machine and System bucketing. `build.rs` embeds the directory through a
generated `include_str!` table, sorted because `read_dir` order is
machine-dependent, so a file added there cannot be left out of the binary.
Embedding costs 25,312 bytes of stripped release binary (1.3%).

The contract, which a change may extend but not alter:

- Stages run `unit`, `class`, `session`, `app`, `placement`. `unit` and
  `class` union every matching rule (`chrome_crashpad_handler` is
  `crash_helper` and `worker`; `flatpak-session-helper.service` is `lying` and
  `service`); the other three take the first match. `session` runs before
  `classify::crash_helper_app` and `app` after it, both in
  `group::direct_place`, and no built-in example can prove that order, so
  `tests/grouping.rs:a_user_session_rule_runs_before_the_crash_helper_and_an_app_rule_after_it`
  holds it.
- Every string test folds ASCII case on both sides: patterns are lowercased
  once at compile, haystacks never. Unicode case is not folded.
- Sources rank XDG, then `/etc`, then built-ins, whatever the file names;
  `HEFT_RULES_PATH` replaces the first two with its entries in order. File
  names sort bytewise within a source. A user rule therefore beats every
  built-in, and moving a table into a built-in file cannot outrank one.
- `disable` entries are `<file>.json` or `<file>.json:<id>`, collected from
  every file that compiled before any rule is kept, applied to that file name
  in every source, and never undone, so the result is order independent. A
  file that fails to compile contributes none.
- Ids are `[a-z0-9-]+`, unique per file. A `match` object carries one key. An
  empty list, `all`, `any` or string pattern fails the file (`name_prefix ""`
  matched every process), and so does `script` outside a class rule whose
  classes are exactly `[anonymous_script]`: `judged` never sets
  `Facts::script`, so it would be a silent miss. Half a file loaded is a rule
  set nobody wrote.
- Placement is two lists by output kind (`Rules::deciding`): a container
  subject sees `owner_uid` rules, an identity sees `fold_to` and `folder`
  rules. `group::override_place` runs only on Applications and User Services.
  `owner_uid` is consulted before workdir and bind-mount inference in
  `containers::insert_resolved`.
- Built-in file names and rule ids are what `disable`, `--check-rules` and
  `--explain` print, so renaming one is a breaking change for the changelog.

`group::Ctx::new` builds `judged` once per pid per tick: the unit, its flags
and the class set, plus the positional-argv launcher test in `group::judge`.
Every class and unit question in `group.rs`, `identity::instance_key` and
`identity::generic_fallback` reads it; asked per call site instead, the name
lookup alone ran 2,200 times per 293-process tick. Keyed on pid and valid for
one `Ctx`, which lives one `build_tree`, so `exec` needs no invalidation.

Session and app rules evaluate on a borrowed `Facts` in `direct_place`, and
evaluation allocates nothing (`rules::tests::evaluation_allocates_nothing`,
counted per thread because the other tests allocate on other threads). The
compiled shape is what makes that affordable: a `Vec<String>` per test with a
`windows` scan cost 750 ns per process, and equality bucketed by byte length
with a first-byte scan for contains cost 415 to 450.

`build_tree` measures 149 to 152 µs on the gui fixture and 463 to 479 µs on a 324-process
`--fixture` dump, and the four stages 433 to 438 ns per process on facts built by
`group::facts_of`. The budget is 20% and 500 ns, measured on a quiet machine:
`cargo test --release --test grouping -- --ignored --nocapture
build_tree_timing` and `cargo test --release --lib -- --ignored --nocapture
rules_timing`, with `HEFT_BENCH_FIXTURE` naming a `--fixture` dump.

`Rules::load`, `rules::load_dir` and `Rules::builtin` carry the load contract.
`tests/common::heft` points
`HEFT_RULES_PATH` at a missing directory so a developer's `/etc/heft/rules.d`
cannot reach a binary-driven test.

`--check-rules` (`rules::print_check`, judged in `Rules::report`) never
samples, so it runs in the bare container. User examples are judged against
the merged set. Exit 1 on any failure or any file that did not load. An
example tests one process's facts, so anything an ancestor walk decides stays
a fixture test.

`--explain <PID>` is `src/explain.rs`, whose module doc carries the design.
`locate` recurses through `ProcNode::children`, since the pid asked about is
usually a folded worker, and offers no key for a container or System row
because `override_place` cannot move one.

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
| SWAP | `SwapPss:` from that same rollup read, sharing the PSS cadence (why `SwapPss`: `Metrics::swap_bytes`). Blank on a `SwapTotal: 0` host |
| Host swap | `SwapTotal` − `SwapFree` from `/proc/meminfo` (`SwapCached` is neither, so it is not subtracted) |
| THR | `num_threads`, field 20 of the `/proc/pid/stat` already parsed for utime/stime; sums like `nproc` |
| AGE | `now - (btime + starttime / CLK_TCK)`; `starttime` is field 22 of that `stat`, `btime` read once per run. Aggregates take the oldest (`Metrics::age_secs`) |
| Disk R/W | Δ `read_bytes` / `write_bytes` from `/proc/pid/io` |
| GPU mem | first tier the client publishes of `gpu::MEM_PREFIXES`; regions `vram`/`gtt` (amdgpu), `local0`/`system0` (i915), `vram0`/`gtt` (xe). The `drm-memory-*` tier was measured on a 4750G |
| NETNS RX/TX | Δ non-`lo` bytes from `/proc/<container-scope-pid>/net/dev`; a new pid or a counter that went backwards discards the interval |
| CPU/IO/MEM ST | Δ `some ... total=` microseconds from that cgroup's `{cpu,io,memory}.pressure` over wall clock (`some`, not `full`: `psi.rs`). A counter that went backwards discards the interval |
| Host `psi` | `some avg10` from `/proc/pressure/{cpu,io,memory}`, on the `--once` and `--json` host line only; `psi.rs` and `psi::header_tail` say why |
| gfx% / compute% | `drm-engine-gfx`/`-render` and `-compute` ns deltas over wall clock. xe: `drm-cycles-rcs`/`-ccs` delta over the `drm-total-cycles-*` delta, each divided by `drm-engine-capacity-*` (`cpu::cycles_pct`). Two formulas, deliberately not unified |

| surface | rule |
| --- | --- |
| CPU bar | `/proc/stat` Δ user+nice / system+irq+softirq / iowait; idle+steal unfilled |
| MEM bar | one MemTotal width when APU VRAM is unified; segment order in `ui::mem_key`, contents and clipping in `mem::clip_used` (reclaimable cache sits beyond `used`, never inside it); unique colour and alternating `█▓▒` fill per segment, swatches only in `?` (`ui::bar_key`) |
| Discrete VRAM | own tank against `mem_info_vram_total`, sharing the MEMORY row with the MEM bar; only `vram` drops out of the MEM segments — GTT is pinned system RAM and stays in MEM |
| Swap | own row against `SwapTotal`, never a MEM segment; absent when `SwapTotal` is 0 (`ui::header_rows`) |
| Layout | CPU and MEMORY, unbordered, plus a SWAP row where there is swap. Discrete VRAM shares the MEMORY row (`ui::tank_widths`). `ui::bar_prefix` aligns the labels and every row draws its bar to `mem_header_line`'s width so the brackets stack; persistent rules header↔tree and tree↔footer |
| Disk R/W | table columns only (formatted rates change width every tick); after compute, before the stall trio |
| THR / AGE | beside `N`, before the metric columns: all three say what the row *is* rather than what it is currently costing |
| CPU/IO/MEM ST | a row carries a figure only when every process under it is in one non-root cgroup; a process row only when it is alone in its cgroup. Folder, User, Host and multi-cgroup rows are blank — a percentage of an interval cannot be summed, and `user-<uid>.slice` is not the User row (a rootful container is billed to its owner from `system.slice`) nor `system.slice` the System row (kernel threads are in the root cgroup). Root-cgroup rows are blank because that pressure is the machine's, the same rule as a `--network=host` container |
| NETNS RX/TX | last two columns, named for the namespace and not the resource: a blank cell means the row owns no namespace, not that it moved no bytes |
| Ordering | one comparator in `once.rs` for every level; a `None` metric sorts last in either direction, name breaks ties, stable over `group::proc_forest` pid order |

TUI sampling runs on a background thread; the ratatui loop only swaps in the
last complete tree and never blocks on `/proc` I/O. `proc::WalkPool` carries
the walk pool's measurements and why it is a pool; `gpu.rs` the 64 KiB fdinfo
skip and the empty-prefilter rule.

`src/root.rs` is the `--proc-root` prefix;
`tests/live_proc.rs:a_proc_root_is_the_only_proc_heft_reads` guards it.

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

`packaging/aur/` holds `heft` (release tarball), `heft-bin` (release musl
binaries, no `depends`) and `heft-git` (main), and is their source of truth;
the AUR repositories are push targets, never edited in place. Each PKGBUILD
says why it carries `options=('!strip' '!debug')`.

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
secret is set (it skips with a notice when it is not).

The secret is deliberately not set, so that step always skips and exits green: once the
release and the job's refresh commit exist, pull main and push both by hand —
clone `ssh://aur@aur.archlinux.org/<pkg>.git`, copy `PKGBUILD`, `.SRCINFO` and
`packaging/aur/LICENSE` in, commit `heft <version>`, push `master`. Check the
result with a fresh clone, not the RPC or cgit, which lag by minutes.

`heft-git` is pushed by hand: a tag changes nothing in a package whose `pkgver()` is `git describe`.

## Gates

Contributor commands: [CONTRIBUTING.md](CONTRIBUTING.md) (same as CI / lefthook).
Suite stays under 30s. No live GPU in CI.
`live_proc::heft_does_not_grow_while_it_follows` watches heft's own RSS
growth across a `--follow` window with several PSS ticks, not a ceiling: one
tight enough to catch a leak in seconds is below what heft legitimately uses
on a busy host. It guards the `0a9ee0a` class of regression. Docker sock is
optional in CI; grouping tests use `tests/fixtures/` via `tests/grouping.rs`.

The gate is `cargo clippy --locked --all-targets -- -D warnings` at the
default level plus the `[lints.clippy]` list in `Cargo.toml`.

The wider groups are measured, not assumed, and the measurement is against that same
`--all-targets` gate: with the list below in place, `pedantic` + `nursery` +
`cargo` report 235 warnings across 23 lints, counted as clippy emits them for
every target. 134 of those are the one nursery lint `redundant_pub_crate`
objecting to a visibility style this crate keeps deliberately; most of the
rest is `too_long_first_doc_paragraph`, `option_if_let_else`, `too_many_lines`
over render functions that are one piece on purpose, and
`multiple_crate_versions` for the two `hashbrown` and two `syn` majors the
dependencies pull. Adopting those wholesale is a mechanical rewrite buying style,
and is refused. Re-measure before quoting a number here: the counts move with
every clippy release and every change to this code.

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
`map_unwrap_or` followed, all of them machine-applicable.

The four cast lints
— `cast_precision_loss`, `cast_possible_truncation`, `cast_possible_wrap`,
`cast_sign_loss` — are the ones that earn their place: a claim that a cast is
bounded lives as an `#[expect(..., reason = ...)]` at that site naming the
bound, never as prose, and the compiler checks the expect is still firing.
`cpu.rs` takes one module-level expect because every cast in it has the same
bound: one interval's `saturating_sub` delta of a kernel counter, widened into
the f64 a rate is divided in. Each lint is at zero, which is what
makes denying it free.

Release profile: LTO, `codegen-units = 1`, strip, `panic = abort`.

## Git

Remote is `Rethunk-Tech/heft`, public, Apache-2.0. `publish = false` is a
decision (HUMANS.md, Install); do not drop it without asking.
