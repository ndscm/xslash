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
elif command -v wget >/dev/null 2>&1; then
  download() { wget -q -O "$2" "$1"; }
else
  printf "error: need curl or wget to download xslash\n" >&2
  exit 1
fi

if [[ "${tag}" == "latest" ]]; then
  download_url="https://github.com/ndscm/xslash/releases/latest/download"
else
  download_url="https://github.com/ndscm/xslash/releases/download/${tag}"
fi

download "${download_url}/xslash-${host_kernel}-${host_arch}" "./xslash-${host_kernel}-${host_arch}"

chmod +x "./xslash-${host_kernel}-${host_arch}"

if [[ "${sudo}" == "sudo" ]]; then
  sudo mv -f "./xslash-${host_kernel}-${host_arch}" "/usr/local/bin/xslash"
else
  mkdir -p "${HOME}/.local/bin"
  mv -f "./xslash-${host_kernel}-${host_arch}" "${HOME}/.local/bin/xslash"
fi
