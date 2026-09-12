# Changelog

## Unreleased

### Added

- `--trend kitty` draws TREND as a kitty-graphics-protocol image rather than
  nine block characters, as a line joined sample to sample rather than a
  filled bar: pixel resolution instead of eight quantised steps,
  and no dependence on the font at all. One image covers the whole column —
  the protocol's row diacritics index into it — so a frame is one escape
  rather than one per row. Where the terminal is local the pixels travel
  through POSIX shared memory and the escape carries only its name; over ssh
  they go inline, which measured about 146 KB a sample on a 24-row terminal,
  so `--trend chars` stays the default and the flag says so. Opt-in, never
  detected: `TERM` names a terminal, not what it implements.

- Both config files take `//` and `/* */` comments. `s` now writes `view.json`
  with a header above it explaining every key and listing the column labels,
  generated from the binary so it cannot go stale — the file explains itself
  rather than sending you to the man page. A save still rewrites the file
  whole, so `grouping.json`, which heft never writes, is the one that keeps
  comments of your own.

### Fixed

- Host, the User rows and the folder headers have no TREND. They are sums,
  and the scale the trend is drawn against is built from the entries, so a
  sum has no figure on it: drawn anyway they sat pinned to the ceiling saying
  only that they were the total, which the header already draws to scale.

- TREND is drawn against one scale for the whole frame rather than against
  each row's own peak: a percentage is full at 100, and everything else scales
  to the heaviest row that is an entry rather than a sum. Against its own peak
  a row sitting flat at 2% had every sample equal to its own maximum, so it
  drew nine full-height marks — "flat and idle" and "flat and busy" were
  opposites, most of the column was a solid block, and no two rows could be
  compared at all. It still follows the sort column and still clears when that
  changes.

## 0.6.2 - 2026-09-12

### Added

- `--glyphs legacy`, for a font that draws the header bars but not the
  sparkline. TREND's ramp needs eight rising steps and six of them — every
  one but `▄` and `█` — are absent from fonts that carry `█▓▒░` perfectly
  well, so the trend column came up as boxes on a screen that was otherwise
  correct. The new set keeps every Unicode character and swaps only the ramp
  for `_.,:-=+#`; `--glyphs ascii` would have answered one column by giving
  up the whole header. `auto` never picks it, because a locale says what the
  terminal can encode and nothing says what the font can draw.

### Fixed

- The VRAM and GTT segments of the MEMORY bar were the quadrant characters
  `▚` and `▙`, and the collapsed-row marker was `▶` (U+25B6). Both sit
  outside what a font carrying the shade ramp necessarily has, so a reader
  whose `█▓▒` bars and `▼` markers drew correctly still got tofu for exactly
  those three. They are now the half blocks `▀` and `▄` and the pointer `►`
  (U+25BA), which look the same and live in the same repertoire as the ramp.
  U+25B6 was the worse of the two for a second reason: it is the base of the
  play-button emoji, so a terminal that resolves emoji presentation drew it
  double-width in a table whose every column is exact.

## 0.6.1 - 2026-09-12

### Fixed

- VRAM and GTT were blank on an amdgpu older than its switch to
  `drm_show_memory_stats`, while gfx% and compute% beside them worked. Those
  kernels publish the region sizes as `drm-memory-vram` and `drm-memory-gtt`,
  the driver's own pre-standard pair, and heft matched only the standardised
  `drm-resident-*`. It now takes the first tier a client publishes —
  `drm-resident-*`, then `drm-total-*`, then `drm-memory-*` — so a current
  kernel still reports what the region is actually holding rather than what
  may be evicted, and an older one reports what it has. Measured on a
  4750G. `drm-total-*` had been documented as the fallback and was never
  matched either.

## 0.6.0 - 2026-09-09

### Added

- A cell whose figure says the row is in trouble is drawn in red, or in
  reverse video where there is no colour: `D` above zero, and a stall column
  at or over 20% of an interval. `D` exists because `%CORE` reads an
  uninterruptible process as idle, but a `3` there had looked exactly like a
  `0` among nineteen other numbers.

