# heft — agent guide

Read-only Linux process monitor. Binary name `heft`.

## Start here

@HUMANS.md — install, keys, XDG, live verify.

## Layout

```
src/main.rs          clap: TUI default, --once, --json, --interval
src/lib.rs           modules
src/types.rs         Process, Metrics, HostTree, JSON shape
src/proc.rs          every visible PID; blank metrics on EACCES
src/cpu.rs           /proc/stat split (usr/sys/wait) + per-pid utime/stime rates
src/mem.rs           meminfo used/Buffers/Cached, unified APU clip, host VRAM
src/io.rs            /proc/pid/io rates and smaps_rollup PSS
src/gpu.rs           amdgpu fdinfo; drm-client-id dedupe
src/classify.rs      launcher / worker / shell / terminal / compositor tables
src/identity.rs      cgroup parse, merge key + display name
src/containers.rs    GET-only docker/podman; project vs per-container
src/group.rs         Host → User → Applications | User Services | Containers, System
src/config.rs        XDG view.json; persist only on explicit save
src/once.rs          table and JSON
src/ui.rs            ratatui header + tree table
tests/fixtures/      GUI + docker grouping snapshots
```

No `sysinfo` crate. No `nix` unless rustix cannot do it; v1 uses `std` + `libc`.
Never read `/proc/pid/mem`. Never ptrace.

## Grouping invariants

- **Host** is the machine, not the compositor or a terminal.
- Bucket: docker/libpod scope or helper that names that id → Containers;
  `uid==0 && ppid==2` or leftover `system.slice` → System; else that uid's User.
- Under a User: user-instance unit `*.service` not starting with `app-` →
  User Services; else Applications. `init.scope` + `systemd --user` is a user
  service. Known compositors are user services even if the unit looks like an
  app.
- Identity is `exe` basename (else `comm`), not the inherited cgroup. Ignore
  terminal transients, toolkit-named Chromium scopes, and file-manager dbus
  scopes for the *name*; Chromium scopes may still be an instance key when they
  belong to this app.
- Launchers (`bwrap`, `flatpak`, `zypak-helper`, `snap-confine`, `bunx`, `npx`,
  `AppRun`, `firejail`, `xdg-dbus-proxy`, `startvesktop`) have no top-level row;
  their cost bills to the unique payload identity and they still appear inside
  the expanded process list. Nested bwrap folds into the payload. `cat` under a
  launcher or app bills to that parent; it does not break unique-payload
  folding and does not become its own row.
- Workers (`--type=*`, `chrome_crashpad_handler`, `crashhelper`, Electron `MainThread`,
  `npm`/`npx`/`node`/`python` under a real app that is not a shell/terminal/compositor/systemd)
  fold into that app. Walk ancestors skipping launchers and other generics;
  do not invent a script-basename identity (`context7-mcp`) when a launching
  agent (`claude`, `cursor`) is above. Processes stay visible on expand.
  `crashhelper` matches exe/cmdline (`/usr/lib64/firefox/crashhelper`) even
  when PPID is user systemd.
