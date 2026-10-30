#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"

if [[ "${1:-}" == "latest" ]]; then
  latest="$(curl -s "https://api.github.com/repos/ndscm/ndscm/releases/latest")"
  tag="$(jq -r '.tag_name' <<<"${latest}")"
  printf "%s\n" "${tag}" >./TAG
fi

xslash -- create --skeleton "./ndscm.xslash.json" --replace "{{TAG}}=${tag}" --final --out "./bin/ndscm.xslash" --symlink nd --symlink ndscm
