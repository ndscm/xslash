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

xslash -- create --skeleton "./cargo-clippy.xslash.json" "${replacements[@]}" --final true --out "./bin/cargo-clippy.xslash"
xslash -- create --skeleton "./cargo-fmt.xslash.json" "${replacements[@]}" --final true --out "./bin/cargo-fmt.xslash"
xslash -- create --skeleton "./cargo.xslash.json" "${replacements[@]}" --final true --out "./bin/cargo.xslash"
xslash -- create --skeleton "./rust-gdb.xslash.json" "${replacements[@]}" --final true --out "./bin/rust-gdb.xslash"
xslash -- create --skeleton "./rustc.xslash.json" "${replacements[@]}" --final true --out "./bin/rustc.xslash"
xslash -- create --skeleton "./rustdoc.xslash.json" "${replacements[@]}" --final true --out "./bin/rustdoc.xslash"
xslash -- create --skeleton "./rustfmt.xslash.json" "${replacements[@]}" --final true --out "./bin/rustfmt.xslash"

ln -s -f "cargo-clippy.xslash" "./bin/cargo-clippy"
ln -s -f "cargo-fmt.xslash" "./bin/cargo-fmt"
ln -s -f "cargo.xslash" "./bin/cargo"
ln -s -f "rust-gdb.xslash" "./bin/rust-gdb"
ln -s -f "rustc.xslash" "./bin/rustc"
ln -s -f "rustdoc.xslash" "./bin/rustdoc"
ln -s -f "rustfmt.xslash" "./bin/rustfmt"
