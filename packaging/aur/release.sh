#!/usr/bin/env bash
# Release heft and publish it to the AUR from this machine.
#
#   release.sh tag <version>      bump, commit, tag, and point `heft` at the tag
#   release.sh publish <version>  push the tag, publish `heft`, wait for the
#                                 release run, publish `heft-bin`, push main
#
# `publish` is safe to re-run after a failed release run: every step it repeats
# is a no-op once done.
#
# The `heft` checksum cannot live in the tagged commit: makepkg checksums a git
# source over the tagged tree, and the PKGBUILD is part of that tree. So the
# sum is a commit on top of the tag. It is taken from the local tag, which gives
# the same sum as a GitHub clone.
#
# The AUR account is shared with packages that are not ours, so the AUR push
# names its two package bases and never iterates over the account's repos.
set -euo pipefail

cmd=${1:-}
version=${2:-}
version=${version#v}
[[ $cmd =~ ^(tag|publish)$ && $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "usage: ${0##*/} tag|publish <version>" >&2
  exit 2
}
tag="v$version"
repo=Rethunk-Tech/heft
root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
aur="$root/packaging/aur"
cd "$root"

die() {
  echo "${0##*/}: $*" >&2
  exit 1
}

# A sum that did not arrive as 64 hex characters is an error page or a
# truncated read, and writing it into a PKGBUILD hides that until a user builds.
hex64() {
  [[ $1 =~ ^[0-9a-f]{64}$ ]] || die "not a sha256: $1"
  printf '%s\n' "$1"
}

# makepkg is Arch-only. The package directory is copied in rather than mounted
# writable, so nothing in the tree ends up owned by the container's user.
in_arch() {
  local pkg=$1 script=$2
  docker run --rm -i -v "$root:/repo:ro" archlinux:base-devel bash -c '
    set -e
    pacman -Sy --noconfirm --needed --quiet git >/dev/null 2>&1
    useradd -m b
    git config --system --add safe.directory "*"
    cp -r "/repo/packaging/aur/$1" /tmp/p
    chown -R b /tmp/p
    cd /tmp/p
    su b -c "$2"
  ' _ "$pkg" "$script"
}

srcinfo() { in_arch "$1" 'makepkg --printsrcinfo' >"$aur/$1/.SRCINFO"; }

set_sum() { sed -i "s|^$1=('[^']*')|$1=('$2')|" "$3"; }

set_pkgver() { sed -i "s/^pkgver=.*/pkgver=$version/; s/^pkgrel=.*/pkgrel=1/" "$aur/$1/PKGBUILD"; }

aur_push() {
  local pkg=$1 work
  work=$(mktemp -d)
  git clone -q "ssh://aur@aur.archlinux.org/$pkg.git" "$work/$pkg"
  cp "$aur/LICENSE" "$aur/$pkg/PKGBUILD" "$aur/$pkg/.SRCINFO" "$work/$pkg/"
  git -C "$work/$pkg" add PKGBUILD .SRCINFO LICENSE
  if git -C "$work/$pkg" diff --cached --quiet; then
    echo "AUR $pkg already at $version"
  else
    git -C "$work/$pkg" commit -q -m "heft $version"
    # The AUR accepts pushes to master only.
    git -C "$work/$pkg" push -q origin master
    echo "AUR $pkg pushed $version"
  fi
  rm -rf -- "$work"
}

tag_release() {
  [[ $(git rev-parse --abbrev-ref HEAD) == main ]] || die "not on main"
  git diff --quiet HEAD || die "uncommitted changes"
  git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "$tag already exists"
  grep -q '^## Unreleased$' CHANGELOG.md || die "CHANGELOG.md has no '## Unreleased' section"

  sed -i "s/^## Unreleased$/## $version - $(date -u +%F)/" CHANGELOG.md
  sed -i "0,/^version = .*/s//version = \"$version\"/" Cargo.toml
  cargo update --offline --workspace --quiet
  git add CHANGELOG.md Cargo.toml Cargo.lock
  git commit -q -m "chore(release): heft $version"
  git tag -a "$tag" -m "heft $version"

  set_pkgver heft
  # A later source= wins, so the sum comes from the local tag, before it is pushed.
  local sum
  sum=$(in_arch heft "echo 'source=(\"heft::git+file:///repo#tag=$tag\")' >>PKGBUILD && makepkg -g" |
    sed -n "s/^sha256sums=('\([^']*\)')$/\1/p")
  set_sum sha256sums "$(hex64 "$sum")" "$aur/heft/PKGBUILD"
  srcinfo heft
  git add "$aur/heft"
  git commit -q -m "chore(aur): heft $version"
  echo "tagged $tag; next: ${0##*/} publish $version"
}

publish() {
  git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "no tag $tag; run: ${0##*/} tag $version"
  grep -q "^pkgver=$version$" "$aur/heft/PKGBUILD" || die "packaging/aur/heft is not at $version"

  git push -q origin "refs/tags/$tag"
  aur_push heft

  local sha run=""
  sha=$(git rev-parse "$tag^{commit}")
  for _ in $(seq 30); do
    run=$(gh run list -R "$repo" --workflow release.yml --commit "$sha" --limit 1 --json databaseId --jq '.[0].databaseId // empty')
    [[ -n $run ]] && break
    sleep 10
  done
  [[ -n $run ]] || die "no release run for $sha after 5 minutes"
  gh run watch -R "$repo" "$run" --exit-status >/dev/null || die "release run $run failed"

  # The binary sums come from the .sha256 files the release publishes rather
  # than from `makepkg -g`, which hashes only the running architecture's
  # sources and would leave the aarch64 array stale and looking correct.
  local rel="https://github.com/$repo/releases/download/$tag"
  published() { hex64 "$(curl -fsSL "$rel/$1.sha256" | cut -d' ' -f1)"; }
  set_pkgver heft-bin
  set_sum sha256sums "$(published heft-completions-man.tar.gz)" "$aur/heft-bin/PKGBUILD"
  set_sum sha256sums_x86_64 "$(published heft-x86_64-unknown-linux-musl)" "$aur/heft-bin/PKGBUILD"
  set_sum sha256sums_aarch64 "$(published heft-aarch64-unknown-linux-musl)" "$aur/heft-bin/PKGBUILD"
  srcinfo heft-bin
  git add "$aur/heft-bin"
  git diff --cached --quiet || git commit -q -m "chore(aur): heft-bin $version"

  echo "pushing main:"
  git log --oneline '@{u}..HEAD'
  git push -q origin main
  aur_push heft-bin
}

case $cmd in
  tag) tag_release ;;
  publish) publish ;;
esac
