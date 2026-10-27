#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"latest"}"
sudo="${2:-""}"

if [[ "${tag}" == "local" ]]; then
  cargo build --release
  if [[ "${sudo}" == "sudo" ]]; then
    sudo cp -f "./target/release/xslash" "/usr/local/bin/xslash"
    sudo cp -f "./target/release/xslash-helper" "/usr/local/bin/xslash-helper"
  else
    mkdir -p "${HOME}/.local/bin"
    cp -f "./target/release/xslash" "${HOME}/.local/bin/xslash"
    cp -f "./target/release/xslash-helper" "${HOME}/.local/bin/xslash-helper"
  fi
  exit 0
fi

host_kernel="$(uname -s | tr '[:upper:]' '[:lower:]')"
host_arch="$(uname -m | tr '[:upper:]' '[:lower:]')"

case "${host_arch}" in
x86_64 | amd64) host_arch="amd64" ;;
aarch64 | arm64) host_arch="arm64" ;;
esac

if command -v curl >/dev/null 2>&1; then
  download() { curl -f -o "$2" -L "$1"; }
  request() { curl -f -s -S -L "$1"; }
elif command -v wget >/dev/null 2>&1; then
  download() { wget -q -O "$2" "$1"; }
  request() { wget -q -O - "$1"; }
else
  printf "error: need curl or wget to download xslash\n" >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  check_sha256() { [[ "$(sha256sum "$1" | awk '{print $1}')" == "$2" ]]; }
elif command -v shasum >/dev/null 2>&1; then
  check_sha256() { [[ "$(shasum -a 256 "$1" | awk '{print $1}')" == "$2" ]]; }
else
  printf "error: need sha256sum or shasum to verify the download\n" >&2
  exit 1
fi

if [[ "${tag}" == "latest" ]]; then
  download_url="https://github.com/ndscm/xslash/releases/latest/download"
  api_url="https://api.github.com/repos/ndscm/xslash/releases/latest"
else
  download_url="https://github.com/ndscm/xslash/releases/download/${tag}"
  api_url="https://api.github.com/repos/ndscm/xslash/releases/tags/${tag}"
fi

expected_sha256="$(request "${api_url}" |
  jq -r --arg n "xslash-${host_kernel}-${host_arch}" '.assets[] | select(.name == $n) | .digest' |
  sed -n 's/^sha256://p')"

if [[ ! "${expected_sha256}" =~ ^[a-f0-9]{64}$ ]]; then
  printf "error: could not find a sha256 digest\n" >&2
  exit 1
fi

download "${download_url}/xslash-${host_kernel}-${host_arch}" "./xslash-${host_kernel}-${host_arch}"

if ! check_sha256 "./xslash-${host_kernel}-${host_arch}" "${expected_sha256}"; then
  printf "error: checksum verification failed for %s (expected %s)\n" \
    "xslash-${host_kernel}-${host_arch}" "${expected_sha256}" >&2
  rm -f "./xslash-${host_kernel}-${host_arch}"
  exit 1
fi

chmod +x "./xslash-${host_kernel}-${host_arch}"

if [[ "${sudo}" == "sudo" ]]; then
  sudo mv -f "./xslash-${host_kernel}-${host_arch}" "/usr/local/bin/xslash"
else
  mkdir -p "${HOME}/.local/bin"
  mv -f "./xslash-${host_kernel}-${host_arch}" "${HOME}/.local/bin/xslash"
fi
