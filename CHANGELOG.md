# Changelog

## Unreleased

### Added

- Initial `heft` TUI and `--once` / `--json` sample: Host → User
  (Applications | User Services | Containers) → System, with amdgpu fdinfo
  and read-only Docker/Podman inspect.
- User Services logical groups (GNOME Settings Daemon plugins, GVFS, Flatpak
  session helper/portal, xdg-desktop-portal family, evolution-data-server)
  merge by unit/package/D-Bus family, not by comm prefix.
