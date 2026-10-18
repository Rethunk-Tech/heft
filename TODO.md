# TODO

[HUMANS.md](HUMANS.md) and [AGENTS.md](AGENTS.md) describe what heft does.

## Unattributed GPU on root-owned containers

The MEMORY header takes VRAM and GTT from `mem_info_vram_used` and `mem_info_gtt_used` when amdgpu publishes them, and from the drm clients heft can read otherwise. The Host row sums only visible PIDs. An unprivileged reader cannot open `/proc/<pid>/fdinfo` for a process it does not own, so a drm client in a root-owned container is missing from every row and the gap between the driver total and the readable clients disappears. `containers::unix_get` sends GET, and `docker_get_path` allows only `/containers/json` and `/containers/<id>/json`.

When the driver total exceeds the sum of readable clients, show that remainder on the container row. Heft keeps sending only the GETs `docker_get_path` already allows.

### Read

- `HUMANS.md:240-245` — kernel counters, the fdinfo blind spot, and the refusal to POST.
- `src/ui.rs:1561-1571` — unified MEM and GTT prefer `mem_info_*_used`, then `host_metrics`.
- `src/ui.rs:1616-1617` — the discrete VRAM tank is `vram_used_bytes` alone.
- `src/mem.rs:105-126` — `read_gpu` sums `mem_info_vram_*` and `mem_info_gtt_*` per card.
- `src/cpu.rs:274-292` — `header_from` copies that pool onto the host header.
- `src/group.rs:1143-1158` — an identity's metrics are `sum_metrics` over pids heft could read.
- `src/proc.rs:1047-1059` — `build_tree`, then `net.apply` and `stalls.apply`, where a figure with no pid is attached.
- `src/types.rs:418-424` — `host_metrics` sums those identity metrics, so a remainder added on two rows is counted twice.
- `src/group.rs:613-621` — an unattributed container is Host → Containers.
- `src/containers.rs:329-341` and `src/containers.rs:396-403` — GET only.

### Traps

- The header tanks already show the kernel totals. Keep `mem_header_line` on `vram_used_bytes` and `gtt_used_bytes` ahead of `host_metrics`, or the bar counts the remainder twice once a row carries it.
- `sum_metrics` runs inside `build_tree` and only sees readable pids. Attach the remainder afterwards, once, on one identity, the way `net.apply` attaches a figure the processes themselves do not hold.
- The remainder is one VRAM figure and one GTT figure for the pooled driver totals. A second container row that also carries it multiplies it in `host_metrics`.
- The same fdinfo blind spot covers any pid heft does not own. With no container row, the remainder stays off the tree.
- i915 and xe leave `GpuPool`'s used fields unset (`src/mem.rs:13-14`). There is no driver total to subtract from.
- Compare bytes with bytes. fdinfo sizes are scaled in `gpu.rs`; the sysfs counters are already bytes.
- A zero remainder leaves the row's cells as the readable clients left them. A blank cell is not a zero (`HUMANS.md:288-291`).

### Acceptance

- On amdgpu, with `mem_info_*_used` above the sum of readable clients and one unattributed root-owned container, that container row shows the difference once, for VRAM and for GTT.
- A second container row does not also carry it, and `host_metrics` for that column equals the kernel used count.
- The MEMORY header still draws from `mem_info_*_used`.
- `docker_get_path` still allows only the two GET paths.
- i915 and xe gain no remainder.
- When the kernel used count equals the readable sum, the container row is unchanged.

## Ryzen AI NPU, measurement first

The GPU columns read amdgpu, i915, and xe fdinfo. This machine is a Strix Halo APU, and the NPU is a device beside that GPU. Whether it publishes utilization an unprivileged process can read is unmeasured. This entry names no sysfs path.

Measure whether the NPU publishes unprivileged fdinfo or sysfs utilization. Add a column only when a non-root read returns that utilization. When the node is root-only or absent, the outcome is a measured-not-worth note in this section.

### Read

- `HUMANS.md:271-278` — the drivers and fdinfo names the columns read.
- `AGENTS.md:25` — `gpu.rs` is amdgpu, i915, and xe.
- `src/gpu.rs:177-194` and `src/gpu.rs:225` — region names and the driver match.
- `src/gpu.rs:247-248` — a client with `drm-client-id` and `drm-resident-*` counts even when the driver name is none of those three.
- `CONTRIBUTING.md:98-101` — observe-only, and no guessed fdinfo keys.

