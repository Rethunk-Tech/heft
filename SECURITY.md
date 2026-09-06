# Security

## Supported versions

The latest tagged release. There are no long-lived release branches.

## Reporting a vulnerability

Use GitHub private vulnerability reporting (Security → Report a vulnerability)
rather than opening a public issue. If that is unavailable to you, email
<security@rethunk.tech>.

## What is in scope

heft is observe-only toward the OS. Interesting failures are ones that break
that boundary or leak data heft should not need:

- Any write to `/proc`, sysfs, cgroup files, or another process (kill, nice,
  ptrace, mem writes).
- Any Docker/Podman method other than GET (create, start, stop, kill, exec).
- Privilege escalation or a requirement to run as root to list the caller's
  own processes.
- A crafted `/proc` or unix-socket response that makes heft write outside its
  XDG directories or the TTY.

## What is not

- **Blank metrics on `EACCES`.** Other users' `smaps_rollup` / `io` / fdinfo /
  `exe` are expected to be unreadable without root; that is a product rule,
  not a vulnerability.
- **Header totals exceeding the Host row.** The header reads machine-wide
  files; the tree sums visible PIDs only.
- **Docker group access.** Talking to `/var/run/docker.sock` as a user in
  `docker` is the same trust the docker CLI already has; heft only GETs.
- **XDG files you asked it to write.** Saving a view with `s` is the
  documented config write.
