<h1 align="center">heft</h1>

<div align="center">

<img src="https://img.shields.io/badge/os-linux-blue" alt="Linux" />
<img src="https://img.shields.io/badge/lang-Rust-dea584?logo=rust&logoColor=000" alt="Rust" />
<img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="Apache 2.0" />

</div>

---

`heft` walks `/proc` and DRM fdinfo (amdgpu, i915, xe), then draws a tree:
**Host → each User (Applications, User Services, Containers) → System**.
Docker and Podman workloads are never billed to `dockerd`, `containerd`, or
the starter daemon.

## Quick start

Grab a binary from the
[latest release](https://github.com/Rethunk-Tech/heft/releases/latest) — the
musl build is static and needs no toolchain — or build it:

```sh
cargo run --release -- --once
```

See [HUMANS.md](HUMANS.md) for install, keybindings, and how a saved view is
written.

## Highlights

- **Application weight** — a terminal, an idle shell, and `claude` are sibling
  identities; workers and launchers fold into the app they serve.
- **Containers as workloads** — `docker-*.scope` never lands in System or on
  `dockerd`.
- **No root** — every visible PID is listed; `EACCES` blanks that metric and
  keeps the row. Heft does not kill, nice, or write `/proc`.
- **One row, every dimension** — CPU, PSS/RSS/swap, disk, GPU memory and
  engine percent, per-container network, threads and age. Hide the columns you
  do not want; a metric heft cannot read stays blank rather than reading zero.
- **TUI plus scripts** — fullscreen table by default; `--once` and `--json`
  for one sample, with `--sort` and `--filter` for the shape you want.

## Documentation

| Doc | Purpose |
| --- | --- |
| [HUMANS.md](HUMANS.md) | Install, run, keys, XDG paths, verification |
| [AGENTS.md](AGENTS.md) | Layout, grouping invariants, gates |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Commits, tests, review bar |
| [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) | Expected conduct and enforcement |
| [SECURITY.md](SECURITY.md) | Observe-only boundary and disclosure |
| [CHANGELOG.md](CHANGELOG.md) | Release notes |

## License

Apache-2.0. See [LICENSE](LICENSE).
