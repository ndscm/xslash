/**
 * A parsed xslash entry: JSON describing where to download a prebuilt executable
 * per platform. A leading `#!` shebang line is stripped before the JSON is parsed.
 */
export type XslashEntry = {
  /** Logical name; the lookup key in `xslash.json`. */
  name: string

  /** If true, use this entry directly and skip project-level resolution. */
  final?: boolean

  /**
   * Another entry file this one resolves to, its `platforms` replacing this
   * entry's. Absolute, or relative to this file's dir. The hook bakes it into
   * shims so they resolve from any cwd.
   */
  actual?: string

  /**
   * Platform blocks keyed by `<kernel>-<arch>` — kernel `linux`/`darwin`/
   * `windows`, arch `amd64`/`arm64` (e.g. `linux-amd64`). Empty for a name-only
   * entry that defers to project-level resolution.
   *
   * Each block is a single artifact or a `compose` of several into one directory;
   * `path` is always required. Enforced at resolution, not by this type.
   */
  platforms?: {
    [platform: string]:
      | {
          /** Hash algorithm for `digest`; only `blake3` is supported. */
          hash: string

          /** Expected artifact digest, lowercase hex. */
          digest: string

          /** Expected artifact size in bytes. */
          size: number

          /**
           * Archive/compression format, or omitted for a raw executable. One of
           * `tar.gz`, `tar.xz`, `tar.zst`, `tar`, `gz`, `xz`, `zst`, `zip`.
           */
          format?: string

          /** If true, extract under `rw/` (mutable) instead of `ro/` (shared). */
          rw?: boolean

          /** dotslash-compatibility fallback for `rw`; shouldn't co-exist with it. */
          readonly?: boolean

          /**
           * Path to the executable, run from `<extraction dir>/<path>`. For a
           * single-file format (`gz`/`xz`/`zst`/none) it's where the decompressed
           * file — itself the executable — is placed.
           */
          path: string

          /**
           * Download sources, tried in order. At least one must carry a `url`;
           * `url` is optional per provider only for dotslash compatibility.
           */
          providers?: {
            /** Provider type, kept for dotslash compatibility. */
            type?: string

            /** Download URL; `http`/`https` only. Optional only for dotslash compat. */
            url?: string
          }[]
        }
      | {
          /** If true, extract under `rw/` (mutable) instead of `ro/` (shared). */
          rw?: boolean

          /** Path to the executable within the composed directory. */
          path: string

          /**
           * Several artifacts composed into one directory, instead of a single
           * artifact. Mutually exclusive with the artifact fields above.
           */
          compose?: {
            /** Hash algorithm for `digest`; only `blake3` is supported. */
            hash: string

            /** Expected artifact digest, lowercase hex. */
            digest: string

            /** Expected artifact size in bytes. */
            size: number

            /**
             * Archive/compression format, or omitted for a raw file. One of
             * `tar.gz`, `tar.xz`, `tar.zst`, `tar`, `gz`, `xz`, `zst`, `zip`.
             */
            format?: string

            /**
             * For an archive, extract only entries under this leading subpath,
             * dropping the prefix; omit to extract the whole archive. Ignored for
             * single-file formats. Lets archives with differing leading directories
             * merge into one tree.
             */
            strip?: string

            /**
             * Where this artifact goes in the composed directory. Archive: the
             * subpath to extract into (defaults to the root). Single-file: the path
             * the file is placed at (required).
             */
            path?: string

            /** Download sources, tried in order. */
            providers: {
              /** Download URL; `http`/`https` only. */
              url: string
            }[]
          }[]
        }
  }
}