### Traps

- A sysfs, debugfs, or accel path belongs in this file, HUMANS.md, or a column only after it has been opened as the user heft runs as.
- A root-only node is the result of the measurement. Record it here. Reading it as root is a different program.
- The fallback at `src/gpu.rs:247-248` may already be counting the device's memory under VRAM or GTT. The measurement says whether that is happening before any new driver arm.
- A key name taken from a kernel tree and absent from the read is a guess. `CONTRIBUTING.md:99-101` leaves those keys unread.
- The reading that closes this section is this host. It does not by itself define a column for every Ryzen AI part.

### Acceptance

- The measurement is a non-root open of the NPU node this kernel exposes, recorded as the bytes or the error it returned.
- A column ships together with the key that read returned, a HUMANS.md sentence for it, and a fixture taken from that read.
- When the open fails for permissions, or the node is absent, this section is the measured-not-worth note and the tree has no new column.

## Network rates exist only on container rows

Per-netns rx/tx comes from `/proc/<pid>/net/dev` and is applied to container rows only. Host and application rows therefore have no network numbers. `AGENTS.md:23` states that layout.

Show rx/tx on non-container rows from that same read. EACCES blanks the metric, the same rule as other unreadable `/proc` fields. Do not add `docker exec` or any POST. Do not change container attribution.

### Read

- `AGENTS.md:23` — `src/net.rs` is per-netns rx/tx from `/proc/pid/net/dev`, container rows only.
- `AGENTS.md:165` — NETNS RX/TX is the delta of non-`lo` bytes from `/proc/<container-scope-pid>/net/dev`.
- `src/net.rs:33-36` — one `net/dev` read per container and none for anything else. The file is the namespace, so an ordinary pid returns the host namespace's total.
- `src/net.rs:45-60` — `Sampler::tick` keeps a delta only for those container pids, keyed by identity and member.
- `src/net.rs:73-85` — `netns_pids` is the lowest pid in a container's own cgroup scope, and only when `own_netns`.
- `src/net.rs:88-109` — `apply` bills `tree.users[].containers` and `tree.containers`, and nothing else.
- `src/net.rs:113-115` — `set` writes `None` when there is no rate.
- `src/net.rs:124-132` — a missing previous sample, a new pid, or a counter that went backwards discards the interval.
- `src/net.rs:135-138` — `read_pid` turns a failed read into `None`.
- `src/net.rs:141-144` — every interface but `lo`; the host-side veth is unread because `/proc/<pid>/ns/net` is EACCES for a root-owned pid.
- `src/net.rs:272-297` — `only_container_rows_are_billed` locks an application row that shares the container's key at `None`.
- `src/types.rs:158-165` and `src/types.rs:195-196` — the pair is documented as container-only, and `accumulate` does not sum it.
- `src/once.rs:209-213` — the column is named NETNS so a blank process cell means no figure, not zero bytes.
- `HUMANS.md:288-291` — a blank is not a zero.
- `HUMANS.md:386-397` — only a container row carries the columns; a `--network=host` container is blank.
- `src/io.rs:6-7` and `src/proc.rs:301-315` — an unreadable `/proc` file is `None`, not zero.
- `src/containers.rs:329-341` and `src/containers.rs:396-403` — GET only.

### Traps

- A process and its container must not double-count the same netns. `net/dev` is one counter for the namespace (`src/net.rs:33-36`), and the container row already holds it (`src/net.rs:96-109`). Writing that total again onto a process in the namespace counts the bytes twice on any row that sums both. `accumulate` currently refuses the sum (`src/types.rs:195-196`).
- Processes in the host namespace each read that same host total (`src/net.rs:33-36`). A `--network=host` container is that namespace and is not read (`src/net.rs:79-80`); its container row stays blank.
- A blank stays blank rather than zero. EACCES is `None` from `read_pid` (`src/net.rs:136`) and stays `None` through `set` (`src/net.rs:113-115`). A `0.0` rate is a counter that did not move. A discarded interval (`src/net.rs:124-132`) stays unpublished.
- A readable file whose only interface is `lo` parses as `(0, 0)` (`src/net.rs:152-153`). That is a read, not an EACCES blank.
- No `docker exec` and no POST. `unix_get` writes a GET, and `docker_get_path` allows two paths (`src/containers.rs:396-403`, `src/containers.rs:329-341`).

