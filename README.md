<h1 align="center">heft</h1>

<div align="center">

<img src="https://img.shields.io/badge/os-linux-blue" alt="Linux" />
<img src="https://img.shields.io/badge/lang-Rust-dea584?logo=rust&logoColor=000" alt="Rust" />
<img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="Apache 2.0" />

</div>

---

Linux process tree from `/proc` and DRM fdinfo: **Host → User (Applications,
User Services, Containers) → System**. Observe-only; blank metrics on
`EACCES`. Docker/Podman workloads never bill to `dockerd`.

```sh
cargo run --release -- --once
```

## Installation

```sh
brew install rethunk-tech/tap/heft
```

```sh
cargo install --git https://github.com/Rethunk-Tech/heft
```

Musl binaries: [latest release](https://github.com/Rethunk-Tech/heft/releases/latest).
Flags: `heft --help`. Install, keys, XDG, rules: [HUMANS.md](HUMANS.md).

| Doc | Purpose |
| --- | --- |
| [HUMANS.md](HUMANS.md) | Install, run, keys, XDG, verification |
| [AGENTS.md](AGENTS.md) | Layout, grouping invariants, gates |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Commits, tests, review bar |
| [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) | Expected conduct |
| [SECURITY.md](SECURITY.md) | Observe-only boundary and disclosure |
| [CHANGELOG.md](CHANGELOG.md) | Release notes |

Apache-2.0. See [LICENSE](LICENSE).
