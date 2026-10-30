#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

tag="$(cat ./TAG)"
version="$(cat ./VERSION)"

if [[ "${1:-}" == "latest" || "${1:-}" == "lts" ]]; then
  query="os=linux&arch=x64"
  query+="&archive_type=tar.gz&java_package_type=jdk"
  if [[ "${1:-}" == "lts" ]]; then
    query+="&support_term=lts"
  fi
  query+="&latest=true"

  packages="$(curl -s "https://api.azul.com/metadata/v1/zulu/packages/?${query}")"
  name="$(jq -r 'first(.[].name | select(test("^zulu[0-9.]+-ca-jdk[0-9.]+-linux_x64\\.tar\\.gz$")))' <<<"${packages}")"
  rest="${name#zulu}"
  tag="${rest%%-ca-jdk*}"
  rest="${rest#*-ca-jdk}"
  version="${rest%%-linux_x64*}"

  printf "%s\n" "${tag}" >./TAG
  printf "%s\n" "${version}" >./VERSION
fi

replacements=(--replace "{{TAG}}=${tag}" --replace "{{VERSION}}=${version}")

xslash -- create --skeleton "./jar.xslash.json" "${replacements[@]}" --final --out "./bin/jar.xslash" --symlink jar
xslash -- create --skeleton "./java.xslash.json" "${replacements[@]}" --final --out "./bin/java.xslash" --symlink java
xslash -- create --skeleton "./javac.xslash.json" "${replacements[@]}" --final --out "./bin/javac.xslash" --symlink javac
xslash -- create --skeleton "./jshell.xslash.json" "${replacements[@]}" --final --out "./bin/jshell.xslash" --symlink jshell
