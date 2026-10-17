# AUR packaging

This directory is the source of truth for `heft` (built from the release tag),
`heft-bin` (release musl binaries, no `depends`) and `heft-git` (main). The AUR
repositories are push targets, never edited in place.

`LICENSE` here is 0BSD and covers the packaging sources only, not heft: the
AUR submission guidelines require a package source licence in each AUR
repository, and one that is not 0BSD makes the package ineligible for
promotion to the official repositories. Every push copies it in beside
`PKGBUILD` and `.SRCINFO`.

The AUR account these packages live in also maintains packages that are not
heft's. `release.sh` pushes `heft` and `heft-bin` by name and nothing else;
keep it that way.

## Release

Move the changes into `## Unreleased` in `CHANGELOG.md`, then:

```sh
packaging/aur/release.sh tag 0.12.3       # local only
packaging/aur/release.sh publish 0.12.3   # pushes
```

`tag` dates the changelog section, bumps `Cargo.toml` and `Cargo.lock`,
commits, tags, and points `heft` at the tag in a second commit. That checksum
cannot be in the tagged commit: makepkg checksums a git source over the tagged
tree, which contains the PKGBUILD. It is taken from the local tag and matches a
GitHub clone.

`publish` pushes the tag, pushes `heft` to the AUR, waits for the release run,
fills `heft-bin` from the release's `.sha256` files, commits that, pushes main,
then pushes `heft-bin`. Re-run it after a failed release run: each step is a
no-op once done.

Both need Docker (makepkg runs in `archlinux:base-devel`), `gh`, and an SSH key
the AUR account accepts. No AUR key is kept in GitHub.

`heft-git` is pushed by hand on its own cadence: a tag changes nothing in a
package whose `pkgver()` is `git describe`. Regenerate its `.SRCINFO` with
`makepkg --printsrcinfo` before pushing.

Check the result with a fresh clone, not the RPC or cgit, which lag by minutes.
