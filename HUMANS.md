# Running heft

## Install

```sh
git clone git@github.com:Rethunk-Tech/heft.git && cd heft
cargo build --release
install -Dm755 target/release/heft ~/.local/bin/heft
```

Linux only. The binary reads `/proc`, `/sys/class/drm`, and (when reachable)
the Docker or Podman API over a unix socket. It never uses `sudo`.

## Run

```sh
heft                 # fullscreen TUI, 1 s refresh
heft --once          # one table on stdout
heft --json          # one JSON tree on stdout
heft --interval 0.5  # sample period in seconds (TUI tick and --once/--json gap)
```

`--json` implies a single sample (two `/proc` walks separated by `--interval`
so CPU, disk, and GPU engine rates exist). `--once` does the same as a table.

## TUI keys

| key | action |
| --- | --- |
| `q` | quit |
| `↑` `↓` / `j` `k` | move the cursor |
| `←` `→` / `h` `l` / Enter / Space | collapse or expand |
| `[` `]` | scroll columns when the terminal is narrower than the table |
| `/` | filter by name (Enter applies, Esc cancels) |
| `c` | cycle the sort column (default `%mach` descending) |
| `d` | reverse the sort direction |
| `s` | save the current sort and filter to `$XDG_CONFIG_HOME/heft/view.json` |
| `?` / `F1` | toggle the key help overlay |

Sort and filter last only for this session until you press `s`. Starting heft
again loads that file if it exists.

Default expand: Host, your user, Applications, and that user's Containers.
Other users, User Services, Host-level Containers, and System start collapsed.

## What the tree means

- **Host** is the machine. The header CPU / RAM / VRAM numbers come from
  world-readable `/proc/stat`, `/proc/meminfo`, and `/sys/class/drm`. Host row
  sums only the PIDs heft can see, so it can sit below the header.
- **User** is a unix uid. Terminals, shells, and the compositor live under that
  user — not as Host.
- **Applications** vs **User Services**: a user-instance `*.service` whose name
  does not start with `app-` is a user service (`earshotd.service`,
  `engined.service`, `org.gnome.Shell@user.service`). Everything else under the
  user is an application.
- **Containers** under a user are workloads heft can attribute (workdir owner,
  or `engined-*` / `engined.spec` using the `engined.service` uid). Unattributed
  running containers sit on Host → Containers.
- **System** is kernel threads (`ppid == 2`) and leftover `system.slice`
  (including `dockerd` / `containerd`). Container scopes never go here.

Other uids appear as extra User nodes when `/proc` lists them. Metrics heft
cannot read (`smaps_rollup`, `io`, fdinfo, `exe`) render as a blank cell.

## Docker / Podman

Heft `GET`s `/containers/json` and inspect on `/var/run/docker.sock`,
`$DOCKER_HOST` when it is a unix socket, or `/run/user/<uid>/podman/podman.sock`.
It never POSTs, never kills, never creates. Without a reachable socket, a
container title is `docker-<12hex>`. Stopped containers (no PID) do not appear.

## XDG

| tree | path | what |
| --- | --- | --- |
| config | `$XDG_CONFIG_HOME/heft/view.json` (default `~/.config/heft/view.json`) | saved sort + filter, only after `s` |
| state | `$XDG_STATE_HOME/heft/` | reserved; unused in v1 |
| cache | `$XDG_CACHE_HOME/heft/` | reserved; unused in v1 |

Heft may write those directories and the TTY alternate screen. It does not
write `/proc`, sysfs, or cgroup files.

## Verify

```sh
cargo test
cargo clippy --all-targets -- --deny warnings
cargo fmt --check
heft --once
```

`--once` should list your terminal, interactive shells, and any of `claude`,
`cursor`, `easyeffects`, `minecraft-launcher`, `signal-desktop`, `spotify`,
`vesktop.bin`, `vivaldi-bin` that are running; User Services should include
`earshotd` / `engined` / the compositor when those units exist; Containers
should split `engined-*` and sum a Supabase/compose project rather than
billing `dockerd`.

## Uninstall

```sh
rm -f ~/.local/bin/heft
rm -rf ~/.config/heft ~/.local/state/heft ~/.cache/heft
```
