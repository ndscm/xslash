#!/usr/bin/env bash
set -euo pipefail

work_dir="$(pwd)"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Prettier keys everything off process.cwd(): it ESM-imports a config's plugins
# from there (src/main/plugins/load-plugin.js), path.resolve()s file patterns
# against it (src/cli/expand-patterns.js), and reads the default .gitignore /
# .prettierignore relative to it (src/utilities/ignore.js). The bundled plugins
# live in this launcher's node_modules, so we must run from here - but that then
# resolves the caller's files/ignores against the wrong directory.
#
# Formatting is pinned to the embedded .prettierrc beside this script; the
# caller's project config, any --config flag, and .editorconfig are ignored, so
# the standalone formats identically everywhere. Ignore files are still honored
# from the caller's directory, reproduced below since we run from elsewhere.

# Honor an explicit --ignore-path from the caller rather than overriding it.
has_ignore=false
for arg in "$@"; do
  case "${arg}" in
  --ignore-path | --ignore-path=*) has_ignore=true ;;
  esac
done

# Always format with the embedded config, and with .editorconfig disabled.
pre_args=(--config "${script_dir}/.prettierrc" --no-editorconfig)

# The default ignore files are read from the working directory only (no upward
# walk), so anchor both to the caller's directory. A missing path is harmless -
# prettier reads it as empty, exactly as the relative default would.
if ! "${has_ignore}"; then
  pre_args+=(--ignore-path "${work_dir}/.gitignore")
  pre_args+=(--ignore-path "${work_dir}/.prettierignore")
fi

# Re-anchor path-like arguments to the caller's directory (prettier
# path.resolve()s them against cwd, which is the extract dir below), and drop any
# caller-supplied config flag - the embedded config above is authoritative.
# Flags and bare values (`--parser babel`, `--tab-width 2`) pass through.
skip_next=false
for arg in "$@"; do
  if "${skip_next}"; then
    skip_next=false
    continue
  fi
  case "${arg}" in
  --config) skip_next=true ;;      # drop the flag and its value
  --config=* | --no-config) ;;     # drop
  -* | /*) pre_args+=("${arg}") ;; # flag or already-absolute path
  *)
    if [[ "${arg}" == *[/*?[]* || -e "${work_dir}/${arg}" ]]; then
      pre_args+=("${work_dir}/${arg}")
    else
      pre_args+=("${arg}")
    fi
    ;;
  esac
done

cd "${script_dir}"
exec ./node_modules/prettier/bin/prettier.cjs "${pre_args[@]}"
