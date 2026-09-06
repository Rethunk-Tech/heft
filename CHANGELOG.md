# Changelog

## Unreleased

First release. Nothing is tagged yet, so this is what `heft` 0.1.0 contains.

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
