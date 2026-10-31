#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"

if [[ "${1:-}" == "latest" ]]; then
  latest="$(curl -s "https://registry.npmjs.org/prettier/latest")"
  tag="$(jq -r '.version' <<<"${latest}")"
  printf "%s\n" "${tag}" >./TAG
fi

xslash -- create --skeleton "./prettier.xslash.json" --replace "{{TAG}}=${tag}" --final --out "./bin/prettier.xslash" --symlink prettier
