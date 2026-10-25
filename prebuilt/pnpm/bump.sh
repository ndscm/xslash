#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"

if [[ "${1:-}" == "latest" ]]; then
  latest="$(curl -s "https://api.github.com/repos/pnpm/pnpm/releases/latest")"
  tag="$(jq -r ".tag_name" <<<"${latest}")"
  printf "%s\n" "${tag}" >./TAG
fi

xslash -- create --skeleton "./pnpm.xslash.json" --replace "{{TAG}}=${tag}" --final true --out "./bin/pnpm.xslash"

ln -s -f pnpm.xslash ./bin/pnpm
