# Changelog

## Unreleased

### Added

- `--sort <column>` and `--filter <text>`, so a script gets the top rows
  without piping through `sort` and losing the indentation that makes the tree
  readable. `--sort` takes the same labels the `c` key cycles and `view.json`
  saves, so all three name a column the same way, and an unknown one is a usage
  error rather than a fallback: a stale saved view must not stop the monitor,
  but a name just typed can still be corrected. `--filter` is the `/` key and
  runs through the same `keep_matches`, so it keeps the ancestors of a match —
  a filter that dropped them would print an orphaned tree — and those ancestors
  keep the totals they were built with rather than the totals of what survived.
  An explicit flag beats a saved `view.json`. `--once` otherwise starts from
  that saved view; `--json` never reads it, because its shape is a documented
  contract and a human's TUI preference is not part of it, and `--filter` is
  refused with `--json` since the JSON tree has no folder rows for "keep the
  ancestors" to mean anything there.

- `hide_columns` in `view.json`: a list of column labels the table leaves out.
  Seventeen columns no longer fit a terminal, which is why `[` and `]` exist,
  and scrolling past a column every tick is not the same as never wanting it.
  It is a hide list rather than a show list because heft keeps growing columns
  and a show list would silently withhold every one added after the file was
  written. It sits in `view.json` beside the sort and filter the `s` key
  already saves, not in `grouping.json`, which is read-only and about identity
  rather than presentation. Absent file or absent key renders exactly as
  before. `name` is refused, since a table of numbers with no labels is
  unreadable, and an unknown label warns on stderr and is ignored the way a
  malformed `grouping.json` does — not `Sort::from_label`'s silent fallback,
  because a mistyped sort still prints a usable table while a mistyped hide
  entry would do nothing and say nothing. Hiding reaches presentation only:
  the same `/proc` files are read either way, so every roll-up invariant still
  holds, the `c` cycle skips what it cannot show rather than moving the sort
  somewhere invisible, and `--json` ignores the list entirely — a consumer
  parsing the tree did not ask for a human's column preference.

- `THR` and `AGE` columns, both from the `/proc/<pid>/stat` heft already parses
  for CPU, so neither adds a per-tick read. `N` counts processes, so a thread
  leak was invisible: one process holding 4000 threads rendered identically to
  one holding none. Threads sum up the tree the way `N` does. `AGE` is
  `now - (btime + starttime / CLK_TCK)`, shown in the largest unit that fits
  (`45s`, `12m`, `3h`, `9d`) rather than raw seconds or a start timestamp that
  would leave the reader to do the subtraction. On a row covering several
  processes it is the oldest of them, which is when the thing on that row first
  appeared; it is deliberately not a sum, since summed durations mean nothing,
  and a maximum cannot be misread as a total. `btime` is read from `/proc/stat`
  once per run and pinned, so an NTP step cannot walk ages heft has already
  printed.

- A `SWAP` column and a host swap tank on the MEMORY header row. Both come from
  files heft already opens — `SwapPss:` from the `smaps_rollup` it reads for
  PSS, and `SwapTotal`/`SwapFree` from the `/proc/meminfo` it reads for the MEM
  bar — so neither costs a tick any extra I/O, and per-process swap arrives on
  the existing `--pss-interval` cadence. The column is `SwapPss` rather than
  `Swap` for the reason PSS is the memory column: `Swap` bills a shared
  swapped-out page to every process mapping it, which a summed tree then
  reports several times over. Swap is a tank of its own and never a segment of
  the MEM bar, because swapped pages are not in RAM. A machine with
  `SwapTotal: 0` renders exactly as before: no tank, no swap field on the
  `--once` host line, and blank `SWAP` cells rather than a column of zeros,
  since with no swap configured there is no figure to report. Until now a
  process with gigabytes paged out showed only its small resident RSS and PSS,
  and heft's memory picture was actively misleading on any host that swaps.

- `NETNS RX` / `NETNS TX` on container rows, from the non-`lo` interfaces of
  `/proc/<pid>/net/dev`. Every other row is blank, and the columns are named
  for the network namespace rather than the resource because that is what the
  counter belongs to: a container gets a figure by owning a namespace, and
  nothing else in the tree owns one. Per-process network I/O is not deferred,
  it is unavailable without CAP_NET_RAW, CAP_BPF or ptrace — see
  [CONTRIBUTING.md](CONTRIBUTING.md) for what was measured. A
  `--network=host` container stays blank, since its counters are the
  machine's, and a restart drops one interval instead of reporting a negative
  rate.

### Changed

- Running the TUI without a terminal on stdout now says so and names `--once`
  and `--json`, instead of `No such device or address (os error 6)`. The check
  runs before the first `/proc` walk, since surfacing it afterwards would burn
  a whole `--interval` first. Still non-zero — it is a usage error — and
  deliberately not a quiet fall back to `--once`, which would surprise anyone
  piping heft expecting a TUI.