- heft says when it cannot account for the machine: `seeing 1% of 4557
  threads`, in the TUI footer and on a `WARN` line under the `--once` host
  line, whenever the tree covers under 90% of the kernel's own thread count.
  On a `hidepid` mount or inside a PID namespace the tree was simply small,
  with nothing to say why.

- `--proc-root DIR` reads `/proc` and `/sys` under `DIR` instead of `/`, for
  another mount namespace's procfs or a tree captured off a machine heft
  cannot run on. A directory with no `proc` in it is a usage error.

- `--json` process nodes carry `cmdline`. A reader could not tell four
  identically-named workers apart without going back to `/proc`, which for a
  `--follow` stream means racing a pid that may already be gone or reused.

- A `TREND` column draws the last nine samples of the sort metric as rising
  blocks, scaled to each row's own peak. Every other column is the current
  interval, so a process that spiked to 400% and went quiet was indistinguish-
  able from one that had been idle the whole time. TUI only — `--once` and
  `--json` take two walks and have no history to draw. `--hide` and `--order`
  take it; `--sort` and `c` do not, since a trend has no ordering.

- `p` pauses the TUI. The table freezes so a row can be read without the
  numbers moving under it, while sampling continues underneath so unpausing
  shows the current machine. The footer says how long the view has been held.

- `man heft` gained KEYS, FILES and ENVIRONMENT. heft's default mode is the
  TUI and the generated page documented every flag and not one keystroke. The
  key list now lives in `src/keys.rs`, which both the man page and the `?`
  overlay read, so they cannot drift the way they had when `i` was added.

- Virtual machines and systemd-nspawn containers (`machine.slice`) are
  Containers rows named after the machine, on Host → Containers. They matched
  no bucket rule, so a libvirt VM appeared as a `qemu-system-x86_64` row under
  root's Applications and an nspawn container as one Applications row per
  process inside it.

- Rootful Podman's `/run/podman/podman.sock` is tried after the rootless one.
  On a RHEL or Fedora server every container rendered `docker-<12hex>` with no
  owner and a blank NETNS, which is what heft shows when it finds no socket at
  all, so the miss was indistinguishable from having no runtime.

- `i` in the TUI opens a detail pane for the selected row: every column,
  including hidden ones and those off the side of a narrow terminal, and for a
  single process the pid, ppid, state, uid, `exe`, cgroup and full command
  line. The tree shows four rows called `cursor`; only the command line says
  which is which. Read from `/proc` on the keypress, so it costs nothing until
  it is opened.

- `--filter` and the TUI's `/` search each row's name **and** the argv of every
  process under it. A row title is an exe basename, so four identical workers
  could not be told apart by the `--port` they hold. The argv is matched on
  collapsed rows too, and is assembled only for a tick that has a filter.

- `--json` documents carry `host.sampled_at`, the unix second the sample was
  taken. A `--json --follow` stream is one document per line with no other
  clock in it, so two records could not be placed in time relative to each
  other — an interval stretched by a PSS pass was invisible to a reader.

### Fixed

- `--interval` below the documented 0.05s floor was accepted and silently
  raised to it, so a caller asking for 0.001 computed rates against a cadence
  heft was not using. It is now a usage error naming the floor, the same split
  as a mistyped `--sort`. `nan`, which parses as a float and would have
  panicked in `Duration::from_secs_f64`, is refused too.

- The TUI clipped a column that did not fit, so a 50-column terminal drew
  `20.1G` as `2` and `548.5` as `5` with nothing to say they had been cut.
  A column is now drawn at its full width or left off, which is what `[` and
  `]` already existed to reach. A wrong figure is the one thing every other
  rule in heft — `SwapPss` over `Swap`, `some` over `full`, blanks on
  multi-cgroup rows — exists to avoid.

