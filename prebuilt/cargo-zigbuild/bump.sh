#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"

if [[ "${1:-}" == "latest" ]]; then
  latest="$(curl -s "https://api.github.com/repos/rust-cross/cargo-zigbuild/releases/latest")"
  tag="$(jq -r ".tag_name" <<<"${latest}")"
  printf "%s\n" "${tag}" >./TAG
fi

xslash -- create --skeleton "./cargo-zigbuild.xslash.json" --replace "{{TAG}}=${tag}" --final=true --out "./bin/cargo-zigbuild.xslash"

ln -s -f cargo-zigbuild.xslash ./bin/cargo-zigbuild
