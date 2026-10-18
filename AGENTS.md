# heft — agent guide

Read-only Linux process monitor. Binary name `heft`.

## Start here

[HUMANS.md](HUMANS.md) — install, keys, XDG, cadence, rules file format, live
verify. Read it before touching a flag, column, key or rule.

## Layout

```text
src/cli.rs           clap Cli; the one --trend / --glyphs choice list; build.rs includes it for completions/man
src/main.rs          dispatch
src/lib.rs           modules
build.rs             clap_complete + clap_mangen → OUT_DIR/assets; HEFT_VERSION; rules.d → builtin_rules.rs
rules.d/             built-in rule files, one stage each
.git-sha             export-subst commit for tag tarballs (no .git for build.rs)
src/types.rs         Process, Metrics, HostTree, JSON shape
src/proc.rs          every visible PID; blank metrics on EACCES
src/cpu.rs           /proc/stat usr/sys/wait + per-pid utime/stime rates
src/mem.rs           meminfo used/Shmem/kernel/cache/Swap, zram, unified APU clip, host VRAM
src/io.rs            /proc/pid/io rates and smaps_rollup PSS + SwapPss
src/net.rs           per-netns rx/tx; container rows only
src/psi.rs           cgroup pressure; max of member `some` on multi-cgroup rows
src/gpu.rs           amdgpu/i915/xe fdinfo; dri/drm prefilter; oversized skip; drm-client-id dedupe; fd scan on PSS ticks
src/classify.rs      crash-helper owner, launcher payload hint, interactive shell
src/identity.rs      cgroup parse, merge key + display name
src/containers.rs    GET-only docker/podman; project vs per-container
src/group.rs         Host → User → Applications | User Services | Containers, System
src/rules.rs         rules.d engine
src/config.rs        XDG view.json; strip_comments for it and rules.d
src/once.rs          columns, tree ordering, table and JSON
src/explain.rs       --explain PID
src/ui.rs            ratatui header + tree table
src/tty.rs           panic hook + signal handler; restores the terminal
src/num.rs           f64_of, sat_* — no silenced cast
src/glyph.rs         unicode, legacy or ascii; resolved once
src/keys.rs          the one TUI key list (man page too)
src/root.rs          --proc-root prefix; tests/live_proc.rs:a_proc_root_is_the_only_proc_heft_reads
src/caps.rs          --trend auto
src/kgp.rs           --trend kitty
src/sixel.rs         --trend sixel
tests/grouping.rs    tests/fixtures/; links the library
tests/live_proc.rs   real /proc; must hold in a bare container
tests/reconcile.rs   heft against /proc read independently
tests/tty_restore.rs SIGTERM/SIGHUP on a pty: terminal restored, exit 128+signal
tests/common/mod.rs  heft(), arr(), pids()
tests/fixtures/      gui/ desktop snapshot, zygote/ fallback
packaging/aur/       three PKGBUILDs + release.sh
```

No `sysinfo` crate. No `nix` unless rustix cannot do it. `std` + `libc`, plus
`rustix` for dirfd-relative `/proc/<pid>` reads. Never `/proc/pid/mem`. Never
ptrace.

## Grouping invariants

Doc comments on the functions they constrain: `group::compute_place`,
`direct_place`, `container_place`, `system_place`, `user_place`, `spawned_app`,
`owning_app_ancestor`, `unique_descendant_ident`, `lying_scope_place`,
`worker_leaf_place`, `MAX_WALK`; `identity::app_scope_names`, `unit_line`,
`generic_fallback`; `classify::name_ref`, `crash_helper_app`; `rules.rs`
module doc.

## Surfaces, flags and columns

Each of these has one home; a second copy is the defect.