- GNOME session plumbing (`gdm-*`, `gnome-session*`, `gnome-keyring*`,
  `gnome-shell-*`, `gnome-calendar`, `gnome-clocks`, `Xwayland`, and `gjs`
  running gnome-shell Notifications/ScreenSaver) is User Services folded into
  `gnome-shell`. User-session `dbus-broker` / `dbus-broker-launch` are
  User Services, never Applications. `lying_unit` matches `dbus:` activation
  scopes, not `dbus-broker.service`. Session helpers stay User Services even
  when reparented to user systemd: `ibus-portal` / `ibus-dconf` / `ibus-x11` /
  `ibus-engine-*` → `ibus-daemon`, `at-spi2-registryd` → `at-spi-bus-launcher`,
  `goa-identity-service` → `goa-daemon`, `p11-kit-server`/`p11-kit-remote` →
  `p11-kit` (never `flatpak-session-helper`; Cursor shares that cgroup).
  A User Services logical group is one identity for processes that share a
  systemd unit family, RPM/package family, D-Bus well-known name family, or
  documented process architecture — not a comm prefix. Merge `gsd-*` plugins
  (`org.gnome.SettingsDaemon.*`, package `gnome-settings-daemon`) into
  `gnome-settings-daemon`; keep `gsd-disk-utility-notify` (`gnome-disk-utility`)
  as its own row. Merge GVFS (`gvfsd*`, volume monitors, `gvfs-*.service`,
  including `gvfs-goa-volume-monitor` and unit-bound `wsdd`) into `gvfs`, never
  into `goa-daemon`. `flatpak-session-helper` and `flatpak-portal` are `flatpak`
  session infrastructure; `xdg-dbus-proxy` with no app-flatpak cgroup folds
  there, app-bound proxy bills to that app. `xdg-desktop-portal` + backends +
  document/permission portals are `xdg-desktop-portal`. EDS factories/alarm
  notify are `evolution-data-server`. `pipewire` + `pipewire-pulse` are
  `pipewire`; `wireplumber` stays separate (different package). `gcr-ssh-agent`
  absorbs `ssh-agent` only in that unit. `abrt-applet` stays its own row.
  Independent apps (vivaldi, cursor, claude, vesktop, firefox, ghostty) never
  fold into gnome-shell or these service identities. A CLI such as `majordomo`
  (`lead get` under a ghostty transient bwrap) is Applications, not a service.
- Split when the child's resolved identity differs and the child is a real app.
  Idle interactive shells stay their own Applications row.
- Generic interpreters (`bun`, `python`, `java`, `node`, `MainThread`) fall back
  to the user unit or a distinctive script basename so they do not collapse into
  one interpreter row. `main.js` is not distinctive.
- Containers: never System, never `dockerd`/`containerd`/`engined` the user
  service. Project key is `com.supabase.cli.project` → `supabase:<name>`, else
  `supabase_<role>_<project>` names, else `com.docker.compose.project`. No
  project → one row per container name. `engined.spec` is not a merge key.
  `containerd-shim-runc-v2 -id`, `docker-proxy -container-ip`, `conmon`,
  `runc`/`crun` bill to that container. Owner: workdir path uid, else
  `engined.service` uid for `engined-*` / `engined.spec`, else Host → Containers.

## Sampler

CPU `%core` = `100 * Δ(utime+stime) / (CLK_TCK * dt)` (can exceed 100).
`%machine` = `%core / nproc`. PSS from `smaps_rollup`; RSS from `statm`.
Disk from `read_bytes`/`write_bytes`. GPU: prefer `drm-resident-vram` /
`drm-resident-gtt` over `drm-total-*`; engine ns deltas → gfx% / compute%.
Header CPU is `/proc/stat` Δ user+nice / system+irq+softirq / iowait (idle+steal
unfilled). Header MEM is one MemTotal bar when the APU VRAM carve-out is unified;
VRAM/GTT resident paint first inside used, then Cached/Buffers, then anon, clipped
so the stack never exceeds `used.min(MemTotal)`. Discrete VRAM as a second tank
is out of scope. TUI header is 2 unbordered rows; the only persistent rules are
header↔tree and tree↔footer.

TUI sampling runs on a background thread; the ratatui loop only swaps in the
last complete tree and never blocks on `/proc` I/O. `--once` / `--json` take
two snapshots `interval` seconds apart.

JSON shape: `host.users[].applications|user_services|containers`,
`host.containers`, `host.system`. Project identities include
`containers[].processes[]`.

## Gates

```
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny --locked check
cargo machete
```

Suite stays under 30s. No live GPU in CI. Docker sock is optional in CI;
grouping tests use `tests/fixtures/`.

Release profile: LTO, `codegen-units = 1`, strip, `panic = abort`.

## Conventions

- Comments explain non-obvious why, never history.
- Never suppress a linter. No `any` equivalent (`unwrap` on I/O is a bug).
- Greenfield: no shims, no dead exports.
- Conventional commits; explicit paths; no AI trailers. **Push only when told.**

## Git

Remote is `Rethunk-Tech/heft`, private.
