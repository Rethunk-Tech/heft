# Running heft

## Install

Grab the static build. It has no libc to match, so it runs on any distro:

```sh
curl -fsSLO https://github.com/Rethunk-Tech/heft/releases/latest/download/heft-x86_64-unknown-linux-musl
install -Dm755 heft-x86_64-unknown-linux-musl ~/.local/bin/heft
```

Every release carries four binaries — `x86_64` and `aarch64`, each in a `musl`
and a `gnu` build — with a `.sha256` beside each
(`sha256sum -c heft-<target>.sha256`). Completions (bash, zsh, fish) and a
man page are `heft-completions-man.tar.gz` on that same release, generated from
the same clap definition the binary parses:

```sh
curl -fsSLO https://github.com/Rethunk-Tech/heft/releases/latest/download/heft-completions-man.tar.gz
curl -fsSLO https://github.com/Rethunk-Tech/heft/releases/latest/download/heft-completions-man.tar.gz.sha256
sha256sum -c heft-completions-man.tar.gz.sha256
mkdir -p /tmp/heft-assets
tar -xzf heft-completions-man.tar.gz -C /tmp/heft-assets
install -Dm644 /tmp/heft-assets/heft.bash ~/.local/share/bash-completion/completions/heft
install -Dm644 /tmp/heft-assets/_heft     ~/.local/share/zsh/site-functions/_heft
install -Dm644 /tmp/heft-assets/heft.fish ~/.config/fish/completions/heft.fish
install -Dm644 /tmp/heft-assets/heft.1    ~/.local/share/man/man1/heft.1
```

The checksum proves the download arrived intact. To also prove it is the
binary this repository built, every release binary carries a signed provenance
statement:

```sh
gh attestation verify heft-x86_64-unknown-linux-musl --repo Rethunk-Tech/heft
```

Or build it:

```sh
git clone https://github.com/Rethunk-Tech/heft.git && cd heft
cargo build --release
install -Dm755 target/release/heft ~/.local/bin/heft
```

A local `cargo build` writes the same four files under a hashed `OUT_DIR`.
Take the newest `heft.1`, not the first directory named `assets`: an earlier
crate build leaves its hashed dir in `target/`, and `head -1` can install
stale completions that predate `src/cli.rs`.

```sh
man=$(find target/release/build -name heft.1 -printf '%T@ %p\n' | sort -rn | head -1 | cut -d' ' -f2-)
assets=$(dirname "$man")
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
heft --once --filter '^(code|claude)$'  # keep rows matching this regex, and their parents
heft --once --user 1000   # only this user's branch (name or uid, repeatable)
heft --once --sort age --asc   # low to high, rather than the saved direction
heft --once --top 5       # the five heaviest rows under each parent
heft --once --hide vram --hide gtt
heft --json --follow      # one JSON document per line, per interval, forever
heft --glyphs ascii       # bars and markers without block characters
```

The TUI needs a terminal. `heft > file`, or heft in a script, says so and
exits non-zero rather than falling back to `--once` behind your back.

`q`, `Esc` and `Ctrl-C` all quit. Every other `Ctrl-` or `Alt-` combination is
ignored rather than running the unmodified key's binding. If heft is killed —
`SIGTERM`, a closed terminal emulator, a logout, or a panic — it puts the
terminal back before it goes, and exits `128 +` the signal, so you are never
left at a shell with no echo.

`--interval` is the catch-all (default 1s, floor 0.05s): `/proc` walk, RSS,
io, GPU, grouping, and CPU/disk/GPU rates. The TUI sleeps `--interval` minus
sample time; a PSS pass may stretch that tick.

The walk is split across as many threads as the machine has cores, since
almost all of its time is spent waiting on the kernel to produce one small
`/proc` file at a time. On a 777-process host that takes an ordinary tick from
about 120ms to about 20ms, which is what makes the 0.05s floor usable. A PSS
tick gains less — roughly 850ms to 340ms — because `smaps_rollup` makes the
kernel walk each process's page tables, so it still stretches its interval.
This is also why heft shows itself holding more than one thread.

`--pss-interval` (default 5s, at least `--interval`) is TUI-only; between
those reads heft reuses last per-PID PSS (vanished PIDs drop). New PIDs show a
blank PSS until the next rollup.

A one-shot `--json` / `--once` takes two `/proc` walks separated by
`--interval` so rates exist, and always reads PSS on the published sample
(`--pss-interval` is ignored). Adding `--follow` makes it a continuous mode
instead, and there `--pss-interval` applies.

