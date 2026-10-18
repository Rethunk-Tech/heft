# AUR packaging

Source of truth for `heft` (release tag), `heft-bin` (release musl, no
`depends`) and `heft-git` (main). AUR clones are push targets, never edited
in place.

`LICENSE` here is 0BSD and covers the packaging sources only. The AUR
guidelines require a package-source licence; anything other than 0BSD makes
the package ineligible for promotion to the official repositories. Every
push copies it beside `PKGBUILD` and `.SRCINFO`.

The AUR account also maintains packages that are not heft's. `release.sh`
pushes `heft` and `heft-bin` by name and nothing else.

## Release

Move changes into `## Unreleased` in `CHANGELOG.md`, then:

```sh
packaging/aur/release.sh tag 0.12.3       # local only
packaging/aur/release.sh publish 0.12.3   # pushes
```

`tag` dates the changelog, bumps `Cargo.toml` / `Cargo.lock`, commits, tags,
and points `heft` at the tag in a second commit. That checksum cannot live
in the tagged commit: makepkg checksums a git source over the tagged tree,
which contains the PKGBUILD. It is taken from the local tag and matches a
GitHub clone.

`publish` pushes the tag, pushes `heft`, waits for the release run, fills
`heft-bin` from the release `.sha256` files, commits that, pushes main, then
pushes `heft-bin`. Re-run after a failed release run: each step is a no-op
once done.

Needs Docker (`archlinux:base-devel`), `gh`, and an SSH key the AUR account
accepts. No AUR key is kept in GitHub.

`heft-git` is pushed by hand: a tag changes nothing in a package whose
`pkgver()` is `git describe`. Regenerate `.SRCINFO` with
`makepkg --printsrcinfo` before pushing.

Check with a fresh clone, not the RPC or cgit (they lag).
