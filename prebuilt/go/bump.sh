#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"

if [[ "${1:-}" == "latest" ]]; then
  index="$(curl -s "https://go.dev/dl/?mode=json")"
  tag="$(jq -r 'first(.[] | select(.stable) | .version | ltrimstr("go"))' <<<"${index}")"
  printf "%s\n" "${tag}" >./TAG
fi

xslash -- create --skeleton "./go.xslash.json" --replace "{{TAG}}=${tag}" --final --out "./bin/go.xslash" --symlink go
xslash -- create --skeleton "./gofmt.xslash.json" --replace "{{TAG}}=${tag}" --final --out "./bin/gofmt.xslash" --symlink gofmt
