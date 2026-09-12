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

On Arch, three AUR packages carry the same binary three ways:

```sh
paru -S heft-bin   # the release musl binary, no toolchain
paru -S heft       # built from the release tarball
paru -S heft-git   # built from main
```

All three install the completions and the man page, and all three `provide`
and `conflict` with `heft`, so only one is ever on a machine. `heft-bin` is
the one to pick unless you want the compile: it is the same static binary this
page starts with, so its package has no dependencies at all.

Or build it:

```sh
git clone https://github.com/Rethunk-Tech/heft.git && cd heft
cargo build --release
install -Dm755 target/release/heft ~/.local/bin/heft
```

There is no crates.io package, deliberately: `cargo install heft` would put a
Rust toolchain and a two-minute compile between you and a monitor you want
running now, and the musl binary above is static, so it needs neither. If you
would rather build than download, `cargo install --git
https://github.com/Rethunk-Tech/heft` does the same as the clone above.

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
heft --once --order pss --order rss --order core
heft --json --follow      # one JSON document per line, per interval, forever
heft --glyphs ascii       # bars and markers without block characters
heft --glyphs legacy      # block bars, but an ASCII TREND ramp
heft --proc-root /mnt/tree   # read /proc and /sys under here instead of /
```

The TUI needs a terminal. `heft > file`, or heft in a script, says so and
exits non-zero rather than falling back to `--once` behind your back.

`q`, `Esc` and `Ctrl-C` all quit. Every other `Ctrl-` or `Alt-` combination is
ignored rather than running the unmodified key's binding. If heft is killed —
`SIGTERM`, a closed terminal emulator, a logout, or a panic — it puts the
terminal back before it goes, and exits `128 +` the signal, so you are never
left at a shell with no echo.

A value below the 0.05s floor is a usage error rather than being quietly
raised to it: heft would have run at 0.05 either way, and you would have been
computing rates against a cadence it was never using. Same rule as a mistyped
`--sort`.

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
under [Columns](#columns)); a name that is not one of them is a
usage error, so a typo is told to you rather than quietly sorting by PSS the
way a stale saved view does. `--filter` is the `/` key: it keeps matching rows
**and their parents**, so the tree stays a tree, and a parent still shows the
total it always did rather than the total of what survived.

The two do not search the same rows, because the two surfaces do not have the
same rows. `--once` prints down to an identity and its containers; the TUI also
has an instance row under an identity, and the individual processes under that
once you expand them. So `/firefox` in the TUI can match a row that
`--filter firefox` never sees, and the flag is the narrower of the two.

What it searches is the row's name **and the argv of every process under it**.
A row title is an exe basename, so four `python3` workers look identical until
you can ask which one holds `--port 8080`; `--filter 'port 8080'` keeps that
one and its parents. The argv is matched whether or not the row is expanded, so
`/` in the TUI finds a collapsed identity by an argument of a process you
cannot see yet. It costs nothing on a tick with no filter, since the argv is
only assembled for a tick that searches it.

The pattern is a regex, so `--filter '^(code|claude)$'` picks exactly two rows
where a substring would also drag in every `code-helper` beside them — though
`^` and `$` anchor to one process's argv rather than to the row name, since
each process's argv is its own line in what the pattern searches. It is
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

`legacy` is for the font in between, and there are many of them: it draws the
bars, the rules and the tree markers in Unicode, and only TREND in ASCII. A
font can carry `█▓▒░`, the half blocks `▀▄` and the triangles `▼►` — everything
the header and the tree are made of — and still not carry `▁▂▃▅▆▇`, which is
six of the sparkline's eight steps. On such a terminal everything renders
except one column, and `ascii` would be a heavy answer to that: it would throw
away a header that was drawing correctly. Nothing heft can read says what a
font covers, so `auto` never picks this — you ask for it when the trend column
comes up as boxes and the rest of the screen is fine.

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

Every JSON document carries `host.sampled_at`, the unix second the sample was
taken. A stream has no other clock in it, so two lines otherwise say nothing
about how far apart they were read — an interval the sampler stretched to
finish a PSS pass is invisible without it. It is on the one-shot `--json` too,
where it dates the snapshot.

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

`--proc-root` points heft at a `/proc` and `/sys` somewhere other than `/`:
another mount namespace's procfs, or a tree captured off a machine you cannot
run heft on. A directory with no `proc` in it is a usage error rather than a
tree of blanks, which would read as a permissions problem.

Two things stay the host's. The container socket is live IPC, not a file in
that tree, so container rows describe the runtime heft can reach; and user
names come from the host's `/etc/passwd`, so a uid with no entry there shows
as a number. Neither is guesswork heft could do better.

The header still reads the machine, because a procfs bind-mounted into a PID
namespace serves the host's own `meminfo` and `stat` — the kernel does not
virtualise those, so nothing heft could read there would be namespace-local.

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
| `i` | detail for the row under the cursor (`i` or Esc closes) |
| `p` | pause the table; the footer says how long it has been frozen |
| `s` | save sort, filter, hidden columns, and column order to `$XDG_CONFIG_HOME/heft/view.json` |
| `?` / `F1` | toggle the key help overlay |

Sorting applies to every level of the tree. The TUI reverses the header of
the column `c` is sorting by, so the sort is on the table as well as in the
footer (`c sort (pss)`). Sort, filter, hidden columns, and column order last
only for this session until you press `s`; starting heft again loads that file
if it exists.

Default expand: Host, your user, Applications, and that user's Containers.
Other users, User Services, Host-level Containers, and System start collapsed.
A folder with nothing in it is not expandable — it does not draw as open over
an empty gap — and becomes one when a row appears.

The highlight stays on the same row when the list reorders or shrinks — a
new sample, `c`/`d`, `/`, `--top`, or expand/collapse. If that row has gone
(process exited, filter dropped it), the nearest parent still on screen is
selected.

`i` opens a detail pane for the row under the cursor. It lists **every**
column for that row — the ones `H` hid and the ones the terminal is too narrow
to reach included — so you can read the whole row at once rather than scrolling
it past with `[` and `]`. A blank there is the blank the table would show: no
figure exists, which is not a zero.

When the cursor is on a single process it also prints what that process *is*,
which no column says: pid, parent pid, state, owning uid, the `exe` path, the
cgroup line, and the full command line. That last one is usually the answer —
four helpers named `cursor` differ only in their `--type=`, and the tree shows
you four rows called `cursor`. Those seven facts are read from `/proc` when you
press the key rather than carried on every process of every sample, so the pane
costs nothing until you open it, and a field heft may not read (another user's
`exe`) is simply blank.

The metrics lay out across the pane rather than down it, so the pane stays a
few lines tall and the `/proc` facts below them are on screen rather than off
the bottom. A command line is cut at 240 characters: a Chromium helper's argv
runs to thousands, and one `--enable-features=` list would push every other
fact off the pane. The front is the part that says which helper this is.

`p` freezes the table so you can read across a row without the numbers moving
under you. Sampling carries on underneath, so unpausing shows the machine as it
is rather than replaying a backlog, and sorting, filtering, expanding and `i`
all still work on the held tree. The footer reads `PAUSED 8s` while it is
frozen: a monitor that has stopped updating and does not say so is how a stale
number gets read as a current one.

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

- **User** is a unix uid, shown by login name. Terminals and the compositor
  live under that user — not as Host. An idle interactive shell folds into
  that terminal; a shell that launched an app bills to the app.
- Folder headings — Applications, User Services, Containers, System — carry
  the count of identities under them, including zero. That count is the
  entries, not `N` (processes).
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
- A **virtual machine or nspawn container** — anything systemd put in
  `machine.slice` — is a Containers row named after the machine. libvirt calls
  a domain's scope `machine-qemu\x2d3\x2dfedora.scope`, and the counter in
  there is libvirt's own, so the row reads `fedora`. These sit on Host →
  Containers: there is no Docker or Podman API to ask who owns one, and the
  uid running it is a service account rather than a person. Rootful Podman
  shares that slice but is a `libpod-` scope, so it is still a container row
  with its real name and owner.

The GPU columns read DRM fdinfo, and only from `amdgpu`, `i915` and `xe` — the
three drivers whose region and engine key names heft knows. Every other driver
is refused rather than guessed at, so `nvidia-drm`, `nouveau`, and the ARM SoC
drivers (`panfrost`, `v3d`, `msm`) leave VRAM, GTT, gfx% and compute% blank on
a machine that has a working GPU. That blank is the ordinary blank contract:
no figure exists, because reading a key by the name another driver happens to
use is how you print a confident wrong number.

Other uids appear as extra User nodes when `/proc` lists them. Metrics heft
cannot read (`smaps_rollup`, `io`, fdinfo, `exe`) render as a blank cell. A
blank is not a zero: those rows sort last whichever way the sort runs.

## TREND

`TREND` (`spark`) draws the last nine samples of whatever column you are
sorting by, as rising blocks. Every other column is the current interval only,
so a process that spiked to 400% and went quiet looked exactly like one that
was idle throughout — by the time you read the row, the spike had already been
overwritten.

It scales to that row's own peak, so the shape answers "when was this row
busy", not "how does it compare to the machine": a row that touched 400% and a
row that touched 4% both draw a full block at their own maximum. A row that has
been flat at zero draws a flat line along the bottom, because that is a history
and it is flat. A row that has only just appeared is blank — heft's usual
blank, no figure yet.

Changing the sort column clears it. The buffer would otherwise hold two
different metrics in two different units and draw them as one picture.

If TREND is the one column that comes up as boxes while the header bars draw
correctly, the font is missing `▁▂▃▅▆▇`: `--glyphs legacy` swaps the ramp for
`_.,:-=+#` and leaves everything else alone.