- On a cgroup v1 host `identity::user_unit` read the leaf of the whole
  multi-line `/proc/<pid>/cgroup`, which is the empty `0::/` that file ends
  with, so every user unit came back blank. The Applications versus User
  Services split needs a unit name, so User Services was permanently empty and
  every user process filed as an application. It now reads the `0::` line, or
  `1:name=systemd:` where there is none.

- The `i` detail pane stacked its twenty metrics one per line, so a two-line
  column of short values sat beside an empty half-screen and pushed `EXE`,
  `CGROUP` and `CMDLINE` off the bottom of the pane, where they were silently
  cut — the three facts the pane exists to show. They now lay out across the
  pane's width, long values wrap instead of being clipped at the frame, and a
  command line is capped at 240 characters so one Chromium `--enable-features=`
  list cannot crowd out everything else.

### Documentation

- HUMANS.md records that there is deliberately no crates.io package: the
  release binaries are static musl, so installing costs no toolchain and no
  compile. `cargo install --git` is noted for anyone who would rather build.

- HUMANS.md states that the GPU columns read DRM fdinfo from `amdgpu`, `i915`
  and `xe` only, so `nvidia-drm`, `nouveau` and the ARM SoC drivers leave VRAM,
  GTT, gfx% and compute% blank on a machine with a working GPU.

## 0.5.0 - 2026-09-08

### Fixed

- The TUI highlight followed a raw index into the flattened list, so a new
  sample (PSS descending by default), a sort, a filter, `--top`, or expanding
  a neighbour moved the cursor onto a different identity. It now tracks the
  row's id, and lands on the nearest remaining parent when that row is gone.

- The `/proc` walk spawned a `thread::scope` every sample, and glibc created
  new malloc arenas for those threads. RSS climbed ~15 MiB every 5s PSS tick
  to ~488 MiB. The same `available_parallelism` workers now live on the
  Sampler for as long as it does. `MALLOC_ARENA_MAX=2` plateaued at 39 MiB;
  pooling is the product fix, not that knob.

- PSS/`--once` GPU collection slurped every fdinfo when no fd name contained
  dri/drm, including a 16,038,344-byte `anon_inode:[fanotify]` dump on
  `localsearch-3` with no `drm-client-id`. Files above 64 KiB are not read
  (real drm fdinfo on this host topped out at 14 KiB).

- An AppImage Chromium crash helper reparented to user systemd
  (`chrome_crashpad_handler` under `/tmp/.mount_…/usr/share/cursor/`) took its
  own Applications row instead of billing to the app. The mount directory is
  still refused; a stable directory nested under it is the same owner an
  `/opt/cursor/…` path already named.

- An empty folder in the default expand set (your user's Containers, with no
  containers) drew the expanded marker over a blank gap. Folders with no
  identities are not expandable; the id stays in the set so the first row
  that appears still opens.

- A crash helper that was a launcher's only child took an application row of
  its own, titled `crashhelper`, instead of billing to the app its own path
  names. The sandbox fallback in the grouping walk now reaches the same verdict
  every other placement site does, so a container or kernel row is possible
  there too.

- The `--once` table counted characters where it meant terminal columns, so a
  process named in CJK or carrying an emoji pushed every column to its right
  out by one per wide character. `trunc` and the row layout measure columns
  now, which is what the exact-width claim always said they did.

- The key-help overlay drew its cursor keys as literal arrow characters
  regardless of `--glyphs`, so the one screen explaining the keys rendered as
  tofu on the bare console that flag exists for.

- An AppImage launcher whose filename is not all lowercase took a top-level row
  of its own, titled with the full `Cursor-x86_64.AppImage` filename, instead of
  billing to its payload. The suffix is now matched case-insensitively wherever
  it is matched at all; shipping AppImages are mixed case.

### Added

- `NO_COLOR` is honoured. Presence and non-emptiness decide, not the value, so
  `NO_COLOR=0` disables hue as well, which is what no-color.org specifies. The
  bars lose nothing by it: each segment's fill character already carried the
  distinction, which is why the variable could be answered by dropping styling
  rather than by adding a second render path.

