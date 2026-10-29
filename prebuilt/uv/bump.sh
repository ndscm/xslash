#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"

if [[ "${1:-}" == "latest" ]]; then
  latest="$(curl -s "https://api.github.com/repos/astral-sh/uv/releases/latest")"
  tag="$(jq -r ".tag_name" <<<"${latest}")"
  printf "%s\n" "${tag}" >./TAG
fi

xslash -- create --skeleton "./uv.xslash.json" --replace "{{TAG}}=${tag}" --final=true --out "./bin/uv.xslash"
xslash -- create --skeleton "./uvx.xslash.json" --replace "{{TAG}}=${tag}" --final=true --out "./bin/uvx.xslash"

ln -s -f uv.xslash ./bin/uv
ln -s -f uvx.xslash ./bin/uvx
