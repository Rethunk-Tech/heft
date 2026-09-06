# Changelog

## Unreleased

### Changed

- Disk rates below 1 KiB/s round instead of truncating; they no longer
  round-trip through an integer byte count.
- TUI start no longer creates `$XDG_STATE_HOME/heft` or `$XDG_CACHE_HOME/heft`.
  Only `$XDG_CONFIG_HOME/heft` is created, and only when saving a view.

### Added

- TUI `--pss-interval` (default 5s, at least `--interval`); between those
  reads heft reuses last per-PID PSS. `--once` / `--json` ignore it and
  always read PSS.
- Initial `heft` TUI and `--once` / `--json` sample: Host → User
  (Applications | User Services | Containers) → System, with amdgpu fdinfo
  and read-only Docker/Podman inspect.
- User Services logical groups (GNOME Settings Daemon plugins, GVFS, Flatpak
  session helper/portal, xdg-desktop-portal family, evolution-data-server)
  merge by unit/package/D-Bus family, not by comm prefix.
