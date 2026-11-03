#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

(
  cd ./standalone
  pnpm install --frozen-lockfile
  pnpm deploy dist
  # Build a reproducible archive with the Python stdlib (portable across linux
  # and macos, unlike GNU-tar-only flags): entries added in sorted order with a
  # fixed mtime and zeroed owner, inside a gzip stream whose header carries no
  # name/mtime. Without this the tarball's bytes (and digest) churn every build.
  python3 - <<'PY'
import gzip, io, json, os, tarfile

src, out = "dist", "../standalone.tar.gz"
root_pkg = os.path.join(src, "package.json")

# pnpm bookkeeping that churns every build and isn't needed to run prettier.cjs:
# `.bin` shims bake in the absolute deploy path (useless once xslash relocates
# the bundle), `.modules.yaml` carries a `prunedAt` timestamp, and
# `.pnpm-workspace-state-v<n>.json` records per-build state. Drop them all.
skip_dirs = {".bin"}

def skip_file(name):
    return name == ".modules.yaml" or name.startswith(".pnpm-workspace-state")

entries = [src]
for root, dirs, files in os.walk(src):
    dirs[:] = [d for d in dirs if d not in skip_dirs]
    entries += [os.path.join(root, d) for d in dirs]
    entries += [os.path.join(root, f) for f in files if not skip_file(f)]

def reset(info):
    info.mtime = 0
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    return info

with open(out, "wb") as raw, \
        gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as gz, \
        tarfile.open(fileobj=gz, mode="w", format=tarfile.GNU_FORMAT) as tar:
    for path in sorted(set(entries)):
        # pnpm writes dist/package.json with dependency keys in a
        # nondeterministic order; canonicalize it (sorted keys) so it's stable.
        if path == root_pkg:
            data = json.dumps(
                json.loads(open(path, "rb").read()), indent=2, sort_keys=True
            ).encode() + b"\n"
            info = reset(tar.gettarinfo(path, arcname=path))
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
        else:
            tar.add(path, arcname=path, recursive=False, filter=reset)
PY
  rm -rf dist
)

xslash -- create --skeleton "./prettier-standalone.xslash.json" --fileurl translate --final --out "./bin/prettier-standalone.xslash" --symlink prettier-standalone
