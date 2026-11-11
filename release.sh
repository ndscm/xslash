#!/usr/bin/env bash
set -euxo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

release_tag="${1:-""}"

git fetch --all --tags --prune --prune-tags

if [[ -z "${release_tag}" ]]; then
  commit=""
  version=""
  candidates=($(git rev-list origin/main -- Cargo.toml))
  for candidate in "${candidates[@]}"; do
    candidate_version="$(git show "${candidate}:Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p')"
    if [[ -n "${candidate_version}" && "${candidate_version}" != "0.0.0" ]]; then
      commit="${candidate}"
      version="${candidate_version}"
      break
    fi
  done
  if [[ -z "${commit}" ]]; then
    printf "error: no release commit found on origin/main\n" >&2
    exit 1
  fi
  release_tag="v${version}"
  if [[ -z "$(git tag --list "${release_tag}")" ]]; then
    git tag "${release_tag}" "${commit}"
    git push origin "${release_tag}"
  fi
fi

git checkout --detach "${release_tag}"
git reset --hard "${release_tag}"
git clean -fdx

# Make sure git describe always generates the same release tag.
git_tag="$(git describe --tags --always --dirty)"
if [[ "${git_tag}" != "${release_tag}" ]]; then
  printf "error: git tag '%s' does not match release tag '%s'\n" "${git_tag}" "${release_tag}" >&2
  exit 1
fi

# Pin the macOS deployment target so the darwin binaries claim a stable minimum.
export MACOSX_DEPLOYMENT_TARGET="11.0"

# The linux targets statically link musl so the binaries carry no libc
# dependency and run on any distro.
platforms=(
  "darwin amd64 x86_64-apple-darwin"
  "darwin arm64 aarch64-apple-darwin"
  "linux amd64 x86_64-unknown-linux-musl"
  "linux arm64 aarch64-unknown-linux-musl"
)

rm -r "./.cache/release" || true
mkdir -p "./.cache/release"

for platform in "${platforms[@]}"; do
  read -r kernel arch zig_target <<<"${platform}"

  # zigbuild takes the version-suffixed form; the binary lands under the plain triple.
  rust_target="${zig_target%%.*}"

  # Two-pass build: compile xslash-helper first so build.rs can hash it, then
  # compile xslash with that hash baked into its embedded helper entry.
  cargo zigbuild --release --target "${zig_target}" --bin xslash-helper

  cp -f "./target/${rust_target}/release/xslash-helper" "./.cache/release/xslash-helper-${kernel}-${arch}"

  XSLASH_HELPER_BIN="./.cache/release/xslash-helper-${kernel}-${arch}" \
    cargo zigbuild --release --target "${zig_target}" --bin xslash

  cp -f "./target/${rust_target}/release/xslash" "./.cache/release/xslash-${kernel}-${arch}"
done

# Publish the GitHub release and attach every built artifact. If the release
# already exists, leave it untouched and exit.
if gh release view "${release_tag}" >/dev/null 2>&1; then
  printf "release '%s' already exists, nothing to do\n" "${release_tag}"
  exit 0
fi

gh release create "${release_tag}" ./.cache/release/* \
  --title "${release_tag}" \
  --notes "" \
  --draft

cargo publish
gh release edit "${release_tag}" --draft=false

# Restore the previous branch if everything succeeded
git checkout -