| what | where | rule |
| --- | --- | --- |
| `--follow` loop | `proc::sample_stream` | honours `--pss-interval`; `once::follow_table` / `follow_json` share `render_table` and `json_text` with the one-shot pair |
| flag over `view.json` | `main::resolve_view` | `--hide` adds; every other flag overwrites. Only `sort`, `desc`, `filter`, `hide_columns`, `column_order` are read back (`users`, `top` are `#[serde(skip)]`). `--once` from `config::load_view()`; `--json` from `View::default()`. `--top`/`--hide`/`--order` `conflicts_with = "json"`. `--filter` is not: `once::keep_filtered` |
| unknown `--sort` | clap `InvalidValue` via `once::sort_labels` | not `Sort::from_label`'s fallback (stale `view.json` must not stop the monitor). `src/cli.rs` cannot reach `COLUMNS`: `build.rs` includes it standalone |
| bad regex | `once::Filter::new` → `None` | clap `InvalidValue`, a `view.json` warning, or the TUI `?` footer |
| escaping process text | `once::printable` | tree and `--json` keep raw strings; TUI relies on ratatui dropping controls |
| `--filter` / `/` haystack | `TableRow::search` / `Flat::search` (`once::haystack`) | `ProcNode::cmdline` is argv; `cwd` when non-empty. `cmdline` always serialized; `cwd` when non-empty |
| `Set::Legacy` vs `Set::Unicode` | `glyph::spark_ramp` only | `glyph.rs` says why `detect` never returns Legacy |
| `--trend kitty` / `sixel` / `auto` | `kgp.rs` / `sixel.rs` / `caps.rs` | module docs; `ui::resolve_trend`; `tty::hold_shm` |
| `i` and `?` | `ui::popup`, `ui::Overlay` | one overlay at a time |
| TREND scale | `ui::trend_scale`, one per frame | `ui::spark` and `kgp::paint` both draw against it |
| `spark` column | `Column::fmt` empty; `ui::draw` substitutes | only `Columns::for_tui` includes it; `Sort::step` skips it (`key: None`); `--order` validates `column_labels()` not `sort_labels()` |
| column model | `once::COLUMNS` via `once::Columns` | rebuilt on `H`/`u`; `config::default_hidden` hides `rss` and the stall trio when `hide_columns` is absent. `--json` ignores visibility |

## Rules

Table-shaped decisions live in `rules.d/*.json`; procedures stay Rust
(ancestor walks, `crash_helper_app`, `launcher_payload_hint`,
`generic_fallback`, container/machine/System bucketing). `build.rs` embeds
the directory sorted (`read_dir` order is machine-dependent).

