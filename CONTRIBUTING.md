# Contributing

Read [AGENTS.md](AGENTS.md) first — grouping invariants live there, not here.

## Setup

[HUMANS.md](HUMANS.md) is the runbook. Toolchain: stable Rust (Fedora 1.98 is
fine; CI uses current stable). MSRV is `rust-version` in `Cargo.toml` (1.85,
edition 2024).

```sh
lefthook install     # per clone; hooks are not committed by git
cargo binstall cargo-deny cargo-machete   # pre-push; do not sudo
```

Pre-commit runs `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`
when Rust or Cargo.toml is staged. Pre-push runs `cargo test`, `cargo deny --locked check`,
and `cargo machete`.

## Gates

Run before a commit that touches code:

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny --locked check
cargo machete
```

The suite must stay under 30 seconds. Do not add live-GPU or live-docker
requirements to `cargo test`; put those walks in fixtures.

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
writes, or mutating Docker/Podman calls. NVIDIA, cgroup v1, and network I/O
are out of v1.
