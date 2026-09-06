# heft — agent guide

Read-only Linux process monitor. Binary name `heft`.

## Start here

@HUMANS.md — install, keys, XDG, sample cadence, live verify.

## Layout

```
src/main.rs          clap: TUI default, --once, --json, --interval, --pss-interval
src/lib.rs           modules
src/types.rs         Process, Metrics, HostTree, JSON shape
src/proc.rs          every visible PID; blank metrics on EACCES
src/cpu.rs           /proc/stat split (usr/sys/wait) + per-pid utime/stime rates
src/mem.rs           meminfo used/Buffers/Cached, unified APU clip, host VRAM
src/io.rs            /proc/pid/io rates and smaps_rollup PSS
src/gpu.rs           amdgpu fdinfo; dri/drm prefilter; full walk on PSS/--once; drm-client-id dedupe
src/classify.rs      launcher / worker / shell / terminal / compositor tables
src/identity.rs      cgroup parse, merge key + display name
src/containers.rs    GET-only docker/podman; project vs per-container
src/group.rs         Host → User → Applications | User Services | Containers, System
src/config.rs        XDG view.json; persist only on explicit save
src/once.rs          columns, tree ordering, table and JSON
src/ui.rs            ratatui header + tree table
tests/grouping.rs    integration tests over tests/fixtures/
tests/fixtures/      GUI grouping snapshot
```

No `sysinfo` crate. No `nix` unless rustix cannot do it; v1 uses `std` + `libc`.
Never read `/proc/pid/mem`. Never ptrace.

## Grouping invariants

- **Host** is the machine, not the compositor or a terminal.
- Bucket (`src/group.rs`): docker/libpod scope or helper that names that id →
  Containers; `identity::is_kernel` or leftover `system.slice`
  (`in_system_slice` and not `in_user_slice`) → System; else that uid's User.
- Under a User: user-instance unit `*.service` not starting with `app-` →
  User Services; else Applications. `init.scope` + `systemd --user` is a user
  service. Known compositors (`classify::is_compositor`) are user services even
  if the unit looks like an app.
- Display name is `classify::name_of` (`exe` basename else `comm`), not the
  inherited cgroup. `identity::lying_unit` skips terminal transients, Chromium
  toolkit scopes (`org.chromium.chromium`), `dbus:` activation, `run-u*`, and
  `flatpak-session-helper` for unit-based identity. `instance_key` uses a real
  user unit only when it is not lying; otherwise `pgid`.
- Launchers (`classify.rs` `LAUNCHERS`, plus names ending `.appimage`) have no
  top-level row; cost bills to the unique payload identity; they still appear
  inside the expanded process list. Nested bwrap folds into the payload. `cat`
  under a launcher or app bills to that parent; it does not break unique-payload
  folding and does not become its own row.
- Workers (`classify::is_worker`) fold into that app. Walk ancestors skipping
  launchers and other generics; do not invent a script-basename identity
  (`context7-mcp`) when a launching agent (`claude`, `cursor`) is above.
  Processes stay visible on expand. `crash_helper_app` matches exe/cmdline
  even when PPID is user systemd.
- User Services grouping is one identity for processes that share a systemd
  unit family, RPM/package family, D-Bus well-known name family, or documented
  process architecture — not a comm prefix. Mappings live in
  `classify::session_helper_ident`. Exceptions: prefix lookalikes with a
  different product stay out (`gsd-disk-utility-notify`, independent `wsdd`,
  `wireplumber`); independent apps never fold into gnome-shell or these
  service identities; an arbitrary user CLI is Applications;
  `p11-kit` must not fold into `flatpak-session-helper` (Cursor shares that
  cgroup); user-session `dbus-broker` is User Services, never Applications
  (`lying_unit` matches `dbus:` activation, not `dbus-broker.service`);
  app-bound `xdg-dbus-proxy` bills to that app, unbound folds into `flatpak`;
  `gcr-ssh-agent` absorbs `ssh-agent` only in that unit.
- Split when the child's resolved identity differs and the child is a real app.
  Idle interactive shells stay their own Applications row.
- Generic interpreters (`classify.rs` `GENERICS`) fall back to the user unit or
  a distinctive script basename (`identity::generic_fallback`) so they do not
  collapse into one interpreter row. Non-distinctive script basenames live in
  that function.
- Containers: never System, never `dockerd`/`containerd`. Project key is
  `com.supabase.cli.project` → `supabase:<name>`, else
  `supabase_<role>_<project>` names, else `com.docker.compose.project`. No
  project → one row per container name; a vendor-specific label is not a merge
  key. `containerd-shim-runc-v2 -id`, `docker-proxy -container-ip`, `conmon`,
  `runc`/`crun` bill to that container. Owner:
  workdir path uid, else the first non-root uid owning an `Inspect.Mounts`
  bind source (named volumes are root-owned and skipped), else Host →
  Containers.

## Sampler

| metric | formula / source |
| --- | --- |
| `%core` | `100 * Δ(utime+stime) / (CLK_TCK * dt)` (can exceed 100) |
| `%machine` | `%core / nproc` |
| RSS | `/proc/pid/statm` |
| PSS | `/proc/pid/smaps_rollup` — cadence in [HUMANS.md](HUMANS.md) |
| Disk R/W | Δ `read_bytes` / `write_bytes` from `/proc/pid/io` |
| GPU mem | prefer `drm-resident-vram` / `drm-resident-gtt` over `drm-total-*` |
| gfx% / compute% | engine ns deltas from amdgpu fdinfo |

| surface | rule |
| --- | --- |
| CPU bar | `/proc/stat` Δ user+nice / system+irq+softirq / iowait; idle+steal unfilled |
| MEM bar | one MemTotal width when APU VRAM is unified; VRAM/GTT resident, then Cached/Buffers, then anon; clip so the stack never exceeds `used.min(MemTotal)` (`mem::clip_used`) |
| Discrete VRAM | out of scope as a second tank |
| Layout | 2 unbordered header rows; persistent rules: header↔tree and tree↔footer |
| Disk R/W | table columns only (formatted rates change width every tick) |
| Ordering | one comparator in `once.rs` for every level; a `None` metric sorts last in either direction, name breaks ties, stable over `group::proc_forest` pid order |

TUI sampling runs on a background thread; the ratatui loop only swaps in the
last complete tree and never blocks on `/proc` I/O. Sample cadence (`--interval`,
`--pss-interval`, `--once` / `--json`): [HUMANS.md](HUMANS.md).

JSON shape (`src/types.rs` `HostTree`): `host.users[].applications|user_services|containers`,
`host.containers`, `host.system`. Project identities include
`containers[].processes[]`.

## Gates

Contributor commands: [CONTRIBUTING.md](CONTRIBUTING.md) (same as CI / lefthook).
Suite stays under 30s. No live GPU in CI. Docker sock is optional in CI;
grouping tests use `tests/fixtures/` via `tests/grouping.rs`.

Release profile: LTO, `codegen-units = 1`, strip, `panic = abort`.

## Git

Remote is `Rethunk-Tech/heft`, public, Apache-2.0.