It is TUI only. `--once` and `--json` take two `/proc` walks and have no
history to draw, so `--order spark` there leaves an empty column rather than a
misleading one, and the JSON never carries it.

## How much heft can see

heft can only bill a process it can walk, so where `/proc` hides pids — a
`hidepid` mount, a PID namespace, another user's processes on a locked-down
host — the tree is quietly smaller than the machine. The kernel publishes its
own thread count in `/proc/loadavg`, and that is a global counter rather than a
walk, so it still answers where the walk has gone blind.

When heft can account for less than 90% of those threads it says so:
`seeing 1% of 4557 threads`, in the TUI footer and on a `WARN` line under the
`--once` host line. Above that the gap is ordinary skew — threads are created
and reaped while the walk runs — and heft stays quiet. On an unrestricted host
the two agree exactly.

This is the same blind spot the GTT note above describes with a number: the
kernel reported 45.7 GiB in use while heft's Host row accounted for 18.1 GiB,
because every drm client inside a root-owned container was unreadable.

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
mount looks in every other column exactly like one that is quiet. The column
sits left of `%CORE` for that reason.

It is field 3 of `/proc/<pid>/stat`, the line heft already reads for CPU and
`THR`, so it costs no extra read.

Unlike `CPU ST` / `IO ST` / `MEM ST` it is a count rather than a percentage of
an interval, so it adds up the tree: a folder, User or Host row carries the
total of everything beneath it, and those are precisely the rows the stall
columns have to leave blank. `0` is a figure here and not a blank — every
process heft can see at all has a state, so there is nothing it can fail to
read.