- `--order` / `column_order` in `view.json`: left-to-right column order,
  repeatable, overwriting a saved list the way `--hide` does. Unlisted columns
  keep compiled order after the named ones; `name` stays first unless the
  list includes it. `--json` refuses the flag.

- `H` hides the current sort column and `u` puts the last hidden one back;
  `--hide` does the same on `--once` (repeatable) and overwrites a saved list.
  `hide_columns` in `view.json` was already the store; it was only writable by
  hand. `name` is refused. `--json` refuses the flag, the same contract as
  `--filter`.

- Plasma session helpers merge under `plasmashell` and kwin helpers under
  `kwin`, from those RPMs' shipped binaries rather than a `plasma-` prefix.
  `krunner` and `plasma-discover` stay independent apps.

- The TUI reverses the header of the column `c` is sorting by. The rest of
  the header row stays bold; `--once` is unchanged, because that table is a
  fixed-width contract other tools slice. Reverse rather than a colour, so
  `NO_COLOR` still marks the column.

- A `D` column counting the processes on a row in uninterruptible sleep. The
  stall columns answer "was this row waiting", but only on the ~82% of rows
  that resolve to one cgroup, and never on a folder, User or Host row, because
  a percentage of an interval cannot be summed. `D` is a count, so it adds up
  the tree and carries a figure exactly where those blanks are. It reads field
  3 of the `/proc/<pid>/stat` line heft already parses for CPU and `THR`, so it
  costs no extra read, and `0` is a figure rather than a blank.

### Changed

- Idle interactive shells fold into their terminal (Ghostty and the other
  names in `TERMINALS`). A shell that launched a single real app still bills
  to that app, the same unique-payload walk launchers use. They are no longer
  their own Applications row.

- `--once` and PSS ticks no longer walk every fdinfo when the dri/drm name
  prefilter is empty. That walk was ~318 ms of ~760 ms serial PSS-tick kernel
  work on 308 pids with no dri/drm fd. GPU clients whose fd names omit
  dri/drm stay blank.

- Folder headings (Applications, User Services, Containers, System) show how
  many identities sit under them, including zero. User rows are the login name
  only: the uid in parentheses looked like a count.

- `D` sits left of `%CORE`, and `DISK R` / `DISK W` sit after `CMP`. `%CORE`
  reads D-state as idle, so the count belongs beside it; disk rates belong
  with the other per-interval costs, after GPU engine percentages.

- The two header bars are drawn to one width, so the CPU and MEMORY brackets
  stack instead of each row sizing its bar around its own text. The MEM group
  spends more of its row on `] used/total` and a fourth legend label, so it
  sets the width and the CPU row pads on the right; where the MEMORY row splits
  into tanks it is the first tank that is matched.

- The TUI header no longer prints the host `psi` tail; the figures are
  unchanged on `--once` and in `--json`, where nothing is drawn to scale.
  Measured on a 175-column terminal: that tail was cancelling most of the
  suffix difference above by accident, so removing it on its own took the two
  brackets from 4 columns apart to 14. The shared bar width is what closes
  them.

- Clap help and usage errors are uncolored. The TUI already honours `NO_COLOR`;
  clap's default `color` feature was pulling `anstream` for stderr heft does
  not paint.

## 0.4.0 - 2026-09-07

### Fixed

- Every `Ctrl-` key ran its unmodified binding. Raw mode turns `ISIG` off, so
  the terminal never raises `SIGINT` and heft has to answer `Ctrl-C` itself; it
  did not. Reproduced in a pty against the release binary: one `Ctrl-C` moved
  the sort from `pss` to `rss` and a second to `swap`, exactly as pressing `c`
  twice; `Ctrl-D` flipped the sort direction; and `Ctrl-S` wrote `view.json`
  with no `s` ever pressed. `Ctrl-C` now quits, from the filter editor as well
  as the table, and every other `CONTROL`, `ALT` or `SUPER` combination is
  dropped. `SHIFT` still reaches the bindings, since a capital is how you type
  one.

