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

## AUR release without the commit-back round trip

A release currently waits on CI twice before the AUR sees it: the `aur` job checksums the published assets, commits them back to main, and the maintainer pulls and pushes by hand. The AUR copy already lags (0.12.1 against 0.12.2). Decided shape (NOMAD, 2026-10-17):

- `heft` takes its source from the tag, `git+$url.git#tag=v$pkgver`, with a real sha256. makepkg (pacman 7.1) checksums a git source over the tree, so the sum is identical from a local clone and from GitHub (both `e0c2568d…` for v0.12.2, measured) and `makepkg --verifysource` passes against it. Arch's own `pacman` package uses the same form. The release commit carries the final PKGBUILD and `.SRCINFO`.
- `packaging/aur/release.sh <version>`: bump `Cargo.toml`, render `heft` in `archlinux:base-devel` (`makepkg -g`, `--printsrcinfo`), commit, tag, one approved `git push --atomic origin main v<version>`, push `heft` to the AUR from the local key, then `gh run watch` the release run, render `heft-bin` from the published `.sha256` files and push it from the local key. No AUR key goes into GitHub.
- The `aur` job is deleted: no checkout of main, no commit-back, no `contents: write`.
- `heft-git` is unchanged.

### Read

- `.github/workflows/release.yml:158` onward: the `aur` job to delete.
- `packaging/aur/heft/PKGBUILD:15-16`: archive-tarball source to replace; `prepare`/`build`/`package` `cd "$pkgname-$pkgver"` becomes `cd "$pkgname"`.
- `packaging/aur/update.sh`: the `.sha256` reading for `heft-bin` moves into `release.sh`; the archive re-hash goes.
- `packaging/aur/README.md:22-40`: release procedure and the unexplained "deliberately unset" line.
- `AGENTS.md:208-213`: points at that README.

### Traps

- The AUR account belongs to gissf1, who cleared NOMAD for `heft*` and `engined*` only. `release.sh` pushes only those package bases, by explicit name; it never loops over `list-repos`.
- Tag before checksumming: the sum is taken from the local tag, and a moved tag changes it.
- `.SRCINFO` drift is what `update.sh` existed to stop. A CI step filtered to `packaging/aur/**` checks `.SRCINFO` against `makepkg --printsrcinfo` and `pkgver` against the crate version.
- The AUR guidelines prefer a dedicated AUR key over a reused one; the local push currently uses `~/.ssh/id_ed25519`.

### Acceptance

- One release is one GitHub push and no pull; `heft` is on the AUR before the release run finishes, `heft-bin` right after it.
- A fresh clone of each AUR repo shows the new `pkgver`, a passing `makepkg --verifysource`, and the 0BSD `LICENSE`.
- No workflow commits to main.
