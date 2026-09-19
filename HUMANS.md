# Running heft

## Install

Grab the static build. It has no libc to match, so it runs on any distro:

```sh
curl -fsSLO https://github.com/Rethunk-Tech/heft/releases/latest/download/heft-x86_64-unknown-linux-musl
install -Dm755 heft-x86_64-unknown-linux-musl ~/.local/bin/heft
```

Every release carries four binaries (`x86_64` and `aarch64`, each a `musl`
and a `gnu` build) with a `.sha256` beside each
(`sha256sum -c heft-<target>.sha256`). Completions (bash, zsh, fish) and a
man page are `heft-completions-man.tar.gz` on the same release:

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

To prove a release binary is the one this repository built, verify its signed
provenance statement:

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
and `conflict` with `heft`. `heft-bin` has no dependencies; pick it unless you
want the compile.

Or build it:

```sh
git clone https://github.com/Rethunk-Tech/heft.git && cd heft
cargo build --release
install -Dm755 target/release/heft ~/.local/bin/heft
```

There is no crates.io package, since the static binary needs no toolchain;
`cargo install --git https://github.com/Rethunk-Tech/heft` does the same as
the clone above. Installing completions from a local build:
[CONTRIBUTING.md](CONTRIBUTING.md#setup).

Linux only. heft reads `/proc`, `/sys/class/drm`, and (when reachable) the
Docker or Podman API over a unix socket. It never uses `sudo`.

## Run

```sh
heft                      # fullscreen TUI, 1 s catch-all / 5 s PSS
heft --once               # one table on stdout
heft --json               # one JSON tree on stdout
heft --interval 0.5       # catch-all period (TUI tick and --once/--json gap)
heft --pss-interval 5     # TUI and --follow: how often to read smaps_rollup (default 5s)
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
heft --trend chars        # TREND as block characters, never an image
heft --trend kitty        # draw TREND as an image (kitty, ghostty)
heft --trend sixel        # the same picture as sixel (xterm, foot, wezterm, …)
heft --proc-root /mnt/tree   # read /proc and /sys under here instead of /
heft --explain 1234       # where this pid landed, and the rule each stage matched
heft --check-rules        # run the examples in every rules.d file
heft --fixture > heft-fixture.json  # what grouping reads, for a bug report
```

The TUI needs a terminal: with stdout redirected it exits non-zero rather than
falling back to `--once`.

`q`, `Esc` and `Ctrl-C` quit; every other `Ctrl-` or `Alt-` combination is
ignored. On `SIGTERM`, a closed terminal emulator, a logout or a panic, heft
restores the terminal and exits `128 +` the signal.

`--interval` (default 1s) is the catch-all: `/proc` walk, RSS, io, GPU,
grouping, and CPU/disk/GPU rates. The TUI sleeps `--interval` minus sample
time; a PSS pass may stretch that tick. `--pss-interval` (default 5s, at least
`--interval`) sets how often the TUI and `--follow` read `smaps_rollup`;
between reads heft reuses each PID's last PSS, and a new PID shows a blank PSS
until the next read. A PSS tick also skips a process whose RSS has moved less
than 1% (or 1 MiB) since its last read, for up to six PSS periods per 512 MiB
of RSS, and never more than 60. A value below the 0.05s floor is a usage error.

A one-shot `--once` or `--json` takes two `/proc` walks `--interval` apart so
rates exist, and always reads PSS.

The TUI and `--follow` keep each process's `stat`, `statm` and `io` open
between ticks, which raises heft's soft open-file limit toward the hard one;
expect around three open files per process in `/proc/<heft>/fd`.

`--follow` keeps sampling and needs `--once` or `--json`. `--json --follow`
writes one compact document per line (NDJSON); `--once --follow` reprints the
table with its header each interval, separated by a blank line. The first
sample always reads PSS. Closing the reader ends any mode quietly, so
`heft --json --follow | head -5` and `heft --explain 1 | head -1` exit 0. Every JSON document carries
`host.sampled_at`, the unix second the sample was taken.

`--sort` takes a column label other than `spark` ([Columns](#columns)); an
unknown label is a usage error. `--desc` and `--asc` set the direction the `d`
key toggles.

`--filter` is the `/` key. The pattern is a regex, case-insensitive unless
`(?-i)` turns that off; Unicode classes (`\p{Greek}`) are not supported. It
matches a row's name and the argv of every process under it, expanded or not,
one process per line, so `^` and `$` anchor to one process's argv. Matching
rows are kept with their parents, and a parent keeps its full total.

`--once` rows stop at identities and their containers while the TUI also has
instance and process rows, so `/` can match rows `--filter` never sees. A
pattern that does not compile is a usage error on the command line, a warning from
`view.json`, and a `?` in the TUI footer while the last compiling pattern stays
live.

`--top N` keeps the N heaviest rows under each parent at every depth, applied
after `--filter`. Cutting a row cuts its subtree. Host, User and folder rows
are never trimmed, and a surviving parent keeps its full total.

`--user` takes a login name or a uid, repeatable. Other User nodes drop; System
and Host-level Containers stay. A bare number is accepted as a uid even with no
`/etc/passwd` entry; an unknown name is a usage error. Unlike `--filter`, the
Host row totals only what survived. It works with `--json` and is never saved.

Flags beat a saved `view.json`. `--once` starts from the saved view; `--json`
never reads it. `--filter`, `--top`, `--hide` and `--order` are refused with
`--json` (slice the tree with `jq`), and `--trend` with `--once` and `--json`.

`--glyphs auto` (the default) uses block characters only when `LC_ALL`,
`LC_CTYPE` or `LANG` names a UTF-8 charmap, and one-column ASCII substitutes
otherwise. `unicode` and `ascii` force either. `legacy` draws everything in
Unicode except the TREND ramp, which becomes `_.,:-=+#`: use it when a font
has `█▓▒░` and `▼►` but TREND shows boxes for `▁▂▃▅▆▇`. `auto` never picks it.

`NO_COLOR` set to any non-empty value, `0` and `false` included, draws the TUI
without hue. Bar segments still differ by fill, and `?` shows a swatch of
each. `--once` and `--json` never emit colour. `--once`, `--explain` and
`--check-rules` print a control character in a name or argv as a visible
escape such as `\u{1b}`; `--json` and `--fixture` carry the raw string,
JSON-escaped.

`--proc-root` reads `/proc` and `/sys` under another directory, such as another
mount namespace's procfs or a tree captured off another machine; a directory
with no `proc` in it is a usage error. The container socket, `/etc/passwd` and
`/etc/heft/rules.d` stay the host's, and the header still describes the host,
because the kernel does not virtualise `meminfo` and `stat` per namespace.

## TUI keys

| key | action |
| --- | --- |
| `q` / `Esc` / `Ctrl-C` | quit |
| `↑` `↓` / `j` `k` | move the cursor |
| `PgUp` `PgDn` / `Home` `End` | page or jump the cursor |
| Enter / Space | collapse or expand |
| `←` `→` / `h` `l` | scroll columns when the terminal is narrower than the table |
| `Shift-←` `Shift-→` / `[` `]` | previous or next sort column (default PSS descending); `[` `]` for terminals that keep Shift-arrows (Konsole's tab switching, the Linux console, rxvt) |
| `/` | filter by regex on the name and argv (Enter applies, Esc cancels) |
| `n` / `N` | next or previous filter match (wraps; skips Host, users, and folder headers) |
| `g` | go to a pid (`goto>`; Enter applies, Esc cancels) |
| `E` | expand every expandable row |
| `c` | collapse to the default expand set |
| `d` | reverse the sort direction |
| `H` | hide the current sort column (`name` is refused) |
| `u` | unhide the last hidden column |
| `i` | detail for the row under the cursor (`i` or Esc closes) |
| `p` | pause the table; the footer says how long it has been frozen |
| `s` | save sort, filter, hidden columns, and column order to `$XDG_CONFIG_HOME/heft/view.json` |
| `?` / `F1` | toggle the key help overlay |

Sorting applies to every level of the tree, and the sort column's header is
drawn reversed. Sort, filter, hidden columns and column order last for the
session until `s` saves them; heft loads that file at start.

Default expand: Host, your user, Applications, and that user's Containers. An
empty folder is not expandable until a row appears. The cursor stays on its
row through new samples, resorts, filtering, `--top` and expand/collapse; if
the row is gone, the nearest parent still on screen is selected.

`i` opens a detail pane listing every column for the row, hidden and
off-screen ones included (`-` is a blank). On a single process it adds pid,
parent pid, state, owning uid, `exe` path, cgroup line and command line (cut
at 240 characters), read from `/proc` on the keypress; a field heft cannot
read is blank. It also shows where the row landed and the placement key; for a
process, the same stages `--explain` prints, without the rule-file recipe.

`p` freezes the table while sampling continues underneath, so unpausing shows
the current machine. Sort, filter, expand and `i` still work on the held tree,
and the footer reads `PAUSED 8s`.

## What the tree means

- **Host** is the machine. The header has a CPU row (usr/sys/wait from
  `/proc/stat`), a MEMORY row, and a SWAP row when `SwapTotal` is non-zero,
  all drawn to one width. Bars carry no legend: `?` shows a swatch per
  segment, and each segment has its own colour and a fill (`█`, `▓`, `▒`)
  unlike its neighbours'.
- The MEM bar's `used` is what the kernel cannot hand back. Inside it: `vram`
  (unified memory only) and `gtt`; `zram`, the RAM compressed swap occupies,
  which no process's PSS holds; `shm` (tmpfs and shared memory); `kernel`
  (unreclaimable slab, page tables, kernel stacks); `anon` (`AnonPages`); and
  `other`. Reclaimable `cache`, `slab` and `buf` follow outside `used`. Where
  memory is unified the row is one bar of width MemTotal; with a discrete card
  it splits into MEM and a VRAM tank against the card's total. GTT is pinned
  system RAM and stays in MEM either way. Swap is its own row, since swapped
  pages are not in RAM.
- VRAM and GTT come from the kernel's `mem_info_vram_used` and
  `mem_info_gtt_used` where the driver has them (amdgpu), else from the drm
  clients heft can see. The Host row sums only visible PIDs, and an
  unprivileged reader cannot open `/proc/<pid>/fdinfo` for a process it does
  not own, so drm clients in a root-owned container are invisible. Reading them would need `docker exec`, a POST heft
  never sends.
- **User** is a unix uid, shown by login name. Terminals and the compositor
  live under that user. An idle interactive shell folds into its terminal; a
  shell that launched an app bills to the app.
- Folder headings (Applications, User Services, Containers, System) carry the
  count of identities under them, including zero, not `N` (processes).
- **Applications** vs **User Services**: a user-instance `*.service` whose name
  does not start with `app-` is a user service (`syncthing.service`,
  `org.gnome.Shell@user.service`). Known compositors and session plumbing sit
  in User Services even under an `app-` or `dbus:` unit. Everything else under
  the user is an application. Merge identities: [AGENTS.md](AGENTS.md). Plasma
  session processes merge as `plasmashell`, kwin and its helpers as `kwin`. A
  Trinity (TDE) session (tdeinit, twin, kicker, kdesktop, artsd, its tdeio
  slaves and tray helpers) merges as `tdeinit`, while apps tdeinit launches
  keep their own rows. A sway session lands in the right buckets but merges
  more coarsely under User Services.
- **Containers** under a user are workloads heft can attribute: the owner of
  the container's workdir label, else the owner of its first bind mount under a
  user's paths. Unattributed running containers sit on Host → Containers.
- **System** is kernel threads and leftover `system.slice` (including
  `dockerd` / `containerd`). Container scopes never go here.
- A **virtual machine or nspawn container** (anything in `machine.slice`) is a
  Host → Containers row named after the machine: libvirt's
  `machine-qemu\x2d3\x2dfedora.scope` reads `fedora`. Rootful Podman shares
  that slice but is a `libpod-` scope, so it keeps its real name and owner.

The GPU columns (VRAM, GTT, GFX, CMP) read DRM fdinfo. A client counts when
its driver is `amdgpu`, `i915` or `xe`, or when its fdinfo publishes both
`drm-client-id` and a `drm-resident-*` key, whatever the driver. Only the
names those three drivers use are read: memory in a `vram`, `vram0`, `vram1`,
`local0`, `gtt` or `system0` region, and time on a `gfx`, `render` or
`compute` engine, or xe's `rcs` and `ccs` cycles. A region or engine under any
other name is not guessed at, so on another driver those columns stay blank
unless it uses the same names.

heft looks for a process's GPU file descriptors on PSS reads, so a GPU a
running process opens can take up to `--pss-interval` to show, and on the
tick a process's set of GPU descriptors changes its GFX and CMP are blank.

Processes nest at most 48 deep: everything below a process at depth 47 is
listed flat beneath it, in pid order, so nothing is dropped and every total is
unchanged.

Other uids appear as extra User nodes when `/proc` lists them. A metric heft
cannot read (`smaps_rollup`, `io`, fdinfo, `exe`) or that does not exist for a
row renders as a blank cell. **A blank is not a zero**: no figure exists, and
blank rows sort last in either direction.

## TREND

`TREND` (`spark`) draws the recent history of the sort column, one sample per
cell, as a line joined sample to sample. It is at least nine cells: spare width
goes to NAME until the longest name fits, then to TREND up to 30 cells, then
back to NAME. Changing the sort column clears it.

Every row in a frame shares one scale. For percentages (`%CORE`, `%MACH`,
`GFX`, `CMP` and the stall columns) it is 100, and a row above it pins to the
top. Otherwise it is the heaviest entry row on screen. Host, User and folder
rows are sums: they draw no trend and are left out of the scale, the same
entry test `--top` uses. A row flat at zero draws a flat line; a row that has
just appeared is blank. If TREND alone shows boxes, use `--glyphs legacy`.

`--trend` chooses characters or an image. `auto` (the default) asks the
terminal before the first frame, with a kitty graphics query and a
device-attributes request, and uses the image protocol it reports: kitty when
local, sixel over ssh. A terminal that answers neither delays the first frame
by 400ms and gets characters. `--trend kitty`, `sixel` and `chars` force one
and ask nothing. A terminal that does not report its cell size in pixels gets
characters. TREND is TUI only: `--once` has no TREND column, and the JSON
never carries it.

The kitty protocol (kitty, ghostty) sends pixels through shared memory
locally, costing tens of bytes a sample. Over ssh they go inline as base64,
far more per sample than sixel and proportionally more for a wider TREND. It is sent once per sample, not per
frame. On a slow link without sixel, use `--trend chars`.

`--trend sixel` works in xterm (on by default since patch #359), foot,
wezterm, konsole 22.04 and later, iTerm2, and Windows Terminal 1.22 and later.
kitty, ghostty and alacritty have no sixel, and GNOME Terminal's sixel setting
does nothing because VTE strips sixel from every stable release. Sixel is
repainted on every redraw (a sample or a keypress), but run-length encoding
keeps it small. Over ssh it is the cheaper choice.

## How much heft can see

Where `/proc` hides pids (a `hidepid` mount, a PID namespace, another user's
processes on a locked-down host) the tree is smaller than the machine. heft
compares its summed threads, taken before `--user` removes anyone, with the
kernel's global count in `/proc/loadavg`;
below 90% it says so, `seeing 1% of 4557 threads`, in the TUI footer and on a
`WARN` line under the `--once` host line. The invisible drm clients under
[What the tree means](#what-the-tree-means) are the same blind spot.

## SWAP

`SWAP` is `SwapPss` from `/proc/<pid>/smaps_rollup`, read with PSS on the same
cadence. `SwapPss` rather than `Swap`, so a swapped page shared by four
processes is counted once in a summed tree. Blank when `SwapTotal` is 0; `0`
means that process has nothing paged out.

## THR / AGE

`THR` is the thread count (`num_threads` from `/proc/<pid>/stat`). `THR` sums
like `N`.

`AGE` is time since the process started, in the largest unit that fits: `45s`,
`12m`, `3h`, `9d`. On a row covering several processes it is the oldest of
them, never a sum.

## CPU ST / IO ST / MEM ST

Hidden by default: `u` brings the last hidden column back, or leave them out of
a `hide_columns` list in `view.json`. Pressure is read only while something
shows it: a stall column, a stall sort, the `i` pane, or `--json`. Turned on
mid-run, the figures are blank for the first interval.

Each figure is the percentage of the last interval during which at least one
task in the row's cgroup was stalled on that resource, from `cpu.pressure`,
`io.pressure` and `memory.pressure`: the kernel's `some` number, not `full`. A
stalled row can look idle in `%CORE`. A row that is one non-root cgroup shows
that cgroup's rate. A row whose processes span several cgroups shows the max of
those members' `some` rates, per resource: sum can exceed 100% (overlapping
intervals) and an average hides a fully-stalled member. Blank:

- folder, User and Host rows (`user-1000.slice` is not the User row, and
  `system.slice` is not the System row);
- a row in the root cgroup, whose pressure is the machine's;
- a process row that shares its cgroup with other processes.

A figure at or over 20% is drawn red, or reverse video without colour. The
`psi` figures on the `--once` and `--json` host line are the kernel's
machine-wide 10-second averages, in cpu/io/memory order. A kernel built
without `CONFIG_PSI`, or booted `psi=0`, gets no host figures and no columns.

## NETNS RX / NETNS TX

Network bytes are counted per network namespace, so only a container row
carries them. Every other row is blank: Linux has no per-process byte counter
heft can read without CAP_NET_RAW, CAP_BPF or ptrace. They are the last
columns; `→` scrolls to them on a narrow terminal.

- They sum the container's interfaces except `lo`.
- A `--network=host` container is blank, since it shares the machine's
  namespace.
- A compose or Supabase project row sums its containers.
- A restart resets the counters, and heft drops that one interval.
- Without a reachable Docker or Podman socket every container is blank.

## Docker / Podman

Heft `GET`s `/containers/json` and inspect on the first of these it finds:
`$DOCKER_HOST` when it is a unix socket, `/var/run/docker.sock`,
`/run/user/<uid>/podman/podman.sock` (rootless Podman), then
`/run/podman/podman.sock` (rootful Podman). It never POSTs, kills or creates.
Without a reachable socket a container title is `docker-<12hex>`. Each sample
gives the daemon 2 s in total for the list and every inspect; a container not
inspected in time keeps that title until a later sample reaches it. The list is
asked for again only when a container starts or stops, or every 30 s, so a
`docker rename` shows within 30 s. Stopped containers (no PID) do not appear.

## XDG

| tree | path | what |
| --- | --- | --- |
| config | `$XDG_CONFIG_HOME/heft/view.json` (default `~/.config/heft/view.json`) | saved sort, direction, filter, hidden columns, and column order (after `s`). `--user` and `--top` are never saved |
| config | `$XDG_CONFIG_HOME/heft/rules.d/*.json` | your grouping rules, if you write any ([Rules](#rules)) |
| config | `/etc/heft/rules.d/*.json` | the administrator's grouping rules, read after yours |

heft creates no `$XDG_STATE_HOME/heft` or `$XDG_CACHE_HOME/heft`. The only file
it writes is `view.json`, besides the shared-memory objects `--trend kitty`
creates under `/dev/shm` and removes; the only other state it touches is its
terminal (the alternate screen, and termios restored on exit). Never `/proc`, sysfs, or
cgroup files.

### Columns

A column is drawn whole or not at all: one that does not fit is left off
rather than clipped. `←` and `→` reach the rest while NAME stays put.

There are twenty-one columns. `H` hides the sort column and moves the sort to
the next visible one; `u` restores the last hidden. `name` cannot be hidden but
can be moved. `--hide` (repeatable) adds to what `view.json` hides. `--order`
(repeatable) overwrites the saved order: named columns come first in that
order, the rest keep their default order after them, and `name` stays first
unless listed. A label listed twice on the command line is a usage error; in
the file the extra is warned and ignored. `s` writes both lists.

```jsonc
{
  // `//` and `/* */` comments are allowed here and in rules.d files
  "sort": "pss",
  "desc": true,
  "hide_columns": ["vram", "gtt", "gfx", "compute"],
  "column_order": ["pss", "rss", "core"]
}
```

`s` writes this file with a header explaining every key and listing the
column labels. A save rewrites the whole file, so comments you add do not
survive one; rules.d files, which heft never writes, keep theirs.

Labels are `name`, `spark`, `nproc`, `threads`, `age`, `core`,
`machine`, `pss`, `rss`, `swap`, `vram`, `gtt`, `gfx`, `compute`, `diskr`,
`diskw`, `cpustall`, `iostall`, `memstall`, `netns_rx`, `netns_tx`. `--sort`
and `Shift-←` `Shift-→` (`[` `]`) take all of them except `spark`, `--hide` all of
them except `name`, and `--order` all of them. With no file or no `hide_columns` key, `rss`,
`cpustall`, `iostall` and `memstall` are hidden and the rest show in compiled order. An
unknown label in the file warns on stderr and is ignored; on `--hide` or
`--order` it is a usage error. Hiding and order are presentation only: heft
reads the same `/proc` files either way.

## Rules

Grouping is decided by rule files. The built-in set is the repository's
`rules.d/`, compiled into the binary. heft also reads
`$XDG_CONFIG_HOME/heft/rules.d/` and then `/etc/heft/rules.d/`, both ahead of
the built-ins. It never writes or creates either directory, and with neither
present it groups exactly as it ships.

`HEFT_RULES_PATH` replaces those two directories with its own colon-separated
list, earlier entries winning. Set but empty, only the built-ins load.

Each file is one stage:

| stage | decides | when several rules match |
| --- | --- | --- |
| `unit` | whether a systemd unit lies about the app in it (`lying`) or is a user service (`service`) | every one adds its flags |
| `class` | `launcher`, `generic`, `shell`, `terminal`, `compositor`, `worker`, `noise`, `crash_helper`, `no_absorb`, `anonymous_script`, `container_runtime` | every one adds its classes |
| `session` | an identity and folder for desktop session plumbing | the first wins |
| `app` | an identity for a helper shipped inside an app's install tree | the first wins |
| `placement` | `fold_to` another identity, a `folder` pin, or a container's `owner_uid` | the first wins |

Your directory beats `/etc`, and both beat every built-in, whatever the files
are called. Within one directory files are read in byte order, so keep a
two-digit prefix: `10-x.json` sorts before `9-x.json`. Only regular files, or
symlinks to them, whose names end `.json` are read. `//` and `/* */` comments
are allowed.

```jsonc
{
  "stage": "placement",
  "rules": [
    // A fold and a pin on the same identity: list the fold first, or the
    // pin matches and the fold is never reached.
    { "id": "fold-worker", "match": { "identity": "mydaemon-worker" }, "fold_to": "mydaemon" },
    { "id": "pin-daemon", "match": { "identity": "mydaemon" }, "folder": "user_services" },
    { "id": "own-runner", "match": { "container": "scratch-runner" }, "owner_uid": 1000 }
  ],
  "examples": [
    { "identity": "mydaemon", "expect": { "rule": "pin-daemon", "folder": "user_services" } },
    { "identity": "htop", "expect": null }
  ]
}
```

A `match` holds one test. `name`, `name_prefix`, `name_suffix` and
`name_contains` look at both `comm` and the display name; `exe_prefix`,
`exe_suffix` and `exe_contains` at the executable path; `arg_prefix` and
`arg_contains` at each argument; `cgroup_contains` at the cgroup; `unit`,
`unit_prefix`, `unit_suffix` and `unit_contains` at the systemd user unit.
`identity` and `container` are for `placement` rules, and `script` only for a
`class` rule whose classes are `["anonymous_script"]`. A list means any of
them, `all`, `any` and `not` combine tests, and every comparison ignores ASCII
case. A `not` over something the process does not have is a match:
`{"not": {"exe_prefix": "/usr"}}` matches a process whose `exe` heft cannot
read.

Placement never moves a container or a kernel thread: those rows ignore it.

A file only ever adds rules. To turn a built-in off, name it:
`"disable": ["40-trinity.json"]` drops a whole file wherever it is loaded
from, and `"disable": ["40-trinity.json:trinity-session"]` drops one rule. A
`disable` that names nothing warns once, which is how a built-in renamed in a
new release shows up. Built-in file names and rule ids are part of heft's
interface, and a rename is called out in the changelog. Do not name your own
file after a built-in: its `disable` of that name applies to your file too.

A file that does not parse, or breaks one of the rules above (a duplicate id,
an empty pattern, an output its stage does not take), is skipped whole with one
warning naming it, and the rest still load.

`examples` are what the file says its rules do: a `--fixture` process row, or
`{"unit": ...}`, `{"identity": ...}` or `{"container": ...}`, with an `expect`
that is `null` for no match or the keys to compare. `heft --check-rules` runs
them all:

```
$ heft --check-rules
90-mine.json:pin-daemon#0 (/home/me/.config/heft/rules.d): expected folder user_services, got applications
30-gnome.json:gsd#9 (built-in): overridden by 90-mine.json:my-gsd (/home/me/.config/heft/rules.d)
40-trinity.json#3 (built-in): disabled by 40-trinity.json (90-mine.json, /home/me/.config/heft/rules.d)
128 examples in 13 files: 1 failed, 1 overridden, 14 disabled
```

It exits 1 on a failed example or a file that did not load. A built-in example
your rules decide differently is reported as `overridden by` or `disabled by`
rather than failed.

Keys are the identities the tree shows you, not pids or comms. `heft --explain
<PID>` tells you what a process resolved to, and what each stage made of it,
and exits 1 when that pid is not visible:

```
$ heft --explain 156859
pid 156859
  EXE       /tmp/.mount_cursorgHNPBN/usr/share/cursor/cursor
  CGROUP    0::/user.slice/.../app.slice/flatpak-session-helper.service
  ...
  placed    Host → damonblais (1000) → Applications
  identity  cursor
  instance  pgid:3743373 (3 processes)
  rules     unit       built-in 05-units.json:lying -> lying
                       built-in 05-units.json:service -> service
            class      no match
            session    no match
            app        no match
            placement  none

  "cursor" is the placement key for this row.
```

A wrong key is silent: an identity that matches nothing is never consulted.
The stage lines say what each stage makes of the process on its own; `placed`
is the tree's verdict, which also depends on the process's parents.

If the built-in grouping is wrong, attach `heft --fixture > heft-fixture.json`
to a bug report. It is every process's exe, cgroup, parent and command line,
in the shape heft's grouping tests load, so your machine becomes the test for
the fix. Nothing is sent anywhere. Your home directory is written as `~`, but
command lines are kept whole, so read the file before you post it.

## Verify

```sh
heft --once
```

`--once` should list your terminal and each running desktop app as its own
Applications row: idle shells in that terminal fold into it, and a shell that
launched an app (claude, …) bills to that app. An Electron app appears once,
not once per helper process. User Services should hold your compositor and any
user `*.service` units. Containers should sum a compose project under one row
and bill `containerd-shim` / `conmon` / `runc` to their container, never to
`dockerd`.

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
