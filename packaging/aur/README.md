# AUR packaging

This directory is the source of truth for `heft` (release tarball), `heft-bin`
(release musl binaries, no `depends`) and `heft-git` (main). The AUR
repositories are push targets, never edited in place.

`LICENSE` here is 0BSD and covers the packaging sources only, not heft: the
AUR submission guidelines require a package source licence in each AUR
repository, and one that is not 0BSD makes the package ineligible for
promotion to the official repositories. Every push copies it in beside
`PKGBUILD` and `.SRCINFO`.

## After editing a PKGBUILD

Re-run `update.sh`: a `.SRCINFO` is generated metadata, and a stale one
publishes the wrong dependencies and version to every AUR consumer while the
PKGBUILD beside it looks right. `update.sh <version>` sets `pkgver`, refreshes
the checksums and regenerates every `.SRCINFO`; its header says why the sums
come from the release's `.sha256` files. It needs makepkg, so it runs inside
`archlinux:base-devel`.

## Release

The `aur` job in `.github/workflows/release.yml` runs `update.sh` once the
release exists (the checksums are of assets that did not exist before), commits
the refresh back to main, and pushes `heft` and `heft-bin` to the AUR only when
the `AUR_SSH_KEY` secret is set. The secret is deliberately unset, so that step
skips with a notice and exits green, and the push is by hand:

```sh
git pull                                  # the job's refresh commit
git clone ssh://aur@aur.archlinux.org/heft.git /tmp/aur-heft
cp packaging/aur/LICENSE packaging/aur/heft/PKGBUILD packaging/aur/heft/.SRCINFO /tmp/aur-heft/
git -C /tmp/aur-heft add PKGBUILD .SRCINFO LICENSE
git -C /tmp/aur-heft commit -m "heft <version>"
git -C /tmp/aur-heft push origin master   # the AUR accepts pushes to master only
```

Repeat for `heft-bin`. `heft-git` is also pushed by hand, on its own cadence: a
tag changes nothing in a package whose `pkgver()` is `git describe`.

Check the result with a fresh clone, not the RPC or cgit, which lag by minutes.
