//! The xslash entry data model: parsing, platform selection, and the list of
//! downloadable sources that make up a platform.

use ::anyhow::Context;
use ::anyhow::Result;
use ::anyhow::bail;
use ::serde::Deserialize;
use ::serde::Serialize;
use ::std::collections::BTreeMap;

use crate::hash::Hash;

/// Strip a leading `#!` shebang line (if present) so the rest can be parsed as
/// JSON.
pub fn strip_shebang(content: &str) -> &str {
    if let Some(rest) = content.strip_prefix("#!") {
        match rest.find('\n') {
            Some(i) => &rest[i + 1..],
            None => "",
        }
    } else {
        content
    }
}

/// The host platform key, `<kernel>-<arch>`.
pub fn host_platform() -> String {
    let kernel = match ::std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        other => other, // use at your own risk; xslash doesn't officially support other OSes
    };
    let arch = match ::std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other, // use at your own risk; xslash doesn't officially support other OSes
    };
    format!("{kernel}-{arch}")
}

/// The alternative host platform key for dotslash compatibility, using Rust's
/// native os-arch names (e.g. `linux-x86_64`).
pub fn alternative_host_platform() -> String {
    format!("{}-{}", ::std::env::consts::OS, ::std::env::consts::ARCH)
}

/// One downloadable+extractable artifact resolved from a [`Platform`].
#[derive(Clone, Copy, Debug)]
pub struct Component<'a> {
    /// Expected size of the downloaded artifact in bytes.
    pub size: u64,

    /// Hash algorithm used for `digest`.
    pub hash: Hash,

    /// Expected digest of the downloaded artifact, in lowercase hex.
    pub digest: &'a str,

    /// Archive/compression format, or `None` for a single raw file.
    pub format: Option<Format>,

    /// For an archive, the leading in-archive subpath to strip: entries under it
    /// are rebased onto `dest` (the prefix is dropped and entries outside it are
    /// skipped); empty extracts the whole archive. Ignored for single-file
    /// formats.
    pub strip: &'a str,

    /// Sources to fetch the artifact from, tried in order.
    pub providers: &'a [Provider],

    /// Where to place this component within the extraction dir: the subpath an
    /// archive is unpacked into (empty for the extraction root), or the path a
    /// single-file artifact's one file is written to.
    pub dest: &'a str,
}

/// A single source for an artifact. xslash only fetches `http`/`https` URLs; a
/// provider with no `url` (e.g. one tagged with a `type` xslash doesn't
/// understand) is skipped.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provider {
    /// Provider type, kept for dotslash compatibility.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,

    /// The download URL. Must start with `http://` or `https://`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The decompressor to run on a downloaded artifact. Required whenever the
/// artifact is an archive; omitted for a single raw executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// A gzip-compressed tarball (`.tar.gz`).
    #[serde(rename = "tar.gz")]
    TarGz,

    /// An xz-compressed tarball (`.tar.xz`).
    #[serde(rename = "tar.xz")]
    TarXz,

    /// A zstd-compressed tarball (`.tar.zst`).
    #[serde(rename = "tar.zst")]
    TarZst,

    /// An uncompressed tarball (`.tar`).
    Tar,

    /// A single gzip-compressed file (`.gz`), not an archive.
    Gz,

    /// A single xz-compressed file (`.xz`), not an archive.
    Xz,

    /// A single zstd-compressed file (`.zst`), not an archive.
    Zst,

    /// A zip archive (`.zip`).
    Zip,
}

/// One element of a `compose` list: an archive extracted into a subdirectory of
/// the composed directory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Compose {
    /// Hash algorithm used for `digest`. Required; validated when resolving
    /// [`Component`]s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<Hash>,

    /// Expected digest of the downloaded artifact, in lowercase hex. Required;
    /// validated when resolving [`Component`]s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,

    /// Expected size of the downloaded artifact in bytes. Required; validated
    /// when resolving [`Component`]s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,

    /// Archive/compression format, or `None` for a single raw file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Format>,

    /// For an archive, strip this leading in-archive subpath: extract only the
    /// entries under it, rebasing them onto the destination (the composed root,
    /// or `path` when also set) with the prefix dropped; entries outside it are
    /// skipped. `None` extracts the whole archive verbatim. Ignored for
    /// single-file formats (`gz`, `xz`, `zst`, or no format), whose one file is
    /// placed at `path`.
    ///
    /// Lets several archives that each wrap their payload in a differing leading
    /// directory be merged into one tree - e.g. the Rust SDK's component
    /// tarballs (`rustc-<ver>-<target>/rustc`, `rust-std-<ver>-<target>/`
    /// `rust-std-<target>`, ...) rebased onto a shared sysroot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strip: Option<String>,

    /// Where this artifact is placed inside the composed directory. For an
    /// archive format, the subpath the decompressed archive is extracted into,
    /// or the composed directory root when `None`. For a single-file format
    /// (`gz`, `xz`, `zst`, or no format), the path at which the single file is
    /// placed; required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// Sources to fetch this artifact from, tried in order.
    pub providers: Vec<Provider>,
}

