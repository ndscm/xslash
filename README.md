# xslash - simplified distribution of prebuilt executables

xslash downloads prebuilt archives from arbitrary URLs, extracts them into cache
directory, and execute the prebuilt executable binary from there.

```bash
$ cd path/to/project

$ ./python --version
# xslash: downloading python 3.15.1 ...
Python 3.15.1

$ ./rustc --version
# xslash: downloading rust 1.99.1 ...
rustc 1.99.1 (58cf8f9c3 2026-10-13)
```

**Benefits**

- Project based - pin dependency toolchains without specialized manager
- No central registry - just http(s) GET permanent links
- Versionless - no version number file to parse
- Distroless - no OS distro specific install
- Lazy downloads - artifacts fetched on first use only
- On-premises cache server for fast, shared fetches

**Trade-offs**

- No dynamic library dependencies
- No side-effect installation

**Good fits**

- Compilers: clang, go, node, python, rust
- Formatters: clang-format, gofmt
- Tools: pnpm, uv, bazel, buck

**Poor fits**

- `git` - no authorized first-party prebuilt executable
- `prettier` - not a prebuilt executable (it's a Node.js script)
- `docker` - requires root permission
- `curl` - needs dynamic library dependencies

## xslash entry

An xslash entry is a JSON file that describes where to download a prebuilt
executable. These files usually end in `.xslash.json`. An entry may also begin
with a shebang line so the shell can locate the xslash binary; in that case the
file usually ends in `.xslash` or has no extension, and is marked executable
(`+x`) so you can run it directly from the shell.

> **Note:** JSON5-style comments are not supported in real entry files (they're
> dropped for parsing speed). The `//` annotations in the example below are
> documentation only - don't include them in actual entries. The one exception
> is the leading shebang: if the first line starts with `#!`, xslash strips it
> before parsing the JSON.

```jsonc
// add shebang: #!/usr/bin/env xslash
{
  "name": "node", // logical name; used as arg0 on exec and as the lookup key in xslash.json
  "final": true, // if true, use this entry directly and skip project-level xslash.json resolution
  "platforms": {
    "linux-amd64": {
      "hash": "blake3", // blake3 is the only supported hash algorithm for now
      "digest": "<blake3>",
      "size": 1234, // bytes
      "format": "tar.gz", // decompressor, required if the artifact is an archive
      "rw": false, // if true, extract under rw/ (mutable) instead of ro/ (shared, read-only)
      "path": "path/to/exe", // required; where the executable is found within the extracted dir.
      // Every entry extracts into a directory and the executable is run from <dir>/<path>:
      // for an archive format, the path to the executable inside the extracted archive;
      // for a single-file format (gz/xz/zst/none), where the decompressed file - or, with no
      // format, the raw binary - is placed in the extracted dir; that file is the executable.
      // When "compose" is set, the path into the composed directory
      "providers": [
        {
          "url": "https://github.com/example/tool/releases/download/x.y.z/tool-x.y.z-linux-amd64.tar.gz",
        },
        // additional urls if the first fails
      ],
      "compose": [
        // if compose is set, the sibling size, hash, digest, format, and providers must be omitted;
        // the top-level path stays and points into the composed directory.
        {
          "hash": "blake3",
          "digest": "<blake3>",
          "size": 1234,
          "format": "tar.gz", // decompressor, required if the artifact is an archive
          "path": "path/in/composed/dir", // where this artifact goes in the composed dir:
          // for an archive format, the subpath it's extracted into (optional; defaults
          // to the composed dir root); for a single-file format (gz/xz/zst/none), where
          // the single file is placed (required)
          "strip": "subpath/inside/archive", // optional, archive formats only: strip this
          // leading in-archive subpath, extracting only the entries under it rebased onto
          // the destination with the prefix dropped (entries outside it are skipped). Lets
          // archives that each wrap their payload in a differing leading dir merge into one
          // tree - e.g. the Rust SDK's rustc-<ver>-<target>/rustc and
          // rust-std-<ver>-<target>/rust-std-<target> onto a shared sysroot.
          "providers": [
            {
              "url": "https://github.com/example/tool/releases/download/x.y.z/tool-x.y.z-linux-amd64.tar.gz",
            },
            // additional urls if the first fails
          ],
        },
        // additional artifacts
      ],
    },
    "darwin-arm64": {
      // ...
    },
    "windows-amd64": {
      // ...
    },
  },
}
```

Platform keys are `<kernel>-<arch>`:

- **kernel:** `linux`, `darwin`, `windows`
- **arch:** `amd64`, `arm64`

At runtime, xslash picks the block that matches the host os/arch, and errors out
if there's no matching block.

## Commands

Since the common invocation is `xslash <entry>`, every subcommand goes after
`--` so it's never mistaken for an entry-file path.

### `xslash <entry.xslash>`

Run the prebuilt executable defined in the xslash entry file.

### `xslash -- <command>`

Run a helper command (see below).

### `xslash --export`

Generate shell code to sync PATH with the current project. Run it with
`eval "$(xslash --export)"`.

### `xslash --hook`

Generate shell code to hook `--export` for interactive shells. Install it by
putting `eval "$(xslash --hook)"` in `~/.bashrc` or `~/.zshrc`.

### `xslash --help`

Print the help message.

### `xslash --version`

Print the build version.

## Helper commands

Helper commands are used for creating and updating xslash entries. It also helps
to manage cached artifacts for moving cached artifacts between workstations.

### `xslash -- create [options]`

Create or update an xslash entry from a skeleton file.

- `--skeleton executable.xslash.json` - required; the skeleton to fill in.
- `--replace {{TAG}}=x.x.x` - repeatable; replaces every literal occurrence of
  the marker (here `{{TAG}}`) in the skeleton file. The marker is matched
  verbatim - any string works; `{{...}}` is just the recommended convention.
- `--final` - sets the `final` flag in the generated entry; absent means not
  final.
- `--out executable.xslash` - required; the path to write the entry to.
- `--symlink name` - repeatable; create a symbolic link beside `--out`.

### `xslash -- download [--all] <entry.xslash>`

Download the archive without extracting it.

- `--all` - download every platform.

### `xslash -- extract [--clean] <entry.xslash>`

Download and extract artifacts.

- `--clean` - remove the downloaded artifact afterward.

### `xslash -- clean [--downloaded|--extracted] <entry.xslash>|<duration>`

Remove extracted files to reclaim disk space.

- `--downloaded` - remove downloaded artifacts.
- `--extracted` - remove extracted files.
- With neither flag, both downloaded and extracted are removed.
- `<duration>` - a duration like `2d`, `3m`, `4y` (days, months, years); removes
  entries whose last touch (access) time is older than the duration.

## Environment

- `XSLASH_CACHE_HOME` - directory holding this user's downloaded artifacts and
  extracted executables. Default: `$XDG_CACHE_HOME/xslash/` or
  `~/.cache/xslash/`, except for root, which defaults to `/var/cache/xslash/`.
- `XSLASH_GLOBAL_CACHE_HOME` - directory holding shared artifacts, usually the
  cache home of the root user. Treated as read-only: a globally extracted `ro/`
  entry is run directly; otherwise its downloaded artifact is reused and
  extracted into the user's own cache home. Unset by default; the shared cache
  is consulted only when this is explicitly set. The recommended location is
  `/var/cache/xslash/`.
- `XSLASH_CACHE_SERVER` - an http(s) server queried for an artifact before
  falling back to the providers listed in the entry.
- `XSLASH_CREDENTIAL_HELPER` - path to a credential helper invoked to obtain
  authorization (e.g. an `Authorization` header) for provider or cache-server
  requests.

## Cache home

```
CACHE_HOME/
  download/
    probe.json               # cached provider URL -> hash/digest/size
    <hash>/                  # the hash algorithm used (default blake3)
      <digest>               # the downloaded file whose content matches <digest>
      <digest>.json          # metadata for the downloaded file
  ro/                        # extracted read-only entries (shared between runs)
    <digest>/                # the extracted directory
    <digest>.json            # metadata for the extracted directory
  rw/                        # extracted read-write entries (use sparingly; defeats sharing)
    <digest>/                # the extracted directory
    <digest>.json            # metadata for the extracted directory
  resolve/                   # per-directory shim dirs injected into PATH
    <digest>/                # keyed by the hash of the project directory that holds its xslash.json
      <name>                 # a shim named after the entry's "name" so the shell finds it
```

For `ro/`, `<digest>` is the hash of the serialized extraction metadata (the
entry's platform block), not of any single source archive, so a composed
directory built from several archives still gets one stable key.

## xslash.json

xslash entries aren't always on `PATH` for a subprocess such as
`git rebase --exec` or `claude`. To mimic direnv's behavior even in that case, a
project can place an `xslash.json` at its root. When an entry that isn't `final`
is invoked - for example a system entry at `/usr/local/bin/entry.xslash` or
`~/.local/bin/entry.xslash` - xslash searches upward from the current directory
for an `xslash.json`; if it finds one that maps the entry's `name`, the
project-defined entry is used instead of the invoked one.

```json
{
  "clang": "path/to/clang.xslash",
  "go": "path/to/go.xslash",
  "node": "path/to/node.xslash",
  "python": "path/to/python.xslash"
}
```

The key should match the `name` in the system entry.

If `final` is true in the xslash entry (usually the repo entry), xslash skips
the search.

## Cache server

When `XSLASH_CACHE_SERVER` is set, xslash sends an http(s) GET to
`XSLASH_CACHE_SERVER/<hash>/<digest>` first (e.g.
`https://cache.example.com/some/path/blake3/xxxxxx`) instead of hitting the
provider URL directly. If that comes back non-OK, xslash downloads the artifact
from the provider and then tries to PUT it back to the same cache location.

## Credential helper

When issuing http(s) requests against a provider or cache server, if
`XSLASH_CREDENTIAL_HELPER` is set (it must be an absolute path to an executable
script or binary, and is not managed by xslash), xslash calls
`$(XSLASH_CREDENTIAL_HELPER <method> <url>)`, where `<method>` is `GET` or `PUT`
(`PUT` is used only for the cache server). On success, the credential helper
should print a single JSON object to stdout, in the same format as the bazel
credential helper. If anonymous access is acceptable, empty stdout is fine. If
the credential helper exits non-zero, the corresponding GET/PUT request is
skipped.

```json
{
  "headers": {
    "Authorization": ["Bearer xxx.xxx.xxx"]
  }
}
```

## Inspired by dotslash

xslash is [dotslash](https://dotslash-cli.com/) extended with a few extra
features:

- Splits download and extract into two steps, for offline caching
- Queries a cache server before reaching out to the public server
- Generates entry files from templates
- Composes multiple archives into one directory (extremely useful for the Rust
  SDK)
- Looks up project-level executables (like direnv, but also works in
  subprocesses - useful for `git rebase --exec`, `claude`, and the like)

xslash deliberately drops the following dotslash features:

- Non-http providers
- GitHub-specific actions

## Limitations

See: <https://dotslash-cli.com/docs/limitations/>

## Features under discussion

- Selecting an artifact tag with a script
- A client SDK for other languages
- Windows support

## Benchmark

TODO