### Fixed

- `heft --once | head` and `heft --json | head` exit quietly instead of
  reporting a broken pipe. `--json` previously panicked outright, which the
  `panic = abort` release profile turns into an abort. A genuine write failure
  such as a full disk is still reported.

## 0.2.0 - 2026-09-06

### Added

- Prebuilt binaries on every `v*` tag, so installing no longer needs a Rust
  toolchain. Each release carries `x86_64-unknown-linux-gnu` and a static
  `x86_64-unknown-linux-musl` build that runs on any glibc, plus a `.sha256`
  beside each one.

- Shell completions (bash, zsh, fish) and a `heft.1` man page, generated at
  build time from the same clap definition the binary parses, so they cannot
  drift from the real flags. `clap_complete` and `clap_mangen` are
  build-dependencies only; the shipped binary gains nothing. Install paths:
  [HUMANS.md](HUMANS.md).

- Optional grouping overrides in `$XDG_CONFIG_HOME/heft/grouping.json`, so a
  local daemon, worker, or container no longer needs a patch to heft's compiled
  tables. `applications` / `user_services` pin an identity to a folder, `fold`
  bills one identity to another, and `container_owners` pins a container to a
  uid. Heft only reads the file; with none present grouping is unchanged. An
  override beats every built-in table but cannot move a container or a kernel
  thread, and a malformed file warns on stderr instead of stopping the monitor.

- Intel GPUs report VRAM/GTT and gfx%/compute% instead of blank columns. The
  driver gate accepted only `amdgpu`, so i915 and xe users saw nothing; their
  fdinfo region and engine names now map onto the same counters. NVIDIA still
  needs NVML and stays out.

- Intel Xe/Arc reports gfx% and compute%. xe publishes engine busy as GPU
  cycles against `drm-total-cycles-*` rather than nanoseconds, so those columns
  were blank; they now come from that cycle ratio, divided by
  `drm-engine-capacity-*` where a class has several engine instances. amdgpu
  and i915 keep the nanoseconds-over-wall-clock rate unchanged.

- Discrete GPUs get a VRAM bar measured against the card's own
  `mem_info_vram_total`. It shares the MEMORY header row with the MEM bar
  rather than adding a third row, so a desktop-GPU user no longer sees VRAM
  silently dropped from the header.

- GTT now paints in the MEM bar on a discrete card, and on any GPU heft has no
  `mem_info_vram_total` for. Splitting VRAM into its own tank had dropped GTT
  from the header entirely, even though GTT is system RAM pinned for the GPU
  and is already inside `used`. The figure sums `drm-resident-gtt` over the drm
  clients heft can see, not the sysfs `mem_info_gtt_total` capacity.

### Changed

- Container ownership comes from bind-mount sources: a container with no
  workdir label now bills to the first non-root uid owning one of its
  `Mounts` bind sources. Named volumes are skipped (root-owned under
  `/var/lib/docker/volumes`), and a container that mounts only root-owned
  paths still sits on Host → Containers, so any tool laying out per-user bind
  mounts is attributed rather than only one vendor's.

### Fixed

- A crash helper unpacked under a temp root no longer invents an Applications
  row from the mount directory (`/tmp/mount` rendered as a row literally
  titled `mount`). AppImages mount at a per-run path, so that name was never
  an app identity; the helper now bills to the ancestor that launched it.

## 0.1.0 - 2026-09-06

First release.

### Added

- Fullscreen TUI, plus `--once` and `--json` for one sample, over the tree
  Host → User (Applications | User Services | Containers) → System.
- Per-row `%core` / `%machine`, PSS, RSS, disk read/write rates, and amdgpu
  VRAM / GTT / gfx% / compute% from fdinfo.
- Read-only Docker and Podman attribution: `GET` only, containers billed to
  their workload rather than to `dockerd` or `containerd`.
- `--interval` catch-all sampling (default 1s, floor 0.05s) and TUI-only
  `--pss-interval` (default 5s). Between PSS reads heft reuses the last
  per-PID value; `--once` / `--json` always read PSS on the published sample.
- Sort and filter across every level of the tree, saved to
  `$XDG_CONFIG_HOME/heft/view.json` with `s`. Default is PSS descending, and a
  metric heft cannot read sorts last rather than as a zero.
- User Services logical groups (GNOME Settings Daemon plugins, GVFS, Flatpak
  session helper/portal, xdg-desktop-portal family, evolution-data-server)
  merge by unit/package/D-Bus family, not by comm prefix.
- `$XDG_CONFIG_HOME/heft` is the only directory heft creates, and only when
  you save a view. No state or cache tree.
