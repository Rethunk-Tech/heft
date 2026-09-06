# Running heft

## Install

Grab the static build. It has no libc to match, so it runs on any distro:

```sh
curl -fsSLO https://github.com/Rethunk-Tech/heft/releases/latest/download/heft-x86_64-unknown-linux-musl
install -Dm755 heft-x86_64-unknown-linux-musl ~/.local/bin/heft
```

Every release carries four binaries — `x86_64` and `aarch64`, each in a `musl`
and a `gnu` build — with a `.sha256` beside each
(`sha256sum -c heft-<target>.sha256`).

The checksum proves the download arrived intact. To also prove it is the
binary this repository built, every release binary carries a signed provenance
statement:

```sh
gh attestation verify heft-x86_64-unknown-linux-musl --repo Rethunk-Tech/heft
```

Or build it, which is also how you get completions and the man page:

```sh
git clone https://github.com/Rethunk-Tech/heft.git && cd heft
cargo build --release
install -Dm755 target/release/heft ~/.local/bin/heft
```

`cargo build` also generates shell completions and a man page from the same
flag definitions the binary uses, under a hashed `OUT_DIR`:

```sh
assets=$(find target/release/build -type d -name assets | head -1)
install -Dm644 "$assets/heft.bash" ~/.local/share/bash-completion/completions/heft
install -Dm644 "$assets/_heft"     ~/.local/share/zsh/site-functions/_heft
install -Dm644 "$assets/heft.fish" ~/.config/fish/completions/heft.fish
install -Dm644 "$assets/heft.1"    ~/.local/share/man/man1/heft.1
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
heft --sort rss           # start on this column instead of the saved one
heft --once --filter code # keep rows whose name contains this, and their parents
```

The TUI needs a terminal. `heft > file`, or heft in a script, says so and
exits non-zero rather than falling back to `--once` behind your back.

`--interval` is the catch-all (default 1s, floor 0.05s): `/proc` walk, RSS,
io, GPU, grouping, and CPU/disk/GPU rates. The TUI sleeps `--interval` minus
sample time; a PSS pass may stretch that tick.

The walk is split across as many threads as the machine has cores, since
almost all of its time is spent waiting on the kernel to produce one small
`/proc` file at a time. On a 777-process host that takes an ordinary tick from
about 120ms to about 20ms, which is what makes the 0.05s floor usable. A PSS
tick gains less — roughly 850ms to 340ms — because `smaps_rollup` makes the
kernel walk each process's page tables, so it still stretches its interval.
This is also why heft shows itself holding more than one thread. `--pss-interval` (default 5s,
at least `--interval`) is TUI-only; between those reads heft reuses last
per-PID PSS (vanished PIDs drop). New PIDs show a blank PSS until the next
rollup.

`--json` / `--once` take two `/proc` walks separated by `--interval` so rates
exist, and always read PSS on the published sample (`--pss-interval` is
ignored).