- The terminal is put back when heft does not exit through `run()`. The release
  profile is `panic = abort`, so a panic never unwinds and no teardown ran, and
  `SIGTERM`/`SIGHUP`/`SIGQUIT` terminated outright — leaving the shell in raw
  mode inside the alternate screen with no echo. Measured in a pty before and
  after: without the guard all three signals left canonical mode and echo off
  and the alternate screen active; with it, all three restore and exit
  `128 +` the signal.

- `systemd_unescape` pushed each decoded byte as a `char`, reading systemd's
  byte-at-a-time escapes as Latin-1, so `\xc3\xa9` rendered `Ã©` rather than `é`
  and any unit or scope name that was not pure ASCII appeared as mojibake.

### Added

- `CPU ST` / `IO ST` / `MEM ST`, per-cgroup stall percentages from the kernel's
  pressure stall information, plus a `psi` figure on the host header. `%CORE`
  says a row used the processor and `PSS` says it holds memory; neither can
  tell an application halfway through its work from one blocked on the disk,
  because both look idle. The columns are Δ`some ... total=` over wall clock so
  they share a time base with `%CORE` and the disk rates beside them, while the
  header uses the kernel's own `avg10`, where a machine-wide trend reads better
  smoothed — the same deliberate split `gfx%`/`compute%` already carries.

  A row carries a figure only when it *is* one non-root cgroup. A stall is a
  percentage of an interval and not a quantity, so two cgroups cannot be added,
  and `Metrics::accumulate` leaves the trio out exactly as it leaves out the
  netns pair. That excludes more than it first appears: `user-<uid>.slice` is
  not heft's User row, because a rootful container lives in `system.slice` and
  is still billed to its owner, and `system.slice` is not the System row,
  because kernel threads sit in the root cgroup. A row resolving to the root
  cgroup is blank because that pressure is the machine's — the rule that
  already blanks a `--network=host` container. A process row is blank unless
  its cgroup holds only that pid, since a cgroup's stall is not one process's.
  Measured, 67% of user identity rows carry a figure on one desktop, and
  sampling costs about 5 ms a tick.

- `--follow`, so heft keeps sampling instead of exiting after one. `--json
  --follow` emits one compact document per line (NDJSON, so a reader takes a
  line at a time without a streaming parser) and `--once --follow` reprints the
  table each interval, each sample carrying its own header. It honours
  `--pss-interval` where the one-shot forms force PSS, because reading
  `smaps_rollup` for every process once a second forever is the cost that flag
  exists to avoid. No `--count`: heft already exits quietly on `EPIPE`.

- `--top N`, keeping the N heaviest rows under each parent at every depth, with
  the subtree of a cut row going with it. It never trims Host, a User or a
  folder header: the sort does not order those at all, so a "top two" of them
  cuts arbitrarily — caught in testing, where `--top 2` on a two-user machine
  silently dropped the entire System section. Row-level rather than tree-level,
  so a surviving parent still shows the total it was built with; trimming the
  tree instead made `Host` report 85 processes on a machine running 813.

- `--desc` and `--asc`. `--sort age` alone meant whichever direction the saved
  view happened to hold, so the same command printed differently on two
  machines.

- `--glyphs auto|unicode|ascii`. Bars, rules, expand markers and the table's
  truncation ellipsis were all block or box-drawing characters, which render as
  tofu on a bare console or a container stripped to a few fonts — worse since
  glyphs became the primary channel for telling bar segments apart. `auto`
  reads `LC_ALL`, `LC_CTYPE` then `LANG`; no locale set at all resolves to
  ASCII, since that is the C locale and the machine most likely to lack the
  fonts. Every substitute is one column, because the header lines land on an
  exact width and the table truncates to an exact column count.

