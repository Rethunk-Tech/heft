# Changelog

## Unreleased

### Added

- The `terminal` class includes `cosmic-term` (with the existing `cosmic-comp`
  compositor), `xterm`/`uxterm`, `qterminal`, `lxterminal`, `mate-terminal`,
  `contour`, and `blackbox-terminal`, so idle shells fold into those rows.

### Changed

- A glycin loader (`glycin-image-rs`, and other `glycin-` binaries under
  `/usr/libexec/glycin-loaders/`) bills to the one other app in its cgroup.
  Sandboxed by `bwrap`, it was its own Applications row.
- `mutter-x11-frames` (comm often truncated to `mutter-x11-fram`) folds into
  `gnome-shell`. It is the frames helper, not the `mutter` compositor.
- `--explain` is one `/proc` walk. It no longer waits `--interval` for rates
  it never prints.

## 0.12.1 - 2026-09-19

### Added

- The TUI `i` pane shows where the row landed and the placement key. A process
  row also lists the same rules stages `--explain` prints, without the
  pin-to-folder recipe; `--explain` and the pane share one renderer.
- TUI `g` jumps to a pid (expanding its ancestors), `n`/`N` walk filter
  matches, `E` expands every expandable row, and `c` restores the default
  expand set.

### Changed

- Idle CPU in the TUI and `--follow` is a quarter to a third lower than
  0.12.0, and syscalls per tick are down by more than half.
- The TUI and `--follow` keep each process's `stat`, `statm` and `io` open
  between ticks and read them with `pread`: about three open files per
  process, with the soft open-file limit raised toward the hard one.
- The TUI and `--follow` walk `/proc` with one worker instead of four. The
  first sample after start arrives later (about half a second on a
  700-process desktop); later ticks are unaffected in practice.
- Pressure (the stall trio) is read only while something shows it: a stall
  column, a stall sort, the `i` pane, or `--json`. Turned on mid-run, the
  figures are blank for the first interval.
- A process's uid, argv and cgroup are re-read every fifth sample, on exec,
  and for its first five samples, and carried otherwise; `exe` is re-read
  when `stat` shows an exec. A `setuid`, cgroup move or argv rewrite without
  an exec shows within five samples.
- uid and cgroup come from one `PIDFD_GET_INFO` call on Linux 6.13 and later,
  falling back to `status` and `cgroup` before that or under `--proc-root`.
- The Docker/Podman container list is asked for only when a container scope
  appears or goes, while an inspect is pending, or every 30 s, so a
  `docker rename` shows within 30 s.
- A PSS tick skips re-reading a process whose RSS has barely moved for up to
  six PSS periods per 512 MiB of RSS, never more than 60, instead of a flat
  six.
- A PSS tick relinks a process's fd table only when its size changed since
  the last scan, and at least every seventh scan.
- Kernel threads read only `stat`.
- `unicode-truncate` 3 and refreshed Rust lockfiles.

### Internal

- Procfs numbers parse as bytes; `/proc` and each cgroup directory are opened
  once and entries reached by name; pid-keyed maps hash with one multiply;
  carried argv and cgroup are shared rather than copied; fdinfo names and
  `net/dev` fields are read without allocating; a short procfs read ends the
  file.
- Comments and docs no longer carry benchmark figures or refused-alternative
  ledgers; `PIDFD_GET_INFO`'s kernel version and the PSS skip bound in
  HUMANS.md are corrected.

## 0.12.0 - 2026-09-16

### Changed

- `RSS` is hidden by default, like the stall trio. Summed over a row it counts
  a shared page once per process mapping it; `PSS` is the column that adds up.
  A `view.json` with a `hide_columns` key keeps its own list.
- `CPU ST`, `IO ST` and `MEM ST` now show on a row spanning several cgroups,
  as the highest member's `some` figure per resource, instead of blank.

### Removed

