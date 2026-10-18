# TODO

[HUMANS.md](HUMANS.md) and [AGENTS.md](AGENTS.md) describe what heft does.

## Ryzen AI NPU — measured, not worth a column

Unprivileged (uid 1000) on this Strix Halo host, 2026-10-18. PCI `0000:c3:00.1` is class `0x118000`, vendor `0x1022`, device `0x17f0`, driver `amdxdna`; `vbnv` reads `NPU Strix Halo\n`.

`open("/sys/class/accel/accel0/dev", O_RDONLY)` returned `261:0\n`. `open(".../uevent")` returned `MAJOR=261\nMINOR=0\nDEVNAME=accel/accel0\nDEVTYPE=accel_minor\n`. That sysfs node has no utilization, busy, or load attribute. `open("/sys/kernel/debug/amdxdna")` and `open("/sys/kernel/debug/accel")` returned `EACCES` (13). No `/proc/<pid>/fdinfo` contained `amdxdna`, `accel`, or `npu` keys, so the `drm-client-id` + `drm-resident-*` fallback is not billing this device under VRAM/GTT. `open("/dev/accel/accel0")` returned `ENOENT` (2) in this process's `/dev`.

No column. `mem_key` / `drm-driver` unchanged. A root debugfs read is a different program.

## NVIDIA and nouveau fdinfo — measured, not worth new keys

Unprivileged (uid 1000) on this amdgpu APU host, 2026-10-18. `/sys/module/nvidia` and `/sys/module/nouveau` are absent. `open("/dev/nvidia0")`, `open("/dev/nvidiactl")`, and `open("/proc/driver/nvidia")` each returned `ENOENT` (2). A scan of `/proc/<pid>/fdinfo` found no `drm-driver: nvidia` or `drm-driver: nouveau` line, so the resident fallback is not holding a blank NVIDIA/nouveau client either.

No region or engine names to map. `mem_key` and `drm-driver` stay amdgpu/i915/xe. Guessed keys from a kernel tree stay unread.

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