- `--user <NAME|UID>`, repeatable, cutting the tree to one owner's branch.
  System and Host-level Containers survive it, since they are the machine's
  cost and belong to nobody and a tree without them stops explaining the header
  above it. It applies to `--json` too, unlike `--filter`, because the JSON
  tree does have User nodes for a prune to mean something. It is never written
  to `view.json`: a saved user cut would hide most of the machine on every
  later run for a reason held by the file rather than the command line.

### Changed

- `--filter` and the `/` key take a **regex** rather than a substring, so
  `^(code|claude)$` picks exactly two rows where a substring also dragged in
  every helper process beside them. Case-insensitive unless the pattern says
  otherwise, so every filter anyone had saved keeps behaving as it did.
  `regex-lite`, not `regex`: measured, the full engine takes the stripped
  binary from 1.53 MB to 2.93 MB and pulls four more crates in for a SIMD
  literal search that matches a few hundred process names once a tick, against
  70 KB and one crate for the same syntax minus Unicode character classes. A
  pattern that does not compile is a usage error when typed, a
  warning-and-ignore from a saved `view.json`, and in the TUI a `?` in the
  footer while the last working pattern keeps filtering.

- The `/proc` walk is split across threads. Almost all of its wall clock was
  this process waiting on the kernel to build one small file at a time, so the
  pid list now goes to a `thread::scope`. Ten interleaved runs of
  `--once --interval 0.05` on a 777-pid host: 1.02s to 0.41s. The gain is not
  uniform — an ordinary tick goes 120ms to 20ms, which is what makes the
  documented 0.05s `--interval` floor reachable rather than aspirational, while
  a PSS tick only goes 850ms to 340ms and still stretches its interval, because
  `smaps_rollup` makes the kernel walk page tables and that is memory-bound.

- Every bar segment now carries its own fill glyph as well as its own colour,
  and the legend prints that glyph beside the label (`█usr/▓sys/▒wait`). Hue
  alone could not carry the distinction: cyan against magenta is the pair
  deuteranopia collapses, and a piped or recorded frame keeps the characters
  while losing the styling. Unconditional rather than gated on `NO_COLOR`, so
  there is one render path instead of two that drift, and nobody needs to know
  an environment variable to read a bar.

- Releases now carry `aarch64` binaries beside `x86_64`, each in a `musl` and a
  `gnu` build, and every release binary carries a signed build provenance
  statement (`gh attestation verify`). A published `.sha256` proves a download
  arrived intact; provenance proves which workflow at which commit produced it.

## 0.3.0 - 2026-09-06

### Added

- `--sort <column>` and `--filter <text>`, so a script gets the top rows
  without piping through `sort` and losing the indentation that makes the tree
  readable. `--sort` takes the same labels the `c` key cycles and `view.json`
  saves, so all three name a column the same way, and an unknown one is a usage
  error rather than a fallback: a stale saved view must not stop the monitor,
  but a name just typed can still be corrected. `--filter` is the `/` key and
  runs through the same `keep_matches`, so it keeps the ancestors of a match —
  a filter that dropped them would print an orphaned tree — and those ancestors
  keep the totals they were built with rather than the totals of what survived.
  An explicit flag beats a saved `view.json`. `--once` otherwise starts from
  that saved view; `--json` never reads it, because its shape is a documented
  contract and a human's TUI preference is not part of it, and `--filter` is
  refused with `--json` since the JSON tree has no folder rows for "keep the
  ancestors" to mean anything there.

- `hide_columns` in `view.json`: a list of column labels the table leaves out.
  Seventeen columns no longer fit a terminal, which is why `[` and `]` exist,
  and scrolling past a column every tick is not the same as never wanting it.
  It is a hide list rather than a show list because heft keeps growing columns
  and a show list would silently withhold every one added after the file was
  written. It sits in `view.json` beside the sort and filter the `s` key
  already saves, not in `grouping.json`, which is read-only and about identity
  rather than presentation. Absent file or absent key renders exactly as
  before. `name` is refused, since a table of numbers with no labels is
  unreadable, and an unknown label warns on stderr and is ignored the way a
  malformed `grouping.json` does — not `Sort::from_label`'s silent fallback,
  because a mistyped sort still prints a usable table while a mistyped hide
  entry would do nothing and say nothing. Hiding reaches presentation only:
  the same `/proc` files are read either way, so every roll-up invariant still
  holds, the `c` cycle skips what it cannot show rather than moving the sort
  somewhere invisible, and `--json` ignores the list entirely — a consumer
  parsing the tree did not ask for a human's column preference.

