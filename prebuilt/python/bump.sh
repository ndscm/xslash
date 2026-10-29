#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"
version="${2:-"$(cat ./VERSION)"}"

if [[ "${1:-}" == "latest" ]]; then
  latest="$(curl -s "https://api.github.com/repos/astral-sh/python-build-standalone/releases/latest")"
  tag="$(jq -r ".tag_name" <<<"${latest}")"
  asset_names="$(jq -r ".assets[].name" <<<"${latest}")"
  version="$(
    awk 'match($0, /cpython-[0-9]+\.[0-9]+\.[0-9]+[+]/) {
      v = substr($0, RSTART, RLENGTH)
      gsub(/^cpython-|[+]$/, "", v)
      print v
    }' <<<"${asset_names}" |
      sort -t. -k1,1n -k2,2n -k3,3n -u |
      tail -n1
  )"
  printf "%s\n" "${tag}" >./TAG
  printf "%s\n" "${version}" >./VERSION
fi

xslash -- create \
  --skeleton "./python3.xslash.json" \
  --replace "{{TAG}}=${tag}" \
  --replace "{{VERSION}}=${version}" \
  --final=true \
  --out "./bin/python3.xslash"

ln -s -f python3.xslash ./bin/python
ln -s -f python3.xslash ./bin/python3
