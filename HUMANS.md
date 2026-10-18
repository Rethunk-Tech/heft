# Running heft

## Install

```sh
brew install rethunk-tech/tap/heft
```

Grab the static build. It has no libc to match, so it runs on any distro:

```sh
curl -fsSLO https://github.com/Rethunk-Tech/heft/releases/latest/download/heft-x86_64-unknown-linux-musl
install -Dm755 heft-x86_64-unknown-linux-musl ~/.local/bin/heft
```

Every release carries four binaries (`x86_64` and `aarch64`, each `musl` and
`gnu`) with a `.sha256` beside each (`sha256sum -c heft-<target>.sha256`).
The musl builds are also `heft-linux-amd64` and `heft-linux-arm64`. `SHA256SUMS`
covers every asset. Completions (bash, zsh, fish) and a man page are
`heft-completions-man.tar.gz`:

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

```sh
gh attestation verify heft-x86_64-unknown-linux-musl --repo Rethunk-Tech/heft
```

On Arch (`provide`/`conflict` `heft`; all three install completions and the
man page). `heft-bin` has no dependencies:

```sh
paru -S heft-bin   # release musl binary
paru -S heft       # built from the release tag
paru -S heft-git   # built from main
```

Or build it. There is no crates.io package; `cargo install --git
https://github.com/Rethunk-Tech/heft` is the same as the clone. Completions
from a local build: [CONTRIBUTING.md](CONTRIBUTING.md#setup).

```sh
git clone https://github.com/Rethunk-Tech/heft.git && cd heft
cargo build --release
install -Dm755 target/release/heft ~/.local/bin/heft
```

Linux only. Reads `/proc`, `/sys/class/drm`, and (when reachable) Docker or
Podman over a unix socket. Never `sudo`.

## Run

`heft --help` lists every flag. Common shapes:

```sh
heft                      # TUI, 1 s catch-all / 5 s PSS
heft --once               # one table on stdout
heft --json               # one JSON tree on stdout
heft --json --follow      # NDJSON, one document per interval
heft --explain 1234       # placement for this pid
heft --check-rules        # examples in every rules.d file
heft --fixture > heft-fixture.json
```

The TUI needs a terminal: redirected stdout is a usage error (exit 129) rather
than falling back to `--once`. `q`, `Esc` and `Ctrl-C` quit; other `Ctrl-` /
`Alt-` chords are ignored. On `SIGTERM`, a closed emulator, a logout or a
panic, heft restores the terminal and exits `128 +` the signal.

`--interval` (default 1s) is the catch-all walk. `--pss-interval` (default 5s,
at least `--interval`) is how often the TUI and `--follow` read
`smaps_rollup`; between reads the last PSS is reused, and a new PID is blank
until the next read. A PSS tick also skips a process whose RSS has moved less
than 1% (or 1 MiB) since its last read, for up to six PSS periods per 512 MiB
of RSS, never more than 60. Below the 0.05s floor is a usage error.

`--once` / `--json` take two walks `--interval` apart so rates exist, and
always read PSS. `--explain` is one walk (grouping needs no rates); the flag
is still accepted. The TUI and `--follow` keep each process's `stat`, `statm`
and `io` open; expect about three open files per process.

`--follow` needs `--once` or `--json`. Closing the reader ends any mode
quietly (`| head` exits 0). Every JSON document carries `"schema": 1` first,
then `interval_secs` and `host.sampled_at`; `--json --follow` also carries
`pss_interval_secs`.

Flags beat `view.json`. `--once` starts from the saved view; `--json` never
reads it. `--top`, `--hide` and `--order` are refused with `--json`; `--trend`
with `--once` and `--json`. `--filter` is allowed with `--json`: matching
processes and their ancestors stay; siblings that do not match drop; ancestors
keep the totals they were built with. `--user` and `--top` are never saved.

`--filter` / `/` is a regex, case-insensitive unless `(?-i)` (no Unicode
classes). It matches name, each process's argv, and cwd, each on its own line.
`--once` rows stop at identities; the TUI also has instance and process rows,
so `/` can match a row `--once` never prints. A pattern that does not compile
is a usage error on the CLI, a warning from `view.json`, and a `?` in the TUI
footer while the last compiling pattern stays live.

`--glyphs auto` uses Unicode only when `LC_ALL` / `LC_CTYPE` / `LANG` names a
UTF-8 charmap. `legacy` keeps block bars but an ASCII TREND ramp (`_.,:-=+#`)
when the font has `█▓▒░` but TREND shows boxes. `auto` never picks `legacy`.
`NO_COLOR` set to any non-empty value, including `0` and `false`, draws
without hue. `--once` / `--json` never emit colour. `--once`, `--explain` and
`--check-rules` print control characters as `\u{1b}`; `--json` and `--fixture`
keep the raw string.

`--proc-root` reads `/proc` and `/sys` under that prefix (header included).
The container socket, `/etc/passwd` and `/etc/heft/rules.d` stay the host's.

## TUI keys

| key | action |
| --- | --- |
| `q` / `Esc` / `Ctrl-C` | quit |
| `↑` `↓` / `j` `k` | move the cursor |
| `PgUp` `PgDn` / `Home` `End` | page or jump |
| Enter / Space | collapse or expand |
| `←` `→` / `h` `l` | scroll columns |
| `Shift-←` `Shift-→` / `[` `]` | previous or next sort column (`[` `]` when the terminal keeps Shift-arrows) |
| `/` | filter (Enter applies, Esc cancels) |
| `n` / `N` | next or previous match (skips Host, users, folder headers) |
| `g` | go to a pid |
| `E` | expand every expandable row |
| `c` | collapse to the default expand set |
| `d` | reverse sort direction |
| `H` | hide the current sort column (`name` is refused) |
| `u` | unhide the last hidden column |
| `i` | detail for the row (`i` or Esc closes) |
| `p` | pause the table; sampling continues underneath |
| `s` | save sort, direction, filter, hidden columns, and column order to `$XDG_CONFIG_HOME/heft/view.json` |
| `?` / `F1` | key help overlay |

Default expand: Host, your user, Applications, and that user's Containers. The
cursor stays on its row through samples, resorts, filter and expand; if the
row is gone, the nearest parent still on screen is selected.

`i` lists every column for the row. On a single process it adds pid, parent,
state, uid, `exe`, cgroup and argv (cut at 240 characters), read on the
keypress, plus the same placement stages `--explain` prints.

## What the tree means

Formulas and bar layout: [AGENTS.md](AGENTS.md#sampler).

- **Host** is the machine. Header: CPU (usr/sys/wait), MEMORY, SWAP when
  `SwapTotal` is non-zero. `?` shows a swatch per segment. `used` is what the
  kernel cannot hand back; reclaimable cache sits beyond it. Unified memory:
  one bar of width MemTotal. Discrete card: MEM plus a VRAM tank; GTT stays
  in MEM (pinned system RAM). Swap is its own row.
- Host VRAM/GTT come from the driver totals where present, else from visible
  drm clients. An unprivileged reader cannot open another uid's fdinfo, so
  drm clients in a root-owned container are invisible (`docker exec` would be
  a POST heft never sends).
- **User** is a unix uid. An idle interactive shell folds into its terminal; a
  shell that launched an app bills to the app.
- Folder headings carry identity count, including zero, not `N` (processes).
- **Applications** vs **User Services**: a user-instance `*.service` whose
  name does not start with `app-` is a user service. Known compositors and
  session plumbing sit in User Services even under an `app-` or `dbus:` unit.
  Merge identities: [AGENTS.md](AGENTS.md). Plasma → `plasmashell` / `kwin`;
  Trinity → `tdeinit` (apps tdeinit launches keep their rows); Sway → `sway`,
  `swaybar`, `swaybg`.
- **Containers** under a user: owner of the workdir label, else the first bind
  mount under a user's paths. Unattributed running containers sit on Host →
  Containers. `machine.slice` is a Host → Containers row named after the
  machine; rootful Podman is a `libpod-` scope and keeps its real name.
- **System** is kernel threads and leftover `system.slice` (`dockerd` /
  `containerd`). Container scopes never go here.

GPU columns count a client when the driver is `amdgpu`, `i915` or `xe`, or
when fdinfo publishes both `drm-client-id` and a `drm-resident-*` key. Only
those three drivers' region and engine names are read; another driver stays
blank unless it uses the same names. GPU fds are scanned on PSS reads, so a
newly opened GPU can take `--pss-interval` to show.

Processes nest at most 48 deep: everything below depth 47 is listed flat
beneath that process. **A blank is not a zero**: no figure exists, and blank
rows sort last in either direction.

## TREND

Recent history of the sort column, at least nine cells, capped at 30. One
scale per frame: 100 for percentages (values above pin to the top), else the
heaviest entry row on screen. Host, User and folder rows draw no trend.
Changing the sort column clears it.

`--trend auto` asks the terminal before the first frame (kitty graphics query
and device-attributes). Kitty when local; sixel when `SSH_CONNECTION` or
`SSH_TTY` is set. No answer: 400ms delay, then characters. A terminal that
does not report cell size in pixels gets characters. TUI only.

Kitty sends pixels through shm locally; over SSH they go inline as base64
(costlier than sixel). Sixel is the cheaper SSH choice (xterm, foot, wezterm,
konsole 22.04+, iTerm2, Windows Terminal 1.22+). kitty, ghostty and alacritty
have no sixel; GNOME Terminal's setting does nothing (VTE strips it). On a
slow link without sixel, `--trend chars`.

## How much heft can see

Where `/proc` hides pids the tree is smaller than the machine. Below 90% of
`/proc/loadavg`'s thread count, heft says so in the TUI footer, on a `WARN`
line under `--once`, and as a top-level `warnings` array on JSON (omitted at
or above 90%).

## SWAP / THR / FD / AGE

`SWAP` is `SwapPss` from the PSS rollup, so a swapped page shared by four
processes is counted once. Blank when `SwapTotal` is 0; `0` means nothing
paged out.

`THR` is `num_threads`; it sums like `N`. `FD` is the last `/proc/<pid>/fd`
scan, held between scans the way PSS is; blank on kernel threads until the
first scan. `AGE` is the oldest start time on a multi-process row, never a
sum: `45s`, `12m`, `3h`, `9d`.

## CPU ST / IO ST / MEM ST

Hidden by default. Pressure is read only while something shows it (column,
sort, `i`, or `--json`). Each figure is the last interval's `some` stall
percentage (`cpu`/`io`/`memory`.pressure), not `full`. Several cgroups on a
row: max of members' `some`. Blank on folder, User and Host rows; on the root
cgroup (that pressure is the machine's); on a process that shares its cgroup.
≥20% is red, or reverse video without colour. Host-line `psi` is the kernel's
10-second averages. No `CONFIG_PSI` / `psi=0`: no host figures and no columns.

## NETNS RX / NETNS TX

Per network namespace, so only container rows. Linux has no per-process byte
counter heft can read without CAP_NET_RAW, CAP_BPF or ptrace. Sums non-`lo`
interfaces. `--network=host` is blank. A compose/Supabase project sums its
containers. A restart drops that interval. No Docker/Podman socket: blank.

## Docker / Podman

GET `/containers/json` and inspect on the first of: `$DOCKER_HOST` when it is
a unix socket, `/var/run/docker.sock`, rootless then rootful podman sockets.
Never POST. Unreachable socket: title `docker-<12hex>`. 2 s budget per sample
for list plus inspects; the list is refreshed on start/stop or every 30 s
(`docker rename` shows within 30 s). Stopped containers do not appear.

## XDG

| tree | path | what |
| --- | --- | --- |
| config | `$XDG_CONFIG_HOME/heft/view.json` (default `~/.config/heft/view.json`) | saved sort, direction, filter, hidden columns, column order |
| config | `$XDG_CONFIG_HOME/heft/rules.d/*.json` | your grouping rules |
| config | `/etc/heft/rules.d/*.json` | administrator rules, read after yours |

The only file heft writes is `view.json` (plus `--trend kitty` shm under
`/dev/shm`). `$XDG_CONFIG_HOME` and `$HOME` must be absolute. `$HOME` is also
what `--fixture` rewrites as `~`. Also read: `NO_COLOR`, `DOCKER_HOST`
(`unix://`), `HEFT_RULES_PATH`, `LC_ALL` / `LC_CTYPE` / `LANG`,
`SSH_CONNECTION` / `SSH_TTY`.

### Columns

A column is drawn whole or not at all. Labels: `name`, `spark`, `nproc`,
`threads`, `fds`, `age`, `core`, `machine`, `pss`, `rss`, `swap`, `vram`,
`gtt`, `gfx`, `compute`, `diskr`, `diskw`, `cpustall`, `iostall`, `memstall`,
`netns_rx`, `netns_tx`. `--sort` / Shift-arrows take all except `spark`;
`--hide` all except `name`; `--order` all of them. Default hidden (no file or
no `hide_columns`): `rss` and the stall trio. Unknown label in the file:
warn and ignore; on `--hide` / `--order`: usage error. Stall columns are the
exception to "hide is presentation only": the TUI and `--once` skip cgroup
pressure while those columns are hidden and not the sort key. `--json` always
samples them.

```jsonc
{
  // `//` and `/* */` comments are allowed here and in rules.d
  "sort": "pss",
  "desc": true,
  "filter": "",
  "hide_columns": ["vram", "gtt", "gfx", "compute"],
  "column_order": ["pss", "rss", "core"]
}
```

`s` rewrites the whole file; comments you add do not survive. `--hide` adds
to the saved list; `--order` overwrites it (`name` stays first unless listed).
A label twice on the CLI is a usage error; in the file the extra is warned
and ignored.

## Rules

Built-ins are `rules.d/` compiled into the binary. User then `/etc` then
built-ins. `HEFT_RULES_PATH` replaces the first two (colon-separated; empty:
built-ins only). heft never writes those directories.

| stage | decides | several matches |
| --- | --- | --- |
| `unit` | `lying`, `service` | every match adds flags |
| `class` | `launcher`, `generic`, `shell`, `terminal`, `compositor`, `worker`, `noise`, `crash_helper`, `no_absorb`, `anonymous_script`, `container_runtime` | every match adds classes |
| `session` | identity and folder for session plumbing | first wins |
| `app` | identity for a helper in an app's install tree | first wins |
| `placement` | `fold_to`, `folder` pin, or container `owner_uid` | first wins |

Within a directory, files are byte order (`10-x.json` before `9-x.json`).
Only regular files (or symlinks) ending `.json`. ASCII case is ignored.

```jsonc
{
  "stage": "placement",
  "rules": [
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

A `match` holds one test: `name` / `name_prefix` / `name_suffix` /
`name_contains` (`comm` and display name); `exe_*`; `arg_*` (each argument);
`cgroup_*`; `unit_*`. `identity` and `container` are placement-only; `script`
only on a `class` rule whose classes are `["anonymous_script"]`. Lists are
any-of; `all`, `any`, `not` combine. `{"not": {"exe_prefix": "/usr"}}` matches
a process whose `exe` heft cannot read. Placement never moves a container or
kernel thread.

`"disable": ["40-trinity.json"]` drops that file from every source;
`"disable": ["40-trinity.json:trinity-session"]` drops one rule. Built-in
file names and ids are interface; do not name your file after a built-in.
A file that does not parse, or breaks the contract (duplicate id, empty
pattern, wrong-stage output), is skipped whole.

`heft --check-rules` runs every `examples` entry; exit 1 on a failed example
or a file that did not load. A built-in example your rules decide differently
is `overridden by` / `disabled by`, not failed.

`heft --explain <PID>` prints the placement key and each stage; exit 1 if
the pid is not visible. A wrong key is silent: an identity that matches
nothing is never consulted. For a grouping bug, attach
`heft --fixture > heft-fixture.json` (exe, cgroup, parent, argv; `$HOME/`
rewritten as `~/`). Nothing is sent.

## Verify

```sh
heft --once
```

Your terminal and each desktop app should be their own Applications rows;
idle shells fold into the terminal; a shell that launched an app bills to
that app. Electron once, not per helper. User Services: compositor and user
`*.service`. Containers: a compose project under one row; shims bill to the
container, never `dockerd`.

Contributor gates: [CONTRIBUTING.md](CONTRIBUTING.md).

## Uninstall

AUR: `pacman -Rns heft-bin` (or `heft` / `heft-git`). `~/.config/heft` stays.

```sh
rm -f ~/.local/bin/heft
rm -rf ~/.config/heft
rm -f ~/.local/share/bash-completion/completions/heft
rm -f ~/.local/share/zsh/site-functions/_heft
rm -f ~/.config/fish/completions/heft.fish
rm -f ~/.local/share/man/man1/heft.1
```
