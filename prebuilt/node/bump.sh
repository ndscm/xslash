#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"

if [[ "${1:-}" == "latest" ]]; then
  index="$(curl -s "https://nodejs.org/dist/index.json")"
  tag="$(jq -r ".[0].version" <<<"${index}")"
  printf "%s\n" "${tag}" >./TAG
elif [[ "${1:-}" == "lts" ]]; then
  index="$(curl -s "https://nodejs.org/dist/index.json")"
  tag="$(jq -r 'first(.[] | select(.lts != false) | .version)' <<<"${index}")"
  printf "%s\n" "${tag}" >./TAG
fi

xslash -- create --skeleton "./node.xslash.json" --replace "{{TAG}}=${tag}" --final --out "./bin/node.xslash" --symlink node
xslash -- create --skeleton "./npm.xslash.json" --replace "{{TAG}}=${tag}" --final --out "./bin/npm.xslash" --symlink npm
xslash -- create --skeleton "./npx.xslash.json" --replace "{{TAG}}=${tag}" --final --out "./bin/npx.xslash" --symlink npx
