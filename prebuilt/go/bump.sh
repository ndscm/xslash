#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"

if [[ "${1:-}" == "latest" ]]; then
  index="$(curl -s "https://go.dev/dl/?mode=json")"
  tag="$(jq -r 'first(.[] | select(.stable) | .version | ltrimstr("go"))' <<<"${index}")"
  printf "%s\n" "${tag}" >./TAG
fi

replacements=(--replace "{{TAG}}=${tag}")

xslash -- create --skeleton "./go.xslash.json" "${replacements[@]}" --final true --out "./bin/go.xslash"
xslash -- create --skeleton "./gofmt.xslash.json" "${replacements[@]}" --final true --out "./bin/gofmt.xslash"

ln -s -f go.xslash ./bin/go
ln -s -f gofmt.xslash ./bin/gofmt
