# Contributing

Issues and pull requests are welcome. By contributing you agree your work is
licensed under [Apache-2.0](LICENSE) and to the
[Code of Conduct](CODE_OF_CONDUCT.md).

Open an issue before a large change. Grouping invariants:
[AGENTS.md](AGENTS.md). Runbook: [HUMANS.md](HUMANS.md).

## Setup

Stable Rust. MSRV is `rust-version` in [Cargo.toml](Cargo.toml); CI checks it.
`rust-toolchain.toml` outranks a `rustup override`, so name the toolchain on
the cargo invocation when you need MSRV.

```sh
lefthook install     # per clone; hooks are not committed by git
cargo binstall cargo-deny cargo-machete   # pre-push; do not sudo
```

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

`gate` from the repository root (same as pre-push / `.gate.toml`). The suite
must stay under 30 seconds. Do not add live-GPU or live-docker requirements
to `cargo test`; those walks belong in fixtures.

Clippy is the default level plus `[lints.clippy]` in `Cargo.toml`. `pedantic`,
`nursery` and `cargo` stay off. Cast convention: [AGENTS.md](AGENTS.md#gates).

CI also runs `cargo +1.98 check --locked --all-targets` (the MSRV) and
`tests/live_proc.rs` as a static musl binary in an image with nothing
installed — no `/etc/passwd`, no docker socket, no DRM, one process in
`/proc` — with the musl `heft` bind-mounted at `CARGO_BIN_EXE_heft`.
`heft --once` on this machine is the product check, not a CI job.

## Commits

Conventional: `type(scope): subject`. Imperative, ≤72 characters. Body
explains why. One logical unit. No AI trailers. Stage explicit paths only.

## Changelog

[CHANGELOG.md](CHANGELOG.md) is Keep a Changelog. Behaviour, flags, and
output-shape changes belong under Unreleased in the same commit. Refactors,
tests, and docs do not.

## Tests

Least tests, most coverage. Fixture-first for `/proc` and cgroup parsing.
No snapshot libraries. A new test must hit a branch nothing else hits.

## Scope

Observe-only. Do not add kill, nice, ptrace, `/proc` writes, or mutating
Docker/Podman calls. Do not guess another GPU driver's fdinfo keys; the
supported set is in [HUMANS.md](HUMANS.md#what-the-tree-means).

Per-process network I/O is unavailable: `/proc/<pid>/net/dev` is per netns
and byte-identical across unrelated pids, socket `fdinfo` has no byte
counter, `/proc/net/tcp` queues are depths, `rchar`/`wchar` miss
`send`/`recv`, and cgroup v2 has no network controller. Remaining routes
need CAP_NET_RAW, CAP_BPF or ptrace.