- `--json` identity and member-container objects no longer carry `title`; it
  always equalled `id`.

### Fixed

- A user `class` rule that makes an identity `no_absorb` now stops workers
  folding onto it; only a parent named `systemd` was refused before.
- A user `crash_helper` rule that matches by `exe` or argument now keeps that
  helper from naming its sibling's app.
- An AppImage whose app outlives its launcher's parentage, and that app's
  crash handler, now bill to the app's row instead of showing as separate
  rows.
- Cursor's bundled agent CLI, run from its `globalStorage` directory, and the
  interpreters it starts (MCP servers under `npm exec`) bill to `cursor`
  rather than an `app-cursor-<pid>` row.
- Cursor's standalone agent CLI under `~/.local/share/cursor-agent` is one
  `cursor-agent` row.
- An interpreter orphaned to `systemd --user` (a `bun` a Claude Code daemon
  left behind) bills to the app in its cgroup, and any other orphan (a
  backgrounded `wl-copy`) to the app that leads its process group.
- Firefox's GNOME search provider bills to `gnome-shell`.
- `rtk` is a launcher, so what it runs bills to the app above it; `rustc`
  bills to the `cargo` that started it.

## 0.11.1 - 2026-09-13

### Added

- `[` `]` change the sort column, like `Shift-←` `Shift-→`, for terminals
  that never pass Shift-arrows through (Konsole, the Linux console, rxvt).

## 0.11.0 - 2026-09-13

### Changed

- The TUI redraws only on a new sample, input, a resize or the pause clock
  rather than every 50 ms. Idle TUI CPU is about 2 points of a core lower, and
  sixel output drops from about 14 KB/s to 1 KB/s.
- Each pid's files are read through one `/proc/<pid>` handle and a reused
  buffer: 27% fewer syscalls per walk.
- Stall figures skip reading `cgroup.procs` for a cgroup the walk already saw
  twice, halving that pass to under 1 ms.
- TREND history pruning is a set lookup, which matters on a fully expanded
  tree (274 µs to 30 µs at 730 rows).
- Idle `--follow` and TUI sampling cost about a quarter of the CPU: a PSS
  tick skips `smaps_rollup` for a process whose RSS moved less than 1% (or
  1 MiB) since its last read, for up to six PSS periods; the walk uses at
  most four threads when following; fd tables are rescanned only on PSS
  ticks. 5.4 s to 1.5 s of CPU per 30 s on a 730-process desktop. A process's
  first GPU fd can take up to `--pss-interval` to show.
- `--follow` without `--once` or `--json` is clap's usage error.
- Containers are indexed by their 12-hex id alone; two running containers
  sharing that prefix bill to one row.
- A rule pinning `containers` or `system` as its folder fails its file with a
  warning naming the two folders a rule can use.
- `?` with the `i` pane open closes the pane and shows the key help; one
  overlay is open at a time.
- A rules.d file named exactly `.json` is not read.
- A crash handler whose install directory names no running process bills to
  its own user's app binary beside it: Vivaldi's `chrome_crashpad_handler` joins
  `vivaldi-bin` rather than taking a `vivaldi` row of its own.
- `--explain PID` exits 1 when the pid is not visible.
- `--help` for `--hide` names the labels it takes.
- Container lookups share a 2 s budget per sample. Inspects past it are
  skipped and retried on the next sample; an inspect that fails is retried
  once the set of running containers changes.

### Removed

- **Breaking:** a rules.d example carrying `kthread`, `utime` or `stime`
  fails its file. heft never read them, and `--fixture` rows do not carry
  them.
- The warning about a leftover `grouping.json`. Each of its keys is a
  placement rule in a rules.d file:

  | grouping.json | rule |
  | --- | --- |
  | `"applications": ["x"]` | `{ "id": "pin-x", "match": { "identity": "x" }, "folder": "applications" }` |
  | `"user_services": ["x"]` | the same, with `"folder": "user_services"` |
  | `"fold": { "a": "b" }` | `{ "id": "fold-a", "match": { "identity": "a" }, "fold_to": "b" }`, listed before the pins |
  | `"container_owners": { "c": 1000 }` | `{ "id": "own-c", "match": { "container": "c" }, "owner_uid": 1000 }` |

