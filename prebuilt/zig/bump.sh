#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"

if [[ "${1:-}" == "latest" ]]; then
  index="$(curl -s "https://ziglang.org/download/index.json")"
  tag="$(jq -r '[keys[] | select(. != "master")]
    | map(split(".") | map(tonumber)) | max | map(tostring) | join(".")' <<<"${index}")"
  printf "%s\n" "${tag}" >./TAG
fi

xslash -- create --skeleton "./zig.xslash.json" --replace "{{TAG}}=${tag}" --final=true --out "./bin/zig.xslash"

ln -s -f zig.xslash ./bin/zig
