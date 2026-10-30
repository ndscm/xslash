#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="${1:-"$(cat ./TAG)"}"
version="${2:-"$(cat ./VERSION)"}"

if [[ "${1:-}" == "latest" ]]; then
  stable="$(curl -s "https://static.rust-lang.org/dist/channel-rust-stable.toml")"
  tag="$(awk -F'"' '/^\[/{exit} /^date/{print $2; exit}' <<<"${stable}")"
  version="$(awk -F'"' '/^\[pkg\.rust\]/{f=1} f&&/^version/{split($2,v," "); print v[1]; exit}' <<<"${stable}")"
  printf "%s\n" "${tag}" >./TAG
  printf "%s\n" "${version}" >./VERSION
fi

replacements=(--replace "{{TAG}}=${tag}" --replace "{{VERSION}}=${version}")

xslash -- create --skeleton "./cargo-clippy.xslash.json" "${replacements[@]}" --final --out "./bin/cargo-clippy.xslash" --symlink cargo-clippy
xslash -- create --skeleton "./cargo-fmt.xslash.json" "${replacements[@]}" --final --out "./bin/cargo-fmt.xslash" --symlink cargo-fmt
xslash -- create --skeleton "./cargo.xslash.json" "${replacements[@]}" --final --out "./bin/cargo.xslash" --symlink cargo
xslash -- create --skeleton "./rust-analyzer.xslash.json" "${replacements[@]}" --final --out "./bin/rust-analyzer.xslash" --symlink rust-analyzer
xslash -- create --skeleton "./rust-gdb.xslash.json" "${replacements[@]}" --final --out "./bin/rust-gdb.xslash" --symlink rust-gdb
xslash -- create --skeleton "./rustc.xslash.json" "${replacements[@]}" --final --out "./bin/rustc.xslash" --symlink rustc
xslash -- create --skeleton "./rustdoc.xslash.json" "${replacements[@]}" --final --out "./bin/rustdoc.xslash" --symlink rustdoc
xslash -- create --skeleton "./rustfmt.xslash.json" "${replacements[@]}" --final --out "./bin/rustfmt.xslash" --symlink rustfmt
