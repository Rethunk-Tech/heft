# Security

## Supported versions

The latest tagged release. There are no long-lived release branches.

## Reporting a vulnerability

GitHub private vulnerability reporting (Security → Report a vulnerability).
If that is unavailable, email <security@rethunk.tech>. Do not open a public
issue.

## In scope

Failures that break observe-only or leak data heft should not need:

- Any write to `/proc`, sysfs, cgroup files, or another process (kill, nice,
  ptrace, mem writes).
- Any Docker/Podman method other than GET.
- Privilege escalation, or a requirement to run as root to list the caller's
  own processes.
- A crafted `/proc` or unix-socket response that makes heft write outside its
  XDG directories or the TTY.

## Out of scope

- Blank metrics on `EACCES` (other users' `smaps_rollup` / `io` / fdinfo /
  `exe`).
- Header totals exceeding the Host row (header is machine-wide; the tree
  sums visible PIDs).
- Docker group access to `/var/run/docker.sock` (same trust as the docker
  CLI; heft only GETs).
- XDG files written because you asked (`s` saves the view).