User-facing format: [HUMANS.md](HUMANS.md#rules). Engine contract a change
may extend but not alter:

- Stages `unit`, `class`, `session`, `app`, `placement`. First two union;
  the rest take the first. `session` before `classify::crash_helper_app`,
  `app` after, both in `group::direct_place` —
  `tests/grouping.rs:a_user_session_rule_runs_before_the_crash_helper_and_an_app_rule_after_it`.
- ASCII case folds on both sides at compile; haystacks are never lowercased.
- Source order (XDG, `/etc`, built-ins; `HEFT_RULES_PATH` in place of the
  first two) outranks file name.
- `disable` is collected from every compiled file before any rule is kept,
  applied to that file name in every source, never undone.
- Ids `[a-z0-9-]+`, unique per file. Empty list, `all`, `any` or string
  pattern fails the file; `script` outside a class rule whose classes are
  exactly `[anonymous_script]` is a silent miss (`judged` never sets
  `Facts::script`). Half a file loaded is a set nobody wrote.
- Placement is two lists (`Rules::deciding`): containers see `owner_uid`;
  identities see `fold_to`/`folder`. `override_place` only on Applications
  and User Services. `owner_uid` before workdir/bind-mount inference.

`group::Ctx::new` builds `judged` once per pid per tick; keyed on pid, valid
for one `Ctx` (one `build_tree`). Session and app rules allocate nothing
(`rules::tests::evaluation_allocates_nothing`, counted per thread).
`HEFT_BENCH_FIXTURE` / `HEFT_BENCH_ITERS` drive the `--ignored` timing
harnesses.

`tests/common::heft` points `HEFT_RULES_PATH` at a missing directory so a
developer `/etc/heft/rules.d` cannot reach a binary-driven test.

`--check-rules` never samples. `--explain` is `sample_placement` (one walk,
no rates); `locate` recurses `ProcNode::children` and offers no key for a
container or System row. `--fixture` dumps the `tests/grouping.rs` loader
shape; `a_fixture_dump_loads_as_a_fixture` holds it. Add a field there when
grouping starts reading one.

## Sampler

| metric | formula / source |
| --- | --- |
| `%core` | `100 * Δ(utime+stime) / (CLK_TCK * dt)` (can exceed 100) |
| `%machine` | `%core / nproc` |
| RSS | `/proc/pid/statm` |
| PSS | `/proc/pid/smaps_rollup` — cadence in [HUMANS.md](HUMANS.md#run) |
| SWAP | `SwapPss:` from that rollup (`Metrics::swap_bytes`) |
| Host swap | `SwapTotal` − `SwapFree` (`SwapCached` is neither) |
| THR | `num_threads`, field 20 of the already-parsed `stat` |
| FD | last fd scan (`Process::fd_count`); blank on kernel threads and until that scan, then held like PSS |
| AGE | `now - (btime + starttime / CLK_TCK)`; field 22; aggregates in `Metrics::age_secs` |
| Disk R/W | Δ `read_bytes` / `write_bytes` from `/proc/pid/io` |
| GPU mem | first published of `gpu::MEM_PREFIXES`; `vram`/`gtt` (amdgpu), `local0`/`system0` (i915), `vram0`/`gtt` (xe) |
| NETNS RX/TX | Δ non-`lo` from `/proc/<container-scope-pid>/net/dev`; new pid or backward counter discards the interval |
| CPU/IO/MEM ST | Δ `some ... total=` µs over wall clock; several cgroups: max of members' `some`. Backward counter discards the interval |
| Host `psi` | `some avg10` from `/proc/pressure/{cpu,io,memory}`; `--once`/`--json` host line (`psi::header_tail`) |
| gfx% / compute% | `drm-engine-gfx`/`-render` and `-compute` ns deltas. xe: `drm-cycles-rcs`/`-ccs` over `drm-total-cycles-*`, each / `drm-engine-capacity-*` (`cpu::cycles_pct`). Two formulas, not unified |

| surface | rule |
| --- | --- |
| CPU bar | `/proc/stat` Δ user+nice / system+irq+softirq / iowait; idle+steal unfilled |
| MEM bar | one MemTotal width when APU VRAM is unified; order `ui::mem_key`, clip `mem::clip_used` (reclaimable cache beyond `used`) |
| Discrete VRAM | own tank against `mem_info_vram_total`; only `vram` leaves MEM; GTT stays |
| Swap | own row against `SwapTotal` (`ui::header_rows`) |
| Layout | unbordered CPU + MEMORY, SWAP where present; discrete VRAM shares MEMORY (`ui::tank_widths`); `ui::bar_prefix` + `mem_header_line` width so brackets stack |
| Disk R/W | table columns only, after compute, before the stall trio |
| THR / FD / AGE | beside `N`, before metric columns |
| CPU/IO/MEM ST | max, not sum or average. Folder/User/Host blank (`user-<uid>.slice` is not the User row; `system.slice` is not System; root-cgroup pressure is the machine's) |
| Ordering | one comparator in `once.rs`; `None` last in either direction; name breaks ties |

TUI sampling is a background thread; the ratatui loop never blocks on `/proc`.
`proc::WalkPool` is why the walk is a pool.

`ui::alarming` marks a stall column ≥ `STALL_ALARM` with `ui::alarm_style`
(red, else `REVERSED`). Reverse rather than a marker: `CPU ST` is six columns
and `100.0` is five. A large `%CORE` is work, not trouble.

JSON (`types.rs` `HostTree`): `{ "host": …, "interval_secs": … }` plus
`pss_interval_secs` on `--json --follow`, plus `warnings` when seen threads
are under 90% of the kernel count. `swap_*_bytes` omitted when total is 0,
kept (including used 0) when the host has swap. `kernel_threads` /
`walked_threads` are `#[serde(skip)]`.

## Packaging

[packaging/aur/README.md](packaging/aur/README.md).

## Gates

[CONTRIBUTING.md](CONTRIBUTING.md). Docker sock is optional in CI; grouping
uses `tests/fixtures/`.

`live_proc::heft_does_not_grow_while_it_follows` watches heft's own RSS
across a `--follow` window with several PSS ticks — not a ceiling, because
one tight enough to catch a leak in seconds is below a busy host. It guards
the walk pool: a fresh thread per walk leaves glibc arenas climbing.

Cast lints in `Cargo.toml` are denied and none are silenced: `From`/`try_from`
or `src/num.rs`.

## Git

Remote `Rethunk-Tech/heft`, public, Apache-2.0. `publish = false` is a
decision ([HUMANS.md](HUMANS.md#install)); do not drop it without asking.