/// A per-platform block. Either a single artifact (via `size`/`hash`/`digest`/
/// `providers`) or several composed into one directory (via `compose`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Platform {
    /// Hash algorithm used for `digest`. Required for a single-artifact
    /// platform; must be absent when `compose` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<Hash>,

    /// Expected digest of the downloaded artifact, in lowercase hex. Required
    /// for a single-artifact platform; must be absent when `compose` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,

    /// Expected size of the downloaded artifact in bytes. Required for a
    /// single-artifact platform; must be absent when `compose` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,

    /// Archive/compression format, or `None` for a single raw executable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Format>,

    /// If true, extract under `rw/` (mutable) instead of `ro/` (readonly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rw: Option<bool>,

    /// A dotslash-compatibility fallback for `rw`, which it should not co-exist
    /// with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,

    /// Where the executable is found within the extraction directory; it is run
    /// from `<extraction dir>/<path>`. Required (enforced when the components
    /// are resolved).
    ///
    /// - Archive formats (`tar`, `tar.gz`, `tar.xz`, `tar.zst`, `zip`): the
    ///   path to the executable inside the extracted archive.
    /// - Single-file formats (`gz`, `xz`, `zst`, or no format): the path at
    ///   which the decompressed file - or, with no format, the raw binary - is
    ///   placed in the extraction directory; that file is itself the executable.
    /// - With `compose` set: the path to the executable within the composed
    ///   directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// Sources to fetch the artifact from, tried in order. Required for a
    /// single-artifact platform; must be absent when `compose` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<Vec<Provider>>,

    /// Several archives composed into one directory, as an alternative to a
    /// single artifact. Mutually exclusive with the sibling artifact fields
    /// above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose: Option<Vec<Compose>>,
}

impl Platform {
    /// The relative path to the executable within the extraction directory.
    ///
    /// Errors when `path` is absent or empty: a platform that extracts into a
    /// directory must say where its executable is.
    pub fn exepath(&self) -> Result<&str> {
        match self.path.as_deref() {
            Some(path) if !path.is_empty() => Ok(path),
            _ => bail!("platform block missing `path` to the executable"),
        }
    }

    /// Whether this platform extracts under `rw/` (mutable), defaulting to
    /// read-only (`ro/`).
    ///
    /// `rw` is authoritative when it decides the answer. `readonly` is only a
    /// dotslash-compatibility fallback that can force read-only: `readonly:
    /// true` keeps it read-only unless `rw: true` overrides, while `readonly:
    /// false` (or absent) is ignored.
    pub fn is_rw(&self) -> bool {
        if self.readonly == Some(true) {
            return self.rw == Some(true);
        }
        self.rw.unwrap_or(false)
    }

    /// A stable cache key: a blake3 over each artifact's identity — size, hash
    /// algorithm, digest, format, where it is placed within the extraction
    /// directory (`dest`), and which leading in-archive subpath is stripped
    /// (`strip`) — rendered as lowercase hex.
    ///
    /// Placement (`dest`) is folded in because a single-file format writes its
    /// one file at that path, so changing it changes the extracted layout and
    /// must move the cache entry; otherwise a renamed `path` would silently
    /// reuse the old extraction and resolve to a file that was never written.
    /// `strip` is folded in for the same reason: it selects which archive
    /// entries land where, so changing it changes the extracted layout.
    /// Deliberately independent of the providers used to fetch the artifact, so
    /// editing a URL doesn't move the cache entry. Unlike dotslash's `readonly`,
    /// the ro/rw distinction is not folded in because xslash already splits it
    /// by parent directory.
    pub fn extract_digest(&self) -> Result<String> {
        let mut hasher = blake3::Hasher::new();
        for src in self.components()? {
            hasher.update(src.size.to_string().as_bytes());
            hasher.update(b"\0");
            hasher.update(src.hash.as_str().as_bytes());
            hasher.update(b"\0");
            hasher.update(src.digest.as_bytes());
            hasher.update(b"\0");
            let format = match src.format {
                None => "",
                Some(Format::TarGz) => "tar.gz",
                Some(Format::TarXz) => "tar.xz",
                Some(Format::TarZst) => "tar.zst",
                Some(Format::Tar) => "tar",
                Some(Format::Gz) => "gz",
                Some(Format::Xz) => "xz",
                Some(Format::Zst) => "zst",
                Some(Format::Zip) => "zip",
            };
            hasher.update(format.as_bytes());
            hasher.update(b"\0");
            hasher.update(src.strip.as_bytes());
            hasher.update(b"\0");
            hasher.update(src.dest.as_bytes());
            hasher.update(b"\0");
        }
        let hex = hasher.finalize().to_hex();
        Ok(hex.to_string())
    }

