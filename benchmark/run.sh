#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

# Force a `.` decimal separator so EPOCHREALTIME and awk agree regardless of locale.
export LC_NUMERIC=C

runs="10000"

# Run the given command `runs` times and report the per-call average (a single
# invocation is too fast for `time` to resolve).
bench() {
  local start
  local end
  start="${EPOCHREALTIME}"
  for ((i = 0; i < "${runs}"; i++)); do
    "$@" >/dev/null
  done
  end="${EPOCHREALTIME}"
  awk -v s="${start}" -v e="${end}" -v n="${runs}" \
    'BEGIN { printf "%.3f ms/call\n", (e - s) / n * 1000 }'
}

printf '# go\n'

raw="$(xslash -- extract "../prebuilt/go/bin/go.xslash")"
(
  printf '\n## raw (%sx)\n' "${runs}"
  bench "${raw}" version
)

(
  printf '\n## path (%sx)\n' "${runs}"
  export PATH="$(dirname "${raw}"):$PATH"
  hash -r
  bench go version
)

xslash -- create --skeleton "../prebuilt/go/bin/go.xslash" --platform linux-amd64 --final --out "./single/go.xslash" >/dev/null
(
  cd ./single
  printf '\n## single (%sx)\n' "${runs}"
  bench ./go.xslash version
)

xslash -- create --skeleton "../prebuilt/go/bin/go.xslash" --final --out "./final/go.xslash" >/dev/null
(
  cd ./final
  printf '\n## final (%sx)\n' "${runs}"
  bench ./go.xslash version
)

xslash -- create --skeleton "../prebuilt/go/bin/go.xslash" --out "./discover/go.xslash" >/dev/null
printf '{"go":"go.xslash"}\n' >./discover/xslash.json
(
  cd ./discover
  printf '\n## discover (%sx)\n' "${runs}"
  bench ./go.xslash version
)

xslash -- create --skeleton "../prebuilt/go/bin/go.xslash" --out "./inject/go.xslash" >/dev/null
printf '{"go":"go.xslash"}\n' >./inject/xslash.json
(
  cd ./inject
  eval "$(xslash --export --silent)"
  printf '\n## inject (%sx)\n' "${runs}"
  bench go version
)

mkdir -p ./dotslash
sed -E '
  1s|^#!/usr/bin/env xslash$|#!/usr/bin/env dotslash|
  /^[[:space:]]*"(darwin|linux|windows)-(amd64|arm64)": \{$/ {
    s/darwin/macos/
    s/amd64/x86_64/
    s/arm64/aarch64/
  }
' ./discover/go.xslash >./dotslash/go.dotslash
chmod +x ./dotslash/go.dotslash
(
  cd ./dotslash
  printf '\n## dotslash (%sx)\n' "${runs}"
  bench ./go.dotslash version
)

printf '\n# rust\n'

raw="$(xslash -- extract "../prebuilt/rust/bin/rustc.xslash")"
(
  printf '\n## raw (%sx)\n' "${runs}"
  bench "${raw}" --version
)

xslash -- create --skeleton "../prebuilt/rust/bin/rustc.xslash" --final --out "./final/rustc.xslash" >/dev/null
(
  cd ./final
  printf '\n## final (%sx)\n' "${runs}"
  bench ./rustc.xslash --version
)
