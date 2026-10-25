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

xslash -- create --skeleton "./node.xslash.json" --replace "{{TAG}}=${tag}" --final true --out "./bin/node.xslash"
xslash -- create --skeleton "./npm.xslash.json" --replace "{{TAG}}=${tag}" --final true --out "./bin/npm.xslash"
xslash -- create --skeleton "./npx.xslash.json" --replace "{{TAG}}=${tag}" --final true --out "./bin/npx.xslash"

ln -s -f node.xslash ./bin/node
ln -s -f npm.xslash ./bin/npm
ln -s -f npx.xslash ./bin/npx
