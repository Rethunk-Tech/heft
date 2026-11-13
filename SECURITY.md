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

## Release signing

Releases from the first one after this section landed carry `SHA256SUMS.asc`, a detached signature over `SHA256SUMS` made with the Rethunk.Tech release signing key. Earlier releases are unsigned.

- Fingerprint: `827D 0553 6BF3 3C8F 634E  DBE3 E60A 9B0F 6E3F 276E`
- Verify: `gpg --import rethunk-release-signing.pub.asc && gpg --verify SHA256SUMS.asc SHA256SUMS`

```
-----BEGIN PGP PUBLIC KEY BLOCK-----

mDMEavZr2xYJKwYBBAHaRw8BAQdAuTAn1oKhKC13SOyJMlYmaoGGQtVRyZ5yEpxT
yZBxH/q0NFJldGh1bmsuVGVjaCByZWxlYXNlIHNpZ25pbmcgPHJlbGVhc2VzQHJl
dGh1bmsudGVjaD6ImQQTFgoAQRYhBIJ9BVNr8zyPY07b4+YKmw9uPyduBQJq9mvb
AhsDBQkDwmcABQsJCAcCAiICBhUKCQgLAgQWAgMBAh4HAheAAAoJEOYKmw9uPydu
7WIA+wYeW352nuKph1uJ+XJPPpgmkPyYujZWiRBZIUUA5hfwAQDY6ru4vNL8TfQ5
5rddKHoSKh1nMIdiLWn9ukkyWW+MCg==
=meeO
-----END PGP PUBLIC KEY BLOCK-----
```