A steady `0` is the ordinary reading on a healthy machine. A row that holds
above zero is waiting on something the kernel will not let it stop waiting for,
and that cell is drawn in red — reverse video where there is no colour, so it
survives `NO_COLOR`, a pipe and a monochrome terminal. The stall columns are
marked the same way once they pass 20% of an interval, which is where a cgroup
is contending for a resource rather than merely using it. Nothing else in the
table is marked: a large `%CORE` is a machine doing work, which is not trouble.

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

Heft `GET`s `/containers/json` and inspect on the first of these it finds:
`$DOCKER_HOST` when it is a unix socket, `/var/run/docker.sock`,
`/run/user/<uid>/podman/podman.sock` (rootless Podman), then
`/run/podman/podman.sock` (rootful Podman, the default on RHEL and Fedora
servers). Rootless comes first because that socket belongs to the user heft is
running as, and a rootful one may not be readable. It never POSTs, never kills,
never creates. Without a reachable socket, a
container title is `docker-<12hex>`. Stopped containers (no PID) do not appear.

## XDG

| tree | path | what |
| --- | --- | --- |
| config | `$XDG_CONFIG_HOME/heft/view.json` (default `~/.config/heft/view.json`) | saved sort, direction, filter, hidden columns, and column order (after `s`). `--user` and `--top` are deliberately never saved |
| config | `$XDG_CONFIG_HOME/heft/grouping.json` | your grouping overrides, if you write one |