- `THR` and `AGE` columns, both from the `/proc/<pid>/stat` heft already parses
  for CPU, so neither adds a per-tick read. `N` counts processes, so a thread
  leak was invisible: one process holding 4000 threads rendered identically to
  one holding none. Threads sum up the tree the way `N` does. `AGE` is
  `now - (btime + starttime / CLK_TCK)`, shown in the largest unit that fits
  (`45s`, `12m`, `3h`, `9d`) rather than raw seconds or a start timestamp that
  would leave the reader to do the subtraction. On a row covering several
  processes it is the oldest of them, which is when the thing on that row first
  appeared; it is deliberately not a sum, since summed durations mean nothing,
  and a maximum cannot be misread as a total. `btime` is read from `/proc/stat`
  once per run and pinned, so an NTP step cannot walk ages heft has already
  printed.

- A `SWAP` column and a host swap tank on the MEMORY header row. Both come from
  files heft already opens — `SwapPss:` from the `smaps_rollup` it reads for
  PSS, and `SwapTotal`/`SwapFree` from the `/proc/meminfo` it reads for the MEM
  bar — so neither costs a tick any extra I/O, and per-process swap arrives on
  the existing `--pss-interval` cadence. The column is `SwapPss` rather than
  `Swap` for the reason PSS is the memory column: `Swap` bills a shared
  swapped-out page to every process mapping it, which a summed tree then
  reports several times over. Swap is a tank of its own and never a segment of
  the MEM bar, because swapped pages are not in RAM. A machine with
  `SwapTotal: 0` renders exactly as before: no tank, no swap field on the
  `--once` host line, and blank `SWAP` cells rather than a column of zeros,
  since with no swap configured there is no figure to report. Until now a
  process with gigabytes paged out showed only its small resident RSS and PSS,
  and heft's memory picture was actively misleading on any host that swaps.

- `NETNS RX` / `NETNS TX` on container rows, from the non-`lo` interfaces of
  `/proc/<pid>/net/dev`. Every other row is blank, and the columns are named
  for the network namespace rather than the resource because that is what the
  counter belongs to: a container gets a figure by owning a namespace, and
  nothing else in the tree owns one. Per-process network I/O is not deferred,
  it is unavailable without CAP_NET_RAW, CAP_BPF or ptrace — see
  [CONTRIBUTING.md](CONTRIBUTING.md) for what was measured. A
  `--network=host` container stays blank, since its counters are the
  machine's, and a restart drops one interval instead of reporting a negative
  rate.

### Changed

- Running the TUI without a terminal on stdout now says so and names `--once`
  and `--json`, instead of `No such device or address (os error 6)`. The check
  runs before the first `/proc` walk, since surfacing it afterwards would burn
  a whole `--interval` first. Still non-zero — it is a usage error — and
  deliberately not a quiet fall back to `--once`, which would surprise anyone
  piping heft expecting a TUI.

### Fixed

- `heft --once | head` and `heft --json | head` exit quietly instead of
  reporting a broken pipe. `--json` previously panicked outright, which the
  `panic = abort` release profile turns into an abort. A genuine write failure
  such as a full disk is still reported.

## 0.2.0 - 2026-09-06

### Added

- Prebuilt binaries on every `v*` tag, so installing no longer needs a Rust
  toolchain. Each release carries `x86_64-unknown-linux-gnu` and a static
  `x86_64-unknown-linux-musl` build that runs on any glibc, plus a `.sha256`
  beside each one.