### Acceptance

- A non-container row with a readable `net/dev` shows rx/tx from that read.
- EACCES on that file leaves `net_rx_bps` and `net_tx_bps` blank, not zero.
- Container rows stay as they are: the same scope pid, the same identity and member keys, a `--network=host` container still blank, a project row still the sum of its containers.
- A process that shares a container's netns does not add that namespace's bytes on top of the container row.
- `docker_get_path` still allows only the two GET paths.

## NVIDIA and nouveau fdinfo keys are unread

The GPU columns accept a client whose `drm-driver` is `amdgpu`, `i915`, or `xe`, or whose fdinfo has both `drm-client-id` and a `drm-resident-*` key. Memory and engine counters are still only the names those three drivers print. A nouveau or nvidia client can therefore appear as a row with blank VRAM/GTT/GFX/CMP, or not appear, depending on whether it publishes `drm-resident-*`. This host is an amdgpu APU; the names another driver prints have not been read here.

Measure an unprivileged open of that driver's fdinfo. Add region and engine names only when that read returns them. When the node is absent or EACCES, the outcome is a measured-not-worth note in this section. Do not guess keys from a kernel tree.

### Read

- `HUMANS.md:272-279` — the three drivers, the resident fallback, and the refusal to guess other region names.
- `AGENTS.md:25` — `gpu.rs` is amdgpu, i915, and xe.
- `src/gpu.rs:174-191` — `MEM_PREFIXES` and `mem_key` region names.
- `src/gpu.rs:222` — `drm-driver` is those three strings.
- `src/gpu.rs:244-246` — a client with `drm-client-id` and `drm-resident` still counts.
- `CONTRIBUTING.md:98-101` — observe-only; no guessed fdinfo keys.

### Traps

- The resident fallback may already be counting the client's memory under a name `mem_key` does not map, so the client is present and the cells stay blank. Measure that before adding a driver arm.
- nvidia proprietary fdinfo is not the DRM usage-stats document. A key from that blob that this user cannot open is a different program.
- `mem_key` returning `None` skips the line. Adding a region that is pinned system RAM to VRAM repeats the GTT-vs-VRAM split `HUMANS.md` already documents for amdgpu.
- A fixture taken as root is not the unprivileged read the columns may use.

### Acceptance

- The measurement is a non-root open of that driver's fdinfo, recorded as the bytes or the error.
- A column or mapped region ships together with the key that read returned, a HUMANS.md sentence, and a fixture from that read.
- When the open fails or the node is absent, this section is the measured-not-worth note and `mem_key` / `drm-driver` stay as they are.
- i915, xe, and amdgpu mappings do not change.

## `--explain --json` is a conflict, not a document

`--explain` writes a human report. `--json` is a host-tree contract. clap refuses the pair (`conflicts_with_all` includes `json`). A script that wants the placement path, identity, and per-stage rule for one pid has to scrape the text, and `ok(false)` vs a missing pid is an exit code rather than a JSON field.

A second, small JSON object for `--explain --json` is a new document shape, not a field on `HostTree`. It must not reuse the `{ "host": … }` envelope, and `--json` without `--explain` must stay the tree. That is a contract addition, so it waits for an explicit ask.

### Read

- `src/cli.rs:68-69` — `--explain` conflicts with `once`, `json`, and `follow`.
- `src/main.rs:63-67` — explain is `explain::run`, never `print_json`.
- `src/explain.rs:241-268` — text to stdout; `Ok(false)` when the pid is not visible.
- `src/types.rs` `HostTree` — the `--json` document. Do not hang explain fields off it.
- `HUMANS.md:87` and `:552-557` — the text report and the exit-1-when-not-visible rule.

### Traps

- Lifting the clap conflict without a new shape would send `--json --explain 1` down `print_json` and ignore the pid.
- Putting the report on `HostTree` changes every `--json` consumer.
- `Ok(false)` must remain distinguishable from a write error. A JSON miss is a document with a miss field plus exit 1, not an empty `{}` on stdout with exit 0.
- `--explain` is one walk without rates (`AGENTS.md:143`). The JSON object must not grow rate fields the text report does not have.

### Acceptance

