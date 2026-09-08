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
when Rust or Cargo.toml is staged. Pre-push runs `cargo test --locked`, `cargo deny --locked check`,
and `cargo machete`.

## Gates

Run before a commit that touches code:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo deny --locked check
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
entry, no docker socket, no DRM, and a `/proc` with one process in it.

`heft --once` on this machine is the product check, not a CI job.

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
writes, or mutating Docker/Podman calls. NVIDIA and cgroup v1 are out of v1.

Per-process network I/O is not a missing feature, it is unavailable: measured,
`/proc/<pid>/net/dev` is per network namespace and byte-identical across
unrelated pids, socket `fdinfo` carries no byte counter, `/proc/net/tcp`
queues are depths rather than totals, `rchar`/`wchar` miss `send`/`recv`, and
cgroup v2 has no network controller. Every remaining route needs CAP_NET_RAW,
CAP_BPF or ptrace. Containers get NETNS RX/TX because a container owns a
namespace; nothing else does.