`--sort` takes the labels the `c` key cycles and `view.json` saves (listed
under [Hiding columns](#hiding-columns)); a name that is not one of them is a
usage error, so a typo is told to you rather than quietly sorting by PSS the
way a stale saved view does. `--filter` is the `/` key: it keeps matching rows
**and their parents**, so the tree stays a tree, and a parent still shows the
total it always did rather than the total of what survived.

Both flags beat a saved `view.json`. `--once` otherwise starts from that saved
view; `--json` never does — its shape is a contract, so only an explicit flag
reshapes it, and `--filter` is refused there because the JSON tree has no
folder rows for "keep the parents" to mean anything (filter it with `jq`).

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
  (usr/sys/wait from `/proc/stat`) and a MEMORY row. Every bar segment has its
  own fill character as well as its own colour, and the legend prints that
  character beside the label (`█usr/▓sys/▒wait`), so the bars stay readable
  without colour — piped, recorded, on a monochrome terminal, or to anyone for
  whom cyan and magenta are the same hue. Where memory is unified,
  VRAM and GTT are carve-outs of that same pool, not a second tank, so the row
  is one MEMORY bar whose width is MemTotal. With a discrete card the MEMORY
  row splits in half: MEM against MemTotal, and VRAM against the card's own
  total. GTT stays in the MEM bar either way — it is system RAM pinned for the
  GPU, not card memory. That GTT slice, and the unified VRAM slice beside it,
  add up only the drm clients heft can see, the same caveat as the Host row,
  which sums only visible PIDs and so can sit below the header. Swap, when the
  machine has any, is a third tank on that same row rather than a segment of
  MEM: swapped pages are not in RAM. A machine with `SwapTotal: 0` gets no
  swap tank and no `SWAP` figures at all.
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

## SWAP

`SWAP` is `SwapPss` from `/proc/<pid>/smaps_rollup`, the file heft already
reads for PSS, so it arrives on the same `--pss-interval` cadence and costs no
extra read. It is `SwapPss` and not `Swap` for the reason PSS is the memory
column: a swapped-out page shared by four processes is one page of swap, and
`Swap` bills it to all four, so a summed tree would report it four times.

The column is blank on a machine with no swap configured. That is the blank
contract, not a zero: with `SwapTotal: 0` there is no swap for a process to be
paged out to, so no figure exists. On a machine that does swap, `0` means that
process has nothing paged out.

## THR / AGE

`THR` is the thread count (`num_threads` from `/proc/<pid>/stat`), and it sums
up the tree the way `N` does. `N` counts processes, so before this a browser
with 40 processes holding 1400 threads looked the same as one holding 40.

`AGE` is how long ago the process started, in the largest unit that fits: `45s`,
`12m`, `3h`, `9d`. On a row that covers several processes it is the *oldest* of
them — when the thing on that row first appeared — never a sum. Both come from
the `/proc/<pid>/stat` heft already reads for CPU, so neither costs a read.

## NETNS RX / NETNS TX

Network I/O is counted per network *namespace*, never per process. A container
gets a figure because it owns a namespace; nothing else in the tree owns one,
so every other row — process, application, user service, folder, User, Host —
is blank there. That blank means heft cannot know, not that the process moved
no bytes. Linux publishes no per-process byte counter that heft could read
without CAP_NET_RAW, CAP_BPF or ptrace, all of which are outside what heft
does; htop and btop leave the column out for the same reason.

The columns are last in the table, so `]` scrolls to them on a narrow
terminal. What they sum:

- The container's own interfaces except `lo`. Loopback traffic never left the
  namespace, and counting it can multiply the figure severalfold.
- A `--network=host` container is blank: it shares the machine's namespace, so
  the only number available is the host's own lifetime traffic. `docker stats`
  reports `0B / 0B` for the same containers.
- One row per container; a compose or Supabase project row sums its
  containers.
- A restart resets the counters, so heft drops that one interval rather than
  showing a negative rate. The next sample resumes.
- Without a reachable Docker or Podman socket heft cannot tell a host-network
  container from a bridged one, so every container is blank.

## Docker / Podman

Heft `GET`s `/containers/json` and inspect on `/var/run/docker.sock`,
`$DOCKER_HOST` when it is a unix socket, or `/run/user/<uid>/podman/podman.sock`.
It never POSTs, never kills, never creates. Without a reachable socket, a
container title is `docker-<12hex>`. Stopped containers (no PID) do not appear.

## XDG

| tree | path | what |
| --- | --- | --- |
| config | `$XDG_CONFIG_HOME/heft/view.json` (default `~/.config/heft/view.json`) | saved sort + filter (after `s`), plus `hide_columns` if you write one |
| config | `$XDG_CONFIG_HOME/heft/grouping.json` | your grouping overrides, if you write one |

v1 creates no `$XDG_STATE_HOME/heft` or `$XDG_CACHE_HOME/heft`. Heft writes
only that config directory and the TTY alternate screen — never `/proc`,
sysfs, or cgroup files.

### Hiding columns

The table has seventeen columns and most terminals cannot hold them. Add
`hide_columns` to `view.json` by hand; `s` keeps whatever is already there.

```json
{ "sort": "pss", "desc": true, "hide_columns": ["vram", "gtt", "gfx", "compute"] }
```

Labels are the ones `c` cycles and `--sort` takes: `name`, `nproc`, `threads`,
`age`, `core`, `machine`, `pss`, `rss`, `swap`, `diskr`, `diskw`, `vram`,
`gtt`, `gfx`, `compute`, `netns_rx`, `netns_tx`. No file, or no key, shows
every column. An unknown label warns on stderr and is ignored, and `name` is
refused — a table of numbers with no labels is unreadable.

Hiding is presentation only: heft reads the same `/proc` files either way, `c`
skips over what it cannot show, and `--json` ignores the list entirely.

## Grouping overrides

Optional. Without the file heft groups exactly as it always has. Write
`grouping.json` yourself — heft only reads it, and a bad one warns on stderr
and is ignored rather than taking the monitor down.

Keys are the identities the tree shows you, not pids or comms.

| key | effect |
| --- | --- |
| `applications` | list of identities pinned under Applications |
| `user_services` | list of identities pinned under User Services |
| `fold` | identity → the identity it bills to instead |
| `container_owners` | container name → the uid that owns it |

```json
{
  "user_services": ["mydaemon"],
  "fold": { "mydaemon-worker": "mydaemon" },
  "container_owners": { "scratch-runner": 1000 }
}
```

An override always beats the built-in tables, but never moves a container or a
kernel thread: those rows ignore it.

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
