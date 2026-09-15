# Contributing

Issues and pull requests are welcome. By contributing you agree your work is
licensed under [Apache-2.0](LICENSE) and to the
[Code of Conduct](CODE_OF_CONDUCT.md).

Open an issue before a large change. heft is deliberately narrow, and the
[Scope](#scope) rules below rule some ideas out entirely.

Read [AGENTS.md](AGENTS.md) first — grouping invariants live there, not here.

## Setup

[HUMANS.md](HUMANS.md) is the runbook. Toolchain: stable Rust (CI uses current
stable). MSRV is `rust-version` in [Cargo.toml](Cargo.toml), and CI checks it.

```sh
lefthook install     # per clone; hooks are not committed by git
cargo binstall cargo-deny cargo-machete   # pre-push; do not sudo
```

Pre-commit runs `cargo fmt --check` and `cargo clippy --locked --all-targets -- -D warnings`
when Rust or Cargo.toml is staged. Pre-push runs `gate`, which is every command below plus
`cargo build --locked` and actionlint.

A local build writes completions and the man page under a hashed `OUT_DIR`.
Take the newest `heft.1`: an older build leaves stale directories in `target/`.

```sh
man=$(find target/release/build -name heft.1 -printf '%T@ %p\n' | sort -rn | head -1 | cut -d' ' -f2-)
assets=$(dirname "$man")
install -Dm644 "$assets/heft.bash" ~/.local/share/bash-completion/completions/heft
install -Dm644 "$assets/_heft"     ~/.local/share/zsh/site-functions/_heft
install -Dm644 "$assets/heft.fish" ~/.config/fish/completions/heft.fish
install -Dm644 "$assets/heft.1"    ~/.local/share/man/man1/heft.1
```

## Gates

Run before a commit that touches code:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo deny --locked check bans licenses sources
cargo deny --locked check advisories
cargo machete
```

The suite must stay under 30 seconds. Do not add live-GPU or live-docker
requirements to `cargo test`; put those walks in fixtures.

Two CI jobs check what a stable toolchain on a full runner cannot. Neither is
a pre-push hook, but both are worth running by hand when you touch what they
cover:

```sh
cargo +1.98 check --locked --all-targets   # the MSRV in Cargo.toml, not stable
```

`rust-toolchain.toml` outranks a `rustup override`, so the toolchain has to be
named on the cargo invocation. The other job runs `tests/live_proc.rs` as a
static musl binary inside an image with nothing installed, because those
invariants are documented to hold in a bare container -- no `/etc/passwd`
entry, no docker socket, no DRM, and a `/proc` with one process in it. The
job bind-mounts the musl `heft` binary at `CARGO_BIN_EXE_heft`, the path the
tests already use to spawn it.

`heft --once` on this machine is the product check, not a CI job.

### Clippy beyond the gate

The gate is `cargo clippy --locked --all-targets -- -D warnings` at the default
level plus `[lints.clippy]` in `Cargo.toml`. The wider groups are measured
against that same `--all-targets` gate, counted as clippy emits them for every
target, and refused. Re-measure before quoting a number: the counts move with
every clippy release and every change to this code.

| group / lint | count | verdict |
| --- | --- | --- |
| `pedantic` + `nursery` + `cargo` | 182 warnings across 8 lints | off the gate |
| `struct_excessive_bools` on `cli::Cli` | the one `pedantic` warning | refused: the seven bools are clap flags that `--help`, the man page and the completions each list on their own line; enums change all three, and the group stays off rather than carry an `#[expect]` |
| `redundant_pub_crate` (`nursery`) | 134 | refused: a visibility style this crate keeps deliberately |
| `too_long_first_doc_paragraph`, `option_if_let_else`, `single_option_map` | the rest | refused: a mechanical rewrite buying style |
| `multiple_crate_versions` (`cargo`) | two `hashbrown` and two `syn` majors | pulled by dependencies, not this crate |
| `literal_string_with_formatting_args` | 8, the `{up}`/`{down}` KEYS placeholders | false positive: literal on purpose |
| `suboptimal_flops` | 3 test assertions wanting `mul_add` | false positive |

Nothing in the wider groups is a latent bug, which is what makes the refusal
safe rather than lucky: every lint that looked like one is a false positive.

The twelve lints denied in `[lints.clippy]` each found something real:
`needless_pass_by_ref_mut` (a `&mut self` on `psi::set_row` and
`proc::WalkPool::collect` that never mutated), `assigning_clones` (a clone
assigned over a live `String` once a frame), `format_push_string` (a temporary
formatted once a frame in `sixel::encode`), `redundant_clone`, the
machine-applicable `use_self`, `missing_const_for_fn`, `doc_markdown` and
`map_unwrap_or`, and the four cast lints, whose `#[expect]` convention is in
[AGENTS.md](AGENTS.md#gates).

## Commits

Conventional commits: `type(scope): subject`. Subject imperative, ≤72
characters. Body explains why. One logical unit. No AI trailers. Stage
explicit paths only.

## Changelog

[CHANGELOG.md](CHANGELOG.md) is Keep a Changelog, newest first. Behaviour
changes, flags, and output-shape changes belong under Unreleased in the same
commit. Refactors, tests, and docs do not.

## Tests

Least tests, most coverage. Fixture-first for `/proc` and cgroup parsing.
No snapshot libraries. A new test must hit a branch nothing else hits.

## Scope

Heft stays observe-only toward the OS. Do not add kill, nice, ptrace, `/proc`
writes, or mutating Docker/Podman calls. Do not read another GPU driver's
fdinfo keys by guessing their names; the supported set is in
[HUMANS.md](HUMANS.md#what-the-tree-means).

Per-process network I/O is not a missing feature, it is unavailable: measured,
`/proc/<pid>/net/dev` is per network namespace and byte-identical across
unrelated pids, socket `fdinfo` carries no byte counter, `/proc/net/tcp`
queues are depths rather than totals, `rchar`/`wchar` miss `send`/`recv`, and
cgroup v2 has no network controller. Every remaining route needs CAP_NET_RAW,
CAP_BPF or ptrace.