`--sort` takes the labels the `c` key cycles and `view.json` saves (listed
under [Hiding columns](#hiding-columns)); a name that is not one of them is a
usage error, so a typo is told to you rather than quietly sorting by PSS the
way a stale saved view does. `--filter` is the `/` key: it keeps matching rows
**and their parents**, so the tree stays a tree, and a parent still shows the
total it always did rather than the total of what survived.

The two do not search the same rows, because the two surfaces do not have the
same rows. `--once` prints down to an identity and its containers; the TUI also
has an instance row under an identity, and the individual processes under that
once you expand them. So `/firefox` in the TUI can match a row that
`--filter firefox` never sees, and the flag is the narrower of the two.

The pattern is a regex, so `--filter '^(code|claude)$'` picks exactly two rows
where a substring would also drag in every `code-helper` beside them. It is
case-insensitive unless you say otherwise — a bare `firefox` matches `Firefox`
the way it always did, and `(?-i)` turns that off. Unicode character classes
(`\p{Greek}`) are the one thing the engine leaves out. A pattern that does not
compile is a usage error on the command line, a warning-and-ignore from a saved
`view.json`, and in the TUI just a `?` in the footer: `/` recompiles as you
type and keeps filtering with the last pattern that worked, rather than
flashing the whole tree back between two keystrokes.

`--user` takes a login name or a uid and can be repeated; other User nodes
drop, while System and Host-level Containers stay, since those are the
machine's own cost and belong to nobody. A bare number is always accepted as a
uid even with no `/etc/passwd` entry, which is the ordinary case inside a
container; a name nothing answers to is a usage error.

Unlike `--filter` it works with `--json`, because the JSON tree does have User
nodes to prune, and unlike `--filter` the Host row totals what survived rather
than what it was built with — asking for one user is asking what the top row
should count.

It is never written to `view.json`, not even by `s`: a saved user cut would
hide most of the machine on every later run for a reason the file, not the
command, was keeping.

`--glyphs` chooses the characters the bars, rules and expand markers use.
`auto`, the default, reads `LC_ALL`, `LC_CTYPE` then `LANG` and uses block
characters only when one of them names a UTF-8 charmap — a bare console, the C
locale, or a container with no locale set at all gets one-column ASCII
substitutes instead of tofu. `unicode` and `ascii` force it either way, for a
terminal whose environment undersells or oversells what its font has.

`NO_COLOR` is honoured: set it to anything that is not the empty string and the
TUI draws without hue. Presence decides, not the value, so `NO_COLOR=0` and
`NO_COLOR=false` disable colour too — that is the no-color.org rule, and a
shell that exports one of those meant it. Nothing is lost by it: every bar
segment already carries its own fill character, and the legend prints that
character beside the label, so the distinction the colour was making is still
on the screen. `--once` and `--json` never emitted colour to begin with.

`--follow` keeps sampling instead of exiting after one. `--json --follow`
emits one compact document per line — NDJSON, so a reader takes a line at a
time without a streaming parser — and `--once --follow` reprints the table each
interval, each sample carrying its own header and separated by a blank line, so
any line of the stream still says which machine state it belongs to. It needs
`--once` or `--json`; the TUI is already a follow. Closing the reader ends it
quietly, so `heft --json --follow | head -5` exits 0.

Unlike the one-shot forms, `--follow` honours `--pss-interval`: reading
`smaps_rollup` for every process once a second forever is the cost that flag
exists to avoid. The first published sample reads PSS regardless, so the stream
never opens with a blank memory column.

`--top N` keeps the N heaviest rows under each parent, at every depth. It
never trims Host, a User, or a folder header: those are the shape of the tree
rather than entries competing to be heaviest, and neither Users nor the
host-level folders are ordered by the sort at all, so trimming them would drop
whichever came last — losing the whole System section on a machine that happens
to have two users. Cutting a row cuts its subtree with it.

Like `--filter`, a surviving parent still shows the total it was built with,
so `Host` keeps counting the whole machine, and `--top` applies after
`--filter`, so `--filter chrome --top 3` is the three heaviest rows that
match.

Also like `--filter`, it is refused with `--json`: the JSON shape is a
contract and a row limit is a human's presentation preference — slice it with
`jq` instead.

`--desc` and `--asc` set the direction the `d` key toggles. Without one,
`--sort age` meant whichever direction the saved view happened to hold, so the
same command printed differently on two machines.

Both flags beat a saved `view.json`. `--once` otherwise starts from that saved
view; `--json` never does — its shape is a contract, so only an explicit flag
reshapes it, and `--filter` is refused there because the JSON tree has no
folder rows for "keep the parents" to mean anything (filter it with `jq`).

## TUI keys

| key | action |
| --- | --- |
| `q` / `Esc` / `Ctrl-C` | quit |
| `↑` `↓` / `j` `k` | move the cursor |
| `PgUp` `PgDn` / `Home` `End` | page or jump the cursor |
| `←` `→` / `h` `l` / Enter / Space | collapse or expand |
| `[` `]` / `<` `>` | scroll columns when the terminal is narrower than the table |
| `/` | filter by regex on the name (Enter applies, Esc cancels) |
| `c` | cycle the sort column (default PSS descending) |
| `d` | reverse the sort direction |
| `H` | hide the current sort column (`name` is refused) |
| `u` | unhide the last hidden column |
| `s` | save sort, filter, and hidden columns to `$XDG_CONFIG_HOME/heft/view.json` |
| `?` / `F1` | toggle the key help overlay |

Sorting applies to every level of the tree. Sort, filter, and hidden columns
last only for this session until you press `s`; starting heft again loads that
file if it exists.

Default expand: Host, your user, Applications, and that user's Containers.
Other users, User Services, Host-level Containers, and System start collapsed.

## What the tree means

- **Host** is the machine. The header is two unbordered rows: stacked CPU
  (usr/sys/wait from `/proc/stat`) and a MEMORY row. Every bar segment has its
  own fill character as well as its own colour, and the legend prints that
  character beside the label (`█usr/▓sys/▒wait`), so the bars stay readable
  without colour — piped, recorded, on a monochrome terminal, or to anyone for
  whom cyan and magenta are the same hue. Where memory is unified, VRAM and
  GTT are carve-outs of that same pool, not a second tank, so the row is one
  MEMORY bar whose width is MemTotal. Both header bars are drawn to the same
  width, so the two `]` line up in one column rather than each row sizing its
  bar to whatever text happens to sit beside it. With a discrete card the
  MEMORY row splits in half: MEM against MemTotal, and VRAM against the card's
  own total, and it is the first of those the CPU bar matches. GTT stays in
  the MEM bar either way — it is system RAM pinned for the GPU, not card
  memory. That GTT slice, and the unified VRAM slice beside it, add up only
  the drm clients heft can see, the same caveat as the Host row, which sums
  only visible PIDs and so can sit below the header. How far below is not a
  rounding error. An unprivileged reader cannot open `/proc/<pid>/fdinfo` for
  a process it does not own, so every drm client inside a root-owned container
  is invisible: measured on one desktop with a single such container running,
  the kernel reported 45.7 GiB of GTT in use while heft's Host row accounted
  for 18.1 GiB of it. When the question is how much of the card is in use
  rather than which application is using it, read the kernel's own
  `/sys/class/drm/card*/device/mem_info_*` totals. Swap, when the machine has
  any, is a third tank on that same row rather than a segment of MEM: swapped
  pages are not in RAM. A machine with `SwapTotal: 0` gets no swap tank and no
  `SWAP` figures at all.

- **User** is a unix uid. Terminals, shells, and the compositor live under that
  user — not as Host.
- **Applications** vs **User Services**: a user-instance `*.service` whose name
  does not start with `app-` is a user service (`syncthing.service`,
  `org.gnome.Shell@user.service`). Known compositors and session plumbing sit
  in User Services even when D-Bus used an `app-` or `dbus:` unit. Everything
  else under the user is an application. Merge identities:
  [AGENTS.md](AGENTS.md). Plasma workspace session processes merge as
  `plasmashell`, and kwin plus its helpers as `kwin`. A sway session still
  lands in the right buckets but merges more coarsely under User Services.
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

## D

`D` counts the processes on a row in uninterruptible sleep: inside a kernel
call that cannot be interrupted, which in practice means waiting on storage or
a network filesystem. Such a process cannot be killed and gets no work done,
but `%CORE` reads it as idle, so a machine grinding to a halt on a stuck NFS
mount looks in every other column exactly like one that is quiet.

It is field 3 of `/proc/<pid>/stat`, the line heft already reads for CPU and
`THR`, so it costs no extra read.

Unlike `CPU ST` / `IO ST` / `MEM ST` it is a count rather than a percentage of
an interval, so it adds up the tree: a folder, User or Host row carries the
total of everything beneath it, and those are precisely the rows the stall
columns have to leave blank. `0` is a figure here and not a blank — every
process heft can see at all has a state, so there is nothing it can fail to
read.

A steady `0` is the ordinary reading on a healthy machine. A row that holds
above zero is waiting on something the kernel will not let it stop waiting for.

## CPU ST / IO ST / MEM ST

The stall columns say whether a row was *waiting* rather than working. `%CORE`
says a row used the processor and `PSS` says it holds memory, but neither can
tell an application that is busy from one that is stuck: both look idle in
`%CORE` while one is halfway through its work and the other has been blocked on
the disk for a second. The kernel accounts exactly that, per cgroup, in
`cpu.pressure`, `io.pressure` and `memory.pressure`, and heft reads those the
way it reads everything else — no root, no tracing.

Each figure is the percentage of the last interval during which **at least one**
task in that cgroup was stalled on the resource. That is the kernel's `some`
number, not `full`; `full` means every task was stalled at once, which on the
single-process cgroups most of a tree is made of prints the same value twice.

The columns are blank on any row that is not exactly one cgroup, and that blank
means what every other blank in heft means: no figure exists, not zero.

- A row whose processes span several cgroups is blank. A stall is a percentage
  of an interval, not a quantity, so two cgroups' figures cannot be added — a
  browser folding a dozen scopes has no single number to show. Measured on one
  desktop, 82% of rows do resolve to one cgroup and carry a figure.
- Folder rows — Applications, User Services, Containers — and the User and Host
  rows are blank for the same reason. `user-1000.slice` is *not* heft's User
  row: a rootful container lives in `system.slice` and heft still bills it to
  its owner. `system.slice` is not the System row either, since kernel threads
  sit in the root cgroup.
- A row that resolves to the root cgroup is blank, because the root cgroup's
  pressure is the machine's. Printing it on a kernel-thread row would read as
  that row's own cost — the same reason a `--network=host` container gets no
  NETNS figure.
- A process row is blank unless that process is the only one in its cgroup. A
  cgroup's stall belongs to the cgroup; showing it beside four sibling pids
  would invite reading it as four separate costs.

The `psi` figures on the `--once` and `--json` host line are a different
measurement of the same thing: those are the kernel's own 10-second averages
for the whole machine, in cpu/io/memory order, which is where a smoothed trend
reads better than an instant. The columns are per-interval deltas so they sit
on the same time base as `%CORE` and the disk rates beside them. The TUI header
leaves them out — that row exists to draw a bar to scale, and a text tail on
it made the CPU bar shorter than the MEMORY bar beside it. A machine whose
kernel was built without `CONFIG_PSI`, or booted `psi=0`, gets no host figures
and no columns at all.

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
| config | `$XDG_CONFIG_HOME/heft/view.json` (default `~/.config/heft/view.json`) | saved sort, direction, filter, and hidden columns (after `s`). `--user` and `--top` are deliberately never saved |
| config | `$XDG_CONFIG_HOME/heft/grouping.json` | your grouping overrides, if you write one |

v1 creates no `$XDG_STATE_HOME/heft` or `$XDG_CACHE_HOME/heft`. The only file
heft writes is that config directory; the only other state it touches is the
terminal it is drawing on — the alternate screen, and the termios settings it
restores on the way out. Never `/proc`, sysfs, or cgroup files.

### Hiding columns

The table has twenty columns and most terminals cannot hold them. `H` hides
the column you are sorting by (and moves the sort to the next visible one);
`u` puts the last hidden column back. `--hide` does the same for `--once`,
repeatable, and overwrites whatever `view.json` held. `s` writes the list.

```json
{ "sort": "pss", "desc": true, "hide_columns": ["vram", "gtt", "gfx", "compute"] }
```

Labels are the ones `c` cycles and `--sort` / `--hide` take: `name`, `nproc`,
`threads`, `age`, `core`, `machine`, `pss`, `rss`, `swap`, `diskr`, `diskw`,
`vram`, `gtt`, `gfx`, `compute`, `dstate`, `cpustall`, `iostall`, `memstall`,
`netns_rx`, `netns_tx`. No file, or no key, shows every column. An unknown label
in the file warns on stderr and is ignored; on `--hide` it is a usage error,
the same split as `--sort`. `name` is refused — a table of numbers with no
labels is unreadable.

Hiding is presentation only: heft reads the same `/proc` files either way, `c`
skips over what it cannot show, and `--json` ignores the list entirely. The
flag is refused with `--json`.

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
rm -f ~/.local/share/bash-completion/completions/heft
rm -f ~/.local/share/zsh/site-functions/_heft
rm -f ~/.config/fish/completions/heft.fish
rm -f ~/.local/share/man/man1/heft.1
```