- Shell completions (bash, zsh, fish) and a `heft.1` man page, generated at
  build time from the same clap definition the binary parses, so they cannot
  drift from the real flags. `clap_complete` and `clap_mangen` are
  build-dependencies only; the shipped binary gains nothing. Install paths:
  [HUMANS.md](HUMANS.md).

- Optional grouping overrides in `$XDG_CONFIG_HOME/heft/grouping.json`, so a
  local daemon, worker, or container no longer needs a patch to heft's compiled
  tables. `applications` / `user_services` pin an identity to a folder, `fold`
  bills one identity to another, and `container_owners` pins a container to a
  uid. Heft only reads the file; with none present grouping is unchanged. An
  override beats every built-in table but cannot move a container or a kernel
  thread, and a malformed file warns on stderr instead of stopping the monitor.

- Intel GPUs report VRAM/GTT and gfx%/compute% instead of blank columns. The
  driver gate accepted only `amdgpu`, so i915 and xe users saw nothing; their
  fdinfo region and engine names now map onto the same counters. NVIDIA still
  needs NVML and stays out.

- Intel Xe/Arc reports gfx% and compute%. xe publishes engine busy as GPU
  cycles against `drm-total-cycles-*` rather than nanoseconds, so those columns
  were blank; they now come from that cycle ratio, divided by
  `drm-engine-capacity-*` where a class has several engine instances. amdgpu
  and i915 keep the nanoseconds-over-wall-clock rate unchanged.

- Discrete GPUs get a VRAM bar measured against the card's own
  `mem_info_vram_total`. It shares the MEMORY header row with the MEM bar
  rather than adding a third row, so a desktop-GPU user no longer sees VRAM
  silently dropped from the header.

- GTT now paints in the MEM bar on a discrete card, and on any GPU heft has no
  `mem_info_vram_total` for. Splitting VRAM into its own tank had dropped GTT
  from the header entirely, even though GTT is system RAM pinned for the GPU
  and is already inside `used`. The figure sums `drm-resident-gtt` over the drm
  clients heft can see, not the sysfs `mem_info_gtt_total` capacity.

### Changed

- Container ownership comes from bind-mount sources: a container with no
  workdir label now bills to the first non-root uid owning one of its
  `Mounts` bind sources. Named volumes are skipped (root-owned under
  `/var/lib/docker/volumes`), and a container that mounts only root-owned
  paths still sits on Host → Containers, so any tool laying out per-user bind
  mounts is attributed rather than only one vendor's.

### Fixed

- A crash helper unpacked under a temp root no longer invents an Applications
  row from the mount directory (`/tmp/mount` rendered as a row literally
  titled `mount`). AppImages mount at a per-run path, so that name was never
  an app identity; the helper now bills to the ancestor that launched it.

## 0.1.0 - 2026-09-06

First release.

### Added

- Fullscreen TUI, plus `--once` and `--json` for one sample, over the tree
  Host → User (Applications | User Services | Containers) → System.
- Per-row `%core` / `%machine`, PSS, RSS, disk read/write rates, and amdgpu
  VRAM / GTT / gfx% / compute% from fdinfo.
- Read-only Docker and Podman attribution: `GET` only, containers billed to
  their workload rather than to `dockerd` or `containerd`.
- `--interval` catch-all sampling (default 1s, floor 0.05s) and TUI-only
  `--pss-interval` (default 5s). Between PSS reads heft reuses the last
  per-PID value; `--once` / `--json` always read PSS on the published sample.
- Sort and filter across every level of the tree, saved to
  `$XDG_CONFIG_HOME/heft/view.json` with `s`. Default is PSS descending, and a
  metric heft cannot read sorts last rather than as a zero.
- User Services logical groups (GNOME Settings Daemon plugins, GVFS, Flatpak
  session helper/portal, xdg-desktop-portal family, evolution-data-server)
  merge by unit/package/D-Bus family, not by comm prefix.
- `$XDG_CONFIG_HOME/heft` is the only directory heft creates, and only when
  you save a view. No state or cache tree.
