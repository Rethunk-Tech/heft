#!/usr/bin/env bash
# Point the AUR packages at a published release: pkgver, checksums, .SRCINFO.
#
# The binary sums come from the .sha256 files the release already publishes
# rather than from re-hashing a download, because `makepkg -g` hashes only the
# sources for the architecture it runs on -- so updpkgsums on an x86_64
# machine leaves the aarch64 array stale and still looking correct.
#
# Needs makepkg (Arch) for --printsrcinfo. Everything else is curl and sed.
set -euo pipefail

version=${1:-}
[[ -n $version ]] || { echo "usage: ${0##*/} <version, no leading v>" >&2; exit 2; }
version=${version#v}

cd "$(dirname "$0")"
repo="https://github.com/Rethunk-Tech/heft"
rel="$repo/releases/download/v$version"

# A sum that did not arrive as 64 hex characters is an error page or a
# truncated read, and writing it into a PKGBUILD hides that until a user
# builds.
hex64() {
  [[ $1 =~ ^[0-9a-f]{64}$ ]] || { echo "not a sha256: $1" >&2; exit 1; }
  printf '%s\n' "$1"
}
published() { hex64 "$(curl -fsSL "$rel/$1.sha256" | cut -d' ' -f1)"; }
# The array name anchors the line, so each sum is replaced in place.
set_sum() { sed -i "s|^$1=('[^']*')|$1=('$2')|" "$3"; }

src=$(hex64 "$(curl -fsSL "$repo/archive/refs/tags/v$version.tar.gz" | sha256sum | cut -d' ' -f1)")
tarball=$(published heft-completions-man.tar.gz)
x86_64=$(published heft-x86_64-unknown-linux-musl)
aarch64=$(published heft-aarch64-unknown-linux-musl)

sed -i "s/^pkgver=.*/pkgver=$version/; s/^pkgrel=.*/pkgrel=1/" heft/PKGBUILD heft-bin/PKGBUILD
set_sum sha256sums "$src" heft/PKGBUILD
set_sum sha256sums "$tarball" heft-bin/PKGBUILD
set_sum sha256sums_x86_64 "$x86_64" heft-bin/PKGBUILD
set_sum sha256sums_aarch64 "$aarch64" heft-bin/PKGBUILD

# heft-git takes its pkgver from git describe at build time, so only its
# .SRCINFO is refreshed here.
for pkg in heft heft-bin heft-git; do
  (cd "$pkg" && makepkg --printsrcinfo > .SRCINFO)
done