### Fixed

- An `--interval` or `--pss-interval` too large for a duration is a usage
  error rather than an abort.
- A process chain nested deeper than 48 levels no longer overflows the stack
  (the TUI aborted and left the terminal raw). Deeper processes list flat
  under their level-47 ancestor, and `--json` from such a chain still parses
  with a default JSON reader.
- A deeply nested chain of shells, launchers or app workers no longer
  overflows the stack while heft groups it; placement follows such a chain at
  most 64 levels.
- `--once`, `--explain` and `--check-rules` print control characters in
  process names and argv as visible escapes (`\u{1b}`), so a process cannot
  send escape sequences to your terminal.
- `--filter` and `/` anchor per process line: `--filter '^(code|claude)$'`
  matches, and `(?-m)` restores whole-text anchors.
- `--user` no longer causes a false `seeing N% of M threads` warning.
- `heft --explain PID | head` exits 0 rather than panicking.
- A process whose name is not valid UTF-8 no longer disappears; its uid and
  cgroup read correctly.
- heft walks `/proc` on one thread rather than aborting when it cannot start
  walk threads (`RLIMIT_NPROC`, a cgroup's `pids.max`).
- SWAP updates at the next PSS read after `swapon` or `swapoff`.
- GFX and CMP no longer jump to 100% for one tick when a process opens or
  closes a GPU client; that tick is blank.
- A Docker or Podman socket that stalls, drips bytes or has a full accept
  queue no longer hangs `--once`, `--json` or `--explain`, or freezes the TUI
  tree.
- A container whose inspect lists a `null` network keeps its IPs, NETNS
  columns and owner.
- The TUI shows a warning for a skipped rules.d file instead of drawing over
  it.
- A `view.json` that does not parse prints its path, line and column before
  heft falls back to defaults. One that leaves out `sort` or `desc` keeps its
  other settings.
- `--explain` draws its placement path in the `--glyphs` set.
- The `i` pane cuts a long command line by display width and never splits a
  character from its combining mark; the `?` overlay aligns keys by display
  width.
- Between PSS reads, a process that took over a dead process's pid shows a
  blank PSS and SWAP, not the dead process's figures.
- `--user ""` no longer matches an `/etc/passwd` line with an empty name.
- `--help` says `--filter` also matches the argv under a row, and that
  `--follow` honours `--pss-interval`.

### Documentation

- HUMANS.md states each contract once and corrects the keys, column count,
  `--filter` and `--pss-interval` statements.
- HUMANS.md says which GPU drivers and fdinfo names heft reads.

## 0.10.1 - 2026-09-13

### Added

- `10-classes.json` carries `RunC` and `CRun` examples, so `--check-rules`
  proves every container runtime name folds case (127 built-in examples).
- Tests for user `app`, `session`, `class` and `unit` rules, stage and
  placement order, `disable`, source precedence, an empty `HEFT_RULES_PATH`,
  and where processes land with a leftover grouping.json present.

### Changed

- `--check-rules` prints values the way a rules file spells them
  (`expected folder user_services, got applications`).
- Grouping allocates less per process: names are borrowed and the AppImage
  and `.mount` checks compare case in place. A multibyte character at the cut
  is a miss, never a panic.
- One function normalises container ids at every index insert and lookup.
- `--explain` builds its trace facts with the grouping code's own function,
  so the two cannot disagree on a display name.
- `rules_timing` runs from the library tests: 433 to 438 ns per process
  against a 500 ns budget.
- The pre-push gate builds with `--locked` and scans RustSec advisories once
  through `cargo deny`.

### Removed

- Test-only library items: `Rules::check`, `Rules::examples_total`,
  `Rules::count`, `Classes::contains`, `rules::OwnedFacts`, `rules::STAGES`
  and the `heft::identity_user_unit` re-export.

### Documentation

- HUMANS.md explains why GPU memory of drm clients in root-owned containers
  stays unread (reading it needs a POST), and states each measurement once;
  AGENTS.md points there.
- AGENTS.md records why the grouping.json warning stays, the clippy lint
  contract, and the bench commands. CONTRIBUTING.md lists the cargo-deny gates.

## 0.10.0 - 2026-09-13

### Added

- Grouping and classification rules are JSON files in `rules.d`, compiled in
  as built-ins. `$XDG_CONFIG_HOME/heft/rules.d/`, `/etc/heft/rules.d/` or
  `HEFT_RULES_PATH` add rules ahead of them, or `disable` a built-in file or
  rule.
- `--check-rules` runs every rules file's examples; exits 1 on a failure or a
  file that did not load.
- `--explain` prints the rule each stage matched.

### Changed

- Every grouping comparison ignores ASCII case.

### Removed

- **Breaking:** `grouping.json` is no longer read. Each key maps to a
  placement rule (the table under 0.11.0, Removed); heft warns while the
  file exists.

## 0.9.0 - 2026-09-12

### Added

- `--fixture` dumps what grouping reads, in the test fixture shape, for bug
  reports.

### Changed

- Binaries inside a VS Code, VS Code Insiders or Cursor install or extension
  tree bill to that editor; `deskflow-core` bills to `deskflow`.
- Trinity (TDE): apps tdeinit launches get their own rows, and the session
  itself is one `tdeinit` row under User Services.

## 0.8.1 - 2026-09-12

### Changed

- TREND stops at 30 cells; spare width goes to NAME.

## 0.8.0 - 2026-09-12

### Added

- The version carries the commit (`0.7.0~1a2b3c4`) on `--version`, the man
  page, and the TUI footer. Tag tarballs carry it through `.git-sha`.

### Changed

- TUI keys: Enter and Space expand and collapse; `←` `→` (`h` `l`) scroll
  columns, replacing `[` `]` and `<` `>`; `Shift-←` `Shift-→` change the sort,
  replacing `c`.
- NAME stays put while columns scroll. Spare width widens NAME until the
  longest name fits, then TREND.
- The header bars have no legend; `?` shows a swatch per segment. Segments use
  fixed xterm-256 colours and full-height fills.
- The MEM bar itemises `used` as vram, gtt, zram, shm, kernel, anon and other,
  then draws reclaimable cache, slab and buffers. `gtt` and unified `vram`
  read the kernel's `mem_info_*_used` where published. New JSON host fields:
  `gtt_used_bytes`, `mem_anon_bytes`, `mem_kernel_bytes`,
  `mem_sreclaimable_bytes`, `mem_shmem_bytes`, `zram_used_bytes`.
- CPU ST, IO ST and MEM ST are hidden by default; `--hide` adds to the saved
  list instead of replacing it.

### Fixed

- `i` detail pane: blank figures draw as `-`, the grid no longer overruns
  and wraps, and TREND is not listed.
- The header rows stop a column short of the right edge.
- The `?` overlay covers the screen, uses two columns when wide, and no
  longer cuts off on a short terminal.
- The MEM bar drew `Cached` and `Buffers` inside `used`, understating anon.

### Removed

- The `D` column, its `dstate` label and the JSON `d_state_procs` field.

## 0.7.0 - 2026-09-12

### Added

- `--trend kitty` draws TREND as one kitty-graphics-protocol image, a line
  per row, via shared memory locally and inline over ssh.
- `--trend sixel` draws the same image for xterm, foot, wezterm, konsole
  22.04+, iTerm2 and Windows Terminal 1.22+. About 559 bytes a frame, against
  about 146 KB for kitty's inline transport.
- `--trend auto` (default) asks the terminal which protocol it has: kitty
  locally, sixel over ssh, characters otherwise. A terminal that answers
  neither query delays the first frame by 400 ms.
- `--explain <PID>` shows where a process landed, its identity, and the
  override that moves it.
- `view.json` and `grouping.json` accept `//` and `/* */` comments; `s`
  writes `view.json` with a header documenting every key.

### Fixed

- `--glyphs ascii` reaches the detail pane and `--explain`.
- Truncation cuts whole grapheme clusters, not half an emoji.
- Swap has its own header row instead of halving the MEM and CPU bars.
- Header labels right-align to the longest one shown, so every bar opens in
  one column.
- Discrete VRAM takes a fixed slice of the MEMORY row instead of half.
- Host, User and folder rows have no TREND.
- TREND uses one scale per frame (100 for percentages, else the heaviest
  entry) instead of each row's own peak.

## 0.6.2 - 2026-09-12

### Added

- `--glyphs legacy`: Unicode bars with an ASCII TREND ramp, for fonts that
  have `█▓▒░` but not `▁▂▃▅▆▇`. `auto` never picks it.

### Fixed

- VRAM and GTT segments and the collapsed marker use glyphs a legacy font
  has (`▀`, `▄`, `►`). `▶` could also render double-width as an emoji.

## 0.6.1 - 2026-09-12

### Added

- AUR packages `heft`, `heft-bin` and `heft-git`.

### Fixed

- VRAM and GTT were blank on older amdgpu kernels that only publish
  `drm-memory-*`. heft now takes the first of `drm-resident-*`,
  `drm-total-*`, `drm-memory-*`.

## 0.6.0 - 2026-09-09

### Added

- A stall figure at or over 20%, or `D` above zero, draws red (reverse video
  without colour).
- heft says when it sees under 90% of the kernel's thread count (`seeing 1%
  of 4557 threads`), in the TUI footer and under the `--once` host line.
- `--proc-root DIR` reads `/proc` and `/sys` under `DIR`.
- `--json` process nodes carry `cmdline`, and documents carry
  `host.sampled_at`.
- TREND column: recent history of the sort metric. TUI only.
- `p` pauses the TUI; the footer shows how long.
- `man heft` has KEYS, FILES and ENVIRONMENT, from the same key list as `?`.
- `machine.slice` VMs and nspawn containers are Containers rows named after
  the machine.
- Rootful Podman's `/run/podman/podman.sock` is tried after the rootless one.
- `i` opens a detail pane: every column, plus pid, ppid, state, uid, `exe`,
  cgroup and command line for a process.
- `--filter` and `/` also search the argv of every process under a row.

### Fixed

- `--interval` below the 0.05 s floor, or `nan`, is a usage error instead of
  being silently raised.
- A column that does not fit is dropped rather than clipped (`20.1G` drew as
  `2`).
- cgroup v1 hosts read the right cgroup line, so User Services is no longer
  empty.
- The detail pane lays metrics across the width and caps the command line at
  240 characters, so `EXE`, `CGROUP` and `CMDLINE` stay visible.

### Documentation

- No crates.io package, deliberately; `cargo install --git` works.
- GPU columns read only `amdgpu`, `i915` and `xe`.

## 0.5.0 - 2026-09-08

### Added

- `NO_COLOR` is honoured.
- `--order` and `column_order` set column order.
- `H` hides the sort column, `u` restores the last hidden; `--hide` does the
  same for `--once`.
- Plasma session helpers merge under `plasmashell`, kwin helpers under `kwin`.
- The TUI reverses the sort column's header.
- A `D` column counts processes in uninterruptible sleep.

### Changed

- Idle interactive shells fold into their terminal.
- Folder headings count their identities; User rows show the login name only.
- `D` sits left of `%CORE`; disk rates sit after CMP.
- The CPU and MEMORY bars share one width, and the TUI header drops the host
  `psi` tail (still on `--once` and `--json`).
- Clap help and errors are uncoloured.

### Fixed

- The TUI cursor stays on the same row when the list reorders.
- The `/proc` walk reuses its threads; RSS had climbed to ~488 MiB from glibc
  arenas.
- GPU collection no longer walks every fdinfo when no fd names dri/drm, and
  skips files over 64 KiB.
- AppImage crash helpers bill to their app, including a launcher's only child.
- Empty folders are not expandable.
- `--once` measures width in terminal columns, so CJK and emoji names align.
- The help overlay's arrows follow `--glyphs`.
- A mixed-case `.AppImage` launcher bills to its payload.

## 0.4.0 - 2026-09-07

### Added

- `CPU ST`, `IO ST`, `MEM ST`: per-cgroup pressure stall percentages, plus a
  host `psi` figure. Blank unless a row is exactly one non-root cgroup.
- `--follow` keeps sampling; `--json --follow` is NDJSON.
- `--top N` keeps the N heaviest rows under each parent.
- `--desc` and `--asc`.
- `--glyphs auto|unicode|ascii`.
- `--user <NAME|UID>`, repeatable, including with `--json`. Never saved.

### Changed

- `--filter` and `/` take a case-insensitive regex (`regex-lite`).
- The `/proc` walk runs across threads: an ordinary tick went from 120 ms to
  20 ms on a 777-process host.
- Bar segments carry a distinct fill glyph as well as a colour.
- Releases add `aarch64` binaries and signed build provenance.

### Fixed

- `Ctrl-` keys no longer run their unmodified binding; `Ctrl-C` quits.
- The terminal is restored on panic and on `SIGTERM`, `SIGHUP` and `SIGQUIT`.
- Non-ASCII systemd unit names no longer render as mojibake.

## 0.3.0 - 2026-09-06

### Added

- `--sort <column>` and `--filter <text>`. Flags beat `view.json`; `--json`
  never reads it, and refuses `--filter`.
- `hide_columns` in `view.json`.
- `THR` and `AGE` columns.
- `SWAP` column (`SwapPss`) and a host swap tank.
- `NETNS RX` / `NETNS TX` on container rows.

### Changed

- The TUI without a terminal says so and names `--once` and `--json`.

### Fixed

- `heft --once | head` and `heft --json | head` exit quietly.

## 0.2.0 - 2026-09-06

### Added

- Prebuilt `x86_64` gnu and static musl binaries on every tag, with `.sha256`.
- Shell completions (bash, zsh, fish) and a man page, generated from the clap
  definition.
- `--version`.
- Optional grouping overrides in `$XDG_CONFIG_HOME/heft/grouping.json`.
- Intel i915 and xe GPU memory and engine percentages.
- Discrete GPUs get a VRAM tank against `mem_info_vram_total`; GTT stays in
  the MEM bar.

### Changed

- A container with no workdir label bills to the first non-root uid owning
  one of its bind mounts.

### Fixed

- A crash helper under a temp mount no longer becomes a row titled `mount`.

## 0.1.0 - 2026-09-06

First release.

### Added

- Fullscreen TUI, plus `--once` and `--json`, over the tree
  Host → User (Applications | User Services | Containers) → System.
- Per-row `%core` / `%machine`, PSS, RSS, disk read/write rates, and amdgpu
  VRAM / GTT / gfx% / compute%.
- Read-only Docker and Podman attribution: containers billed to their
  workload, not `dockerd` or `containerd`.
- `--interval` (default 1 s, floor 0.05 s) and TUI-only `--pss-interval`
  (default 5 s).
- Sort and filter at every tree level, saved to `view.json` with `s`.
- User Services merge by unit, package or D-Bus family.
- `$XDG_CONFIG_HOME/heft` is the only directory heft creates.
