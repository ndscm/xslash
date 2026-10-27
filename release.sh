#!/usr/bin/env bash
set -euxo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

release_tag="${1:-""}"
commit="${2:-"origin/main"}"

if [[ -z "${release_tag}" ]]; then
  release_tag=$(date -u "+v%y.%m.%d" | sed 's/\.0*/./g; s/v0*/v/')
fi

if [[ ! "${release_tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-.*)?$ ]]; then
  printf "error: tag '%s' must be vX.Y.Z with an optional -suffix\n" "${release_tag}" >&2
  exit 1
fi

git fetch --all --tags --prune --prune-tags

if [[ -z "$(git tag --list "${release_tag}")" ]]; then
  git tag "${release_tag}" "${commit}"
  git push origin "${release_tag}"
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

version="${release_tag#v}"
sed -i "s/^version = \"0.0.0\"/version = \"${version}\"/" Cargo.toml
if ! grep -qF "version = \"${version}\"" Cargo.toml; then
  printf "error: failed to set package version to '%s' in Cargo.toml\n" "${version}" >&2
  exit 1
fi

# Pin the macOS deployment target so the darwin binaries claim a stable minimum.
export MACOSX_DEPLOYMENT_TARGET="11.0"

# The .2.17 suffix on the linux targets pins glibc so the binaries run on older
# distros instead of requiring the build host's glibc.
platforms=(
  "darwin amd64 x86_64-apple-darwin"
  "darwin arm64 aarch64-apple-darwin"
  "linux amd64 x86_64-unknown-linux-gnu.2.17"
  "linux arm64 aarch64-unknown-linux-gnu.2.17"
)

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

cargo publish --allow-dirty
