# Running heft

## Install

```sh
git clone https://github.com/Rethunk-Tech/heft.git && cd heft
cargo build --release
install -Dm755 target/release/heft ~/.local/bin/heft
```

Linux only. The binary reads `/proc`, `/sys/class/drm`, and (when reachable)
the Docker or Podman API over a unix socket. It never uses `sudo`.

## Run

```sh
heft                      # fullscreen TUI, 1 s catch-all / 5 s PSS
heft --once               # one table on stdout
heft --json               # one JSON tree on stdout
heft --interval 0.5       # catch-all period (TUI tick and --once/--json gap)
heft --pss-interval 5     # TUI only: how often to read smaps_rollup (default 5s)
```

`--interval` is the catch-all (default 1s, floor 0.05s): `/proc` walk, RSS,
io, GPU, grouping, and CPU/disk/GPU rates. The TUI sleeps `--interval` minus
sample time; a PSS pass may stretch that tick. `--pss-interval` (default 5s,
at least `--interval`) is TUI-only; between those reads heft reuses last
per-PID PSS (vanished PIDs drop). New PIDs show a blank PSS until the next
rollup.

`--json` / `--once` take two `/proc` walks separated by `--interval` so rates
exist, and always read PSS on the published sample (`--pss-interval` is
ignored).

## TUI keys

| key | action |
| --- | --- |
| `q` / `Esc` | quit |
| `↑` `↓` / `j` `k` | move the cursor |
| `PgUp` `PgDn` / `Home` `End` | page or jump the cursor |
| `←` `→` / `h` `l` / Enter / Space | collapse or expand |
| `[` `]` / `<` `>` | scroll columns when the terminal is narrower than the table |
| `/` | filter by name (Enter applies, Esc cancels) |
| `c` | cycle the sort column (default PSS descending) |
| `d` | reverse the sort direction |
| `s` | save the current sort and filter to `$XDG_CONFIG_HOME/heft/view.json` |
| `?` / `F1` | toggle the key help overlay |

Sorting applies to every level of the tree. Sort and filter last only for this
session until you press `s`; starting heft again loads that file if it exists.

Default expand: Host, your user, Applications, and that user's Containers.
Other users, User Services, Host-level Containers, and System start collapsed.

## What the tree means

- **Host** is the machine. The header is two unbordered rows: stacked CPU
  (usr/sys/wait from `/proc/stat`) and a MEMORY row. Where memory is unified,
  VRAM and GTT are carve-outs of that same pool, not a second tank, so the row
  is one MEMORY bar whose width is MemTotal. With a discrete card the MEMORY
  row splits in half: MEM against MemTotal, and VRAM against the card's own
  total. The Host row sums only the PIDs heft can see, so it can sit below the
  header.
- **User** is a unix uid. Terminals, shells, and the compositor live under that
  user — not as Host.
- **Applications** vs **User Services**: a user-instance `*.service` whose name
  does not start with `app-` is a user service (`syncthing.service`,
  `org.gnome.Shell@user.service`). Known compositors and session plumbing sit
  in User Services even when D-Bus used an `app-` or `dbus:` unit. Everything
  else under the user is an application. Merge identities:
  [AGENTS.md](AGENTS.md). Those merge families are GNOME-weighted, so a KDE or
  sway session lands in the right buckets but merges more coarsely under User
  Services.
- **Containers** under a user are workloads heft can attribute: the owner of
  the container's workdir label, else the owner of the first bind mount it
  has under a user's paths. Unattributed running containers sit on
  Host → Containers.
- **System** is kernel threads and leftover `system.slice` (including
  `dockerd` / `containerd`). Container scopes never go here.

Other uids appear as extra User nodes when `/proc` lists them. Metrics heft
cannot read (`smaps_rollup`, `io`, fdinfo, `exe`) render as a blank cell. A
blank is not a zero: those rows sort last whichever way the sort runs.

## Docker / Podman

Heft `GET`s `/containers/json` and inspect on `/var/run/docker.sock`,
`$DOCKER_HOST` when it is a unix socket, or `/run/user/<uid>/podman/podman.sock`.
It never POSTs, never kills, never creates. Without a reachable socket, a
container title is `docker-<12hex>`. Stopped containers (no PID) do not appear.

## XDG

| tree | path | what |
| --- | --- | --- |
| config | `$XDG_CONFIG_HOME/heft/view.json` (default `~/.config/heft/view.json`) | saved sort + filter, only after `s` |

v1 creates no `$XDG_STATE_HOME/heft` or `$XDG_CACHE_HOME/heft`. Heft writes
only that config directory and the TTY alternate screen — never `/proc`,
sysfs, or cgroup files.

## Verify

```sh
heft --once
```

`--once` should list your terminal, your interactive shells, and each running
desktop app as its own Applications row — an Electron app appears once, not
once per helper process. User Services should hold your compositor and any
user `*.service` units. Containers should sum a compose project under one row
and bill `containerd-shim` / `conmon` / `runc` to their container, never to
`dockerd`.

Contributor gates live in [CONTRIBUTING.md](CONTRIBUTING.md).

## Uninstall

```sh
rm -f ~/.local/bin/heft
rm -rf ~/.config/heft
```