    /// The artifacts to download (and extract) for this platform.
    pub fn components(&self) -> Result<Vec<Component<'_>>> {
        // Every platform needs a `path` to locate its executable within the
        // extraction dir, whether a single artifact, archive, or composed.
        let exepath = self.exepath()?;
        if let Some(compose) = &self.compose {
            if self.size.is_some()
                || self.hash.is_some()
                || self.digest.is_some()
                || self.format.is_some()
                || self.providers.is_some()
            {
                bail!("platform block sets both `compose` and sibling artifact fields");
            }
            let mut out = Vec::with_capacity(compose.len());
            for c in compose {
                let dest = match c.format {
                    // A single-file component is itself a file placed at `path`,
                    // which is therefore required.
                    None | Some(Format::Gz | Format::Xz | Format::Zst) => c
                        .path
                        .as_deref()
                        .filter(|p| !p.is_empty())
                        .context("composed single-file artifact missing `path`")?,
                    // An archive is unpacked at `path`, or the composed root
                    // when omitted.
                    Some(
                        Format::Tar | Format::TarGz | Format::TarXz | Format::TarZst | Format::Zip,
                    ) => c.path.as_deref().unwrap_or(""),
                };
                out.push(Component {
                    size: c.size.context("composed artifact missing `size`")?,
                    hash: c.hash.context("composed artifact missing `hash`")?,
                    digest: c
                        .digest
                        .as_deref()
                        .context("composed artifact missing `digest`")?,
                    format: c.format,
                    strip: c.strip.as_deref().unwrap_or(""),
                    providers: &c.providers,
                    dest,
                });
            }
            Ok(out)
        } else {
            let size = self.size.context("platform block missing `size`")?;
            let hash = self.hash.context("platform block missing `hash`")?;
            let digest = self
                .digest
                .as_deref()
                .context("platform block missing `digest`")?;
            let providers = self
                .providers
                .as_deref()
                .context("platform block missing `providers`")?;
            let dest = match self.format {
                // A single-file format (including a raw binary with no format)
                // is itself the executable, so it's placed at `path` within the
                // extraction dir. An archive is unpacked into the root, with the
                // executable then found at `path` inside it.
                None | Some(Format::Gz | Format::Xz | Format::Zst) => exepath,
                Some(
                    Format::Tar | Format::TarGz | Format::TarXz | Format::TarZst | Format::Zip,
                ) => "",
            };
            Ok(vec![Component {
                size,
                hash,
                digest,
                format: self.format,
                strip: "",
                providers,
                dest,
            }])
        }
    }
}

/// A parsed xslash entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XslashEntry {
    /// Logical name; the lookup key in xslash.json.
    pub name: String,

    /// If `Some(true)`, use this entry directly and skip project-level
    /// resolution. Absent or `Some(false)` both mean not final.
    #[serde(default, rename = "final", skip_serializing_if = "Option::is_none")]
    pub is_final: Option<bool>,

    /// The entry file this one resolves to, its `platforms` replacing this
    /// entry's. Absolute, or relative to this file's directory; absent or empty
    /// means no redirect. The hook bakes it into shims so they resolve from any
    /// cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,

    /// Empty for a bare name-only entry (e.g. a directory-hook shim) that defers
    /// to project-level resolution for the real platform blocks.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub platforms: BTreeMap<String, Platform>,
}

impl XslashEntry {
    /// Whether this entry is used directly, skipping project-level resolution.
    /// Absent and `Some(false)` are both treated as not final.
    pub fn is_final(&self) -> bool {
        self.is_final == Some(true)
    }

    /// The redirect target, or `None` when there is none: both an absent and an
    /// empty `actual` mean no redirect.
    pub fn actual(&self) -> Option<&str> {
        self.actual.as_deref().filter(|a| !a.is_empty())
    }

    /// Parse an xslash entry from its file contents, tolerating a leading
    /// shebang.
    pub fn parse(content: &str) -> Result<XslashEntry> {
        let json = strip_shebang(content);
        serde_json::from_str(json).context("parsing xslash entry")
    }

    /// Select the platform block for `platform_key`. When `platform_key` is
    /// `None`, matches the host kernel-arch, falling back to the alternative key
    /// (e.g. `linux-x86_64`) for dotslash compatibility when the preferred key
    /// (e.g. `linux-amd64`) is absent.
    pub fn select_platform(&self, platform_key: Option<&str>) -> Result<(String, &Platform)> {
        if let Some(key) = platform_key {
            let platform = match self.platforms.get(key) {
                Some(platform) => platform,
                None => bail!("entry {:?} has no platform block for {key}", self.name),
            };
            return Ok((key.to_string(), platform));
        }

        let host_key = host_platform();
        if let Some(platform) = self.platforms.get(&host_key) {
            return Ok((host_key, platform));
        }

        let alternative_key = alternative_host_platform();
        if let Some(platform) = self.platforms.get(&alternative_key) {
            return Ok((alternative_key, platform));
        }

        bail!(
            "entry {:?} has no platform block for {host_key} (or {alternative_key})",
            self.name
        )
    }
}