v1 creates no `$XDG_STATE_HOME/heft` or `$XDG_CACHE_HOME/heft`. The only file
heft writes is that config directory; the only other state it touches is the
terminal it is drawing on — the alternate screen, and the termios settings it
restores on the way out. Never `/proc`, sysfs, or cgroup files.

### Columns

A column is drawn whole or not at all. Where the terminal cannot hold one at
its full width it is left off rather than cut short, because a clipped `20.1G`
reads as `2` and a wrong figure is the one thing heft will not print — the same
rule as the blank cells. `[` and `]` reach the columns that were left off.

The table has twenty columns and most terminals cannot hold them. `H` hides
the column you are sorting by (and moves the sort to the next visible one in
the order on screen); `u` puts the last hidden column back. `--hide` does the
same for `--once`, repeatable, and overwrites whatever `view.json` held.

`--order` sets left-to-right order, also repeatable, and also overwrites the
saved list. Columns you name come first, in that order; anything you leave out
keeps its default place after them. `name` stays first unless you include it,
so `--order pss --order rss` is "those two after the tree names" rather than a
table with no labels on the left. A label listed twice on the command line is a
usage error; in the file the extra is warned and ignored.

`s` writes both lists.

```json
{
  "sort": "pss",
  "desc": true,
  "hide_columns": ["vram", "gtt", "gfx", "compute"],
  "column_order": ["pss", "rss", "core"]
}
```

Labels are `name`, `spark`, `nproc`, `threads`, `age`, `dstate`, `core`,
`machine`, `pss`, `rss`, `swap`, `vram`, `gtt`, `gfx`, `compute`, `diskr`,
`diskw`, `cpustall`, `iostall`, `memstall`, `netns_rx`, `netns_tx`. `--sort`
and `c` take all of them except `spark`, which draws a trend rather than a
number and so has no ordering; `--hide` and `--order` take it like any other,
since hiding and moving a column are presentation. No file, or no key, shows every
column in compiled order. An unknown label in the file warns on stderr and is
ignored; on `--hide` or `--order` it is a usage error, the same split as
`--sort`. `name` cannot be hidden — a table of numbers with no labels is
unreadable — but it can be moved.

Hiding and order are presentation only: heft reads the same `/proc` files
either way, `c` skips over what it cannot show, and `--json` ignores both lists
entirely. Both flags are refused with `--json`.

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

`--once` should list your terminal and each running desktop app as its own
Applications row — idle shells in that terminal fold into it, and a shell
that launched an app (claude, …) bills to that app. An Electron app appears
once, not once per helper process. User Services should hold your compositor
and any user `*.service` units. Containers should sum a compose project under
one row and bill `containerd-shim` / `conmon` / `runc` to their container,
never to `dockerd`.

Contributor gates live in [CONTRIBUTING.md](CONTRIBUTING.md).

## Uninstall

From the AUR, `pacman -Rns heft-bin` (or `heft`, or `heft-git`) takes the
binary, completions and man page with it; `~/.config/heft` is yours and stays.
Installed by hand:

```sh
rm -f ~/.local/bin/heft
rm -rf ~/.config/heft
rm -f ~/.local/share/bash-completion/completions/heft
rm -f ~/.local/share/zsh/site-functions/_heft
rm -f ~/.config/fish/completions/heft.fish
rm -f ~/.local/share/man/man1/heft.1
```
