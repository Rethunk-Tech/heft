<h1 align="center">heft</h1>

<div align="center">

Read-only Linux TUI that groups process cost by application, user service, and container.

<br />

<img src="https://img.shields.io/badge/os-linux-blue" alt="Linux" />
<img src="https://img.shields.io/badge/lang-Rust-dea584?logo=rust&logoColor=000" alt="Rust" />
<img src="https://img.shields.io/badge/repo-private-lightgrey" alt="Private repository" />

</div>

---

`heft` walks `/proc` and amdgpu fdinfo, then draws a tree: **Host → each User
(Applications, User Services, Containers) → System**. Launchers such as `bunx`
and `bwrap` bill to the payload. Docker and Podman workloads are never billed
to `dockerd`, `containerd`, or the starter daemon.

## Quick start

```sh
cargo run --release -- --once
```

See [HUMANS.md](HUMANS.md) for install, keybindings, and how a saved view is
written.

## Highlights

- **Application weight** — a terminal, an idle shell, and `claude` are sibling
  identities; workers and launchers fold into the app they serve.
- **Containers as workloads** — `docker-*.scope` never lands in System;
  compose/Supabase projects sum; `engined-*` stay separate rows.
- **No root** — every visible PID is listed; `EACCES` blanks that metric and
  keeps the row. Heft does not kill, nice, or write `/proc`.
- **TUI plus scripts** — fullscreen table by default; `--once` and `--json`
  for one sample.

## Documentation

| Doc | Purpose |
| --- | --- |
| [HUMANS.md](HUMANS.md) | Install, run, keys, XDG paths, verification |
| [AGENTS.md](AGENTS.md) | Layout, grouping invariants, gates |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Commits, tests, review bar |
| [SECURITY.md](SECURITY.md) | Observe-only boundary and disclosure |
| [CHANGELOG.md](CHANGELOG.md) | Release notes |

## License

Private repository — all rights reserved. See [LICENSE](LICENSE).
