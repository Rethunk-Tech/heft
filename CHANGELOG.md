# Changelog

## Unreleased

### Added

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
