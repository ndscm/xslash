#!/usr/bin/env bash
set -euxo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

release_tag="${1:-""}"

git fetch --all --tags --prune --prune-tags

if [[ -z "${release_tag}" ]]; then
  release_major="$(date -u "+%y" | sed 's/^0*//')"
  release_minor="$(date -u "+%m" | sed 's/^0*//')"
  release_patch="$(date -u "+%d" | sed 's/^0*//')"
  release_tag="v${release_major}.${release_minor}.${release_patch}"
  while git rev-parse --verify --quiet "refs/tags/${release_tag}" >/dev/null; do
    release_patch="$((release_patch + 1))"
    release_tag="v${release_major}.${release_minor}.${release_patch}"
  done
fi

if [[ ! "${release_tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9a-z.]+)?$ ]]; then
  printf "error: tag '%s' must be vX.Y.Z with an optional alphanumeric -suffix\n" "${release_tag}" >&2
  exit 1
fi

if git rev-parse --verify --quiet "refs/tags/${release_tag}" >/dev/null; then
  printf "error: tag '%s' already exists\n" "${release_tag}" >&2
  exit 1
fi

if [[ -n "$(git status --porcelain)" ]]; then
  printf "error: git working directory is dirty\n" >&2
  exit 1
fi

version="${release_tag#v}"
sed -i "s/^version = \"0.0.0\"/version = \"${version}\"/" Cargo.toml
if ! grep -qF "version = \"${version}\"" Cargo.toml; then
  printf "error: failed to set package version to '%s' in Cargo.toml\n" "${version}" >&2
  exit 1
fi
cargo build

git add Cargo.toml
git add Cargo.lock
git commit -m "xslash: release ${release_tag}"

sed -i "s/^version = \"${version}\"/version = \"0.0.0\"/" Cargo.toml
if ! grep -qF "version = \"0.0.0\"" Cargo.toml; then
  printf "error: failed to reset package version to '0.0.0' in Cargo.toml\n" >&2
  exit 1
fi
cargo build

git add Cargo.toml
git add Cargo.lock
git commit -m "xslash: reset version"