- `heft --explain PID --json` prints one object with pid, visible, placed path, identity, instance, and the per-stage rule lines (or `null` expect-equivalents). Exit 1 still means not visible.
- `heft --json` is unchanged: `{ "host", "interval_secs" }` and no explain keys.
- `--once --explain` stays a conflict.
- `HUMANS.md` names the object keys in the same commit.

## A failed `s` ends the TUI

`s` calls `config::save_view` with `?`. `handle_key` returns `Result<bool, Error>`, and the event loop treats `Err` as fatal (`handle_key(...)?` then `break` is only the `Ok(true)` quit path; an `Err` leaves `ui::run`). A full disk or an unwritable XDG directory therefore tears down the alternate screen instead of saying so in the footer the way "name cannot be hidden" does.

Keep sampling. Put the error in `app.status`. Do not change the on-disk write (temp file, rename, 0o600).

### Read

- `src/ui.rs:931-933` — `save_view` then `saved {path}` on success.
- `src/ui.rs:877-882` — `handle_key` returns `Result<bool, Error>`.
- `src/ui.rs:293-299` — `handle_key(...)?` on each press.
- `src/ui.rs:946` — hide refusal is a status string, not an `Err`.
- `src/config.rs:201-228` — `save_view` still returns io errors after cleaning the temp file.

### Traps

- Mapping every `handle_key` error to a status would swallow a panic-path or a terminal write failure that should still quit.
- The footer already shows `status`. A multi-line io error must be one line, like other status strings.
- `s` and `S` are the same arm. Both stay.
- Tests that `unwrap()` `handle_key` for `s` will start seeing `Ok` on a forced save failure; they must assert `status` instead of `Err`.

### Acceptance

- When `save_view` fails, the TUI stays up, the previous `view.json` is untouched, and the footer names the error.
- When it succeeds, the footer still reads `saved` plus the path.
- `q` / `Esc` / `Ctrl-C` still quit.
- `save_view`'s rename and mode do not change.

## Rules cannot match cwd, while `/` can

`--filter` and the TUI `/` haystack search each process's working directory. Rule `Facts` has no cwd field, `facts_of` does not copy `Process::cwd`, and an example cannot name `cwd` (`deny_unknown_fields`). A rule that should pin everything in `~/src/foo` has to match name, exe, or argv instead.

Add `cwd` / `cwd_prefix` / `cwd_suffix` / `cwd_contains` on the same terms as `exe`, fed from `Process::cwd`. Empty cwd (kernel thread, EACCES) is a miss, the same as a missing exe. This is deferred because a cwd rule changes grouping, not just the filter, and empty-vs-`/` vs the `~/` fixture rewrite need a decision before the key exists.

### Read

- `src/once.rs:960-968` and `:991-996` — filter haystack includes cwd.
- `src/types.rs:50-52` — `Process::cwd` is `/proc/<pid>/cwd`, empty when unread.
- `src/group.rs:102-113` — `facts_of` copies comm, name, exe, argv, cgroup, not cwd.
- `src/rules.rs:330-340` — `Facts` has no cwd.
- `src/rules.rs:204-226` — `Example` has no cwd key.
- `HUMANS.md:128-137` — `/` matches argv and working directory.
- `HUMANS.md:579-583` — `--fixture` rewrites `$HOME` as `~` in some fields.

### Traps

- Evaluation must stay allocation-free (`rules.rs` `evaluation_allocates_nothing`). Borrow `Process::cwd`; do not stringify per tick.
- A fixture example with `cwd: "/home/me/src"` will not match a live process whose home was rewritten to `~` unless examples follow the fixture rewrite, or they don't and HUMANS says so.
- `{"cwd_prefix": "/"}` matches every readable cwd. Empty prefix already fails the file; a one-character prefix that matches everyone still loads.
- Kernel threads stay unmatched. Do not treat empty as `/`.
- Placement and class rules that start matching on cwd can move rows the current fixtures pin by name. New examples must cover the intended pin, and existing built-in files must not gain cwd tests that reshuffle them.

### Acceptance

- A class or session rule with `cwd_prefix` equal to a live process's cwd places or classifies that process; EACCES/empty cwd does not match.
- `--check-rules` examples may include `cwd` and `expect`.
- `--filter` / `/` behaviour is unchanged.
- `evaluation_allocates_nothing` still passes.
- Built-in `rules.d` files gain no cwd rules unless a fixture says they must.
