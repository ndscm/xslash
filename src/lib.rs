//! xslash: simplified distribution of prebuilt executables.
//!
//! This crate exposes the core building blocks so other Rust callers can embed
//! xslash's resolution logic. The `xslash` and `xslash-helper` binaries (in
//! `src/bin/`) are thin CLIs on top of these modules.

mod cache;
mod chmod;
mod core;
mod entry;
mod exec;
mod hash;
mod http;
mod signal;

use ::anyhow::Context;
use ::anyhow::Result;
use ::anyhow::bail;
use ::std::ffi::OsStr;
use ::std::ffi::OsString;
use ::std::path::Path;
use ::std::path::PathBuf;
use ::std::time::SystemTime;

use crate::core::Core;

pub use crate::cache::Cache;
pub use crate::chmod::Chmod;
pub use crate::entry::XslashEntry;
pub use crate::hash::Hash;

/// Error if `entry` still carries an `actual` redirect: the `*_final` runners run
/// its `platforms` directly, so it must be followed first with [`read_actual`].
fn ensure_resolved(entry: &XslashEntry) -> Result<()> {
    if let Some(actual) = entry.actual() {
        bail!(
            "entry {} has an unresolved `actual` redirect to {actual}",
            entry.name
        );
    }
    Ok(())
}

/// Resolve `entry` for the host platform and **replace** the current process
/// with it, setting argv\[0\] to `arg0` and forwarding `forward` as its
/// arguments. Pass the path the launcher was invoked as for `arg0` so the
/// resolved binary sees exactly what a symlink in its place would.
///
/// The `final` field is ignored: this runs the given entry directly and does
/// not consult a project-level `xslash.json`. On success it never returns.
pub fn execv_final(entry: &XslashEntry, arg0: &OsStr, forward: &[OsString]) -> Result<()> {
    ensure_resolved(entry)?;
    let core = Core::from_env(true)?;
    let exe = core.ensure_extracted(entry)?;
    match crate::exec::execv(&exe, arg0, forward)? {}
}

/// Resolve `entry` for the host platform and run it as a **subprocess** with
/// argv\[0\] set to `arg0`, waiting for it to exit. Returns an error if it exits
/// with a non-zero status.
///
/// Like [`execv_final`], the `final` field is ignored.
pub fn spawn_final(entry: &XslashEntry, arg0: &OsStr, forward: &[OsString]) -> Result<()> {
    ensure_resolved(entry)?;
    let core = Core::from_env(true)?;
    let exe = core.ensure_extracted(entry)?;
    let status = crate::exec::spawn(&exe, arg0, forward)?;
    if !status.success() {
        bail!("{} exited with {status}", entry.name);
    }
    Ok(())
}

/// Read the xslash entry at `path`, following each `actual` (a path, absolute or
/// relative to the file that named it) to the entry it resolves to.
///
/// Errors if a redirect renames the entry, if the chain exceeds `MAX_REDIRECTS`,
/// or if it cycles back to a file already read.
pub fn read_actual(path: &Path) -> Result<XslashEntry> {
    /// How many `actual` redirects a single `read_actual` follows before erroring.
    const MAX_REDIRECTS: usize = 10;

    let mut current = path.to_path_buf();
    let mut visited: Vec<PathBuf> = Vec::new();
    // The name of the starting (shim) entry; every entry reached via `actual`
    // must match it.
    let mut name: Option<String> = None;
    // Read the starting entry, then follow up to MAX_REDIRECTS redirects.
    for _ in 0..=MAX_REDIRECTS {
        // Canonicalize so a cycle reached through differing spellings (absolute
        // vs relative, `..`, symlinks) is still detected.
        let canonical = current
            .canonicalize()
            .with_context(|| format!("reading entry {}", current.display()))?;
        if visited.contains(&canonical) {
            bail!("xslash entry {} redirects in a loop", canonical.display());
        }
        visited.push(canonical.clone());

        let contents = ::std::fs::read_to_string(&canonical)
            .with_context(|| format!("reading entry {}", canonical.display()))?;
        let entry = XslashEntry::parse(&contents)
            .with_context(|| format!("in entry {}", canonical.display()))?;

        // `actual` is a replacement for `platforms`, not an identity change, so a
        // redirect may not rename the entry.
        match &name {
            None => name = Some(entry.name.clone()),
            Some(expected) => {
                if entry.name != *expected {
                    bail!(
                        "xslash entry {} is named {:?}, but it is reached through an `actual` \
                         redirect from an entry named {:?}",
                        canonical.display(),
                        entry.name,
                        expected
                    );
                }
            }
        }

        // No redirect (absent or empty `actual`) ends the chain here.
        let Some(actual) = entry.actual() else {
            return Ok(entry);
        };

        // A redirect target is absolute, or relative to the directory of the
        // file that named it.
        let target = Path::new(actual);
        current = if target.is_absolute() {
            target.to_path_buf()
        } else {
            canonical
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(target)
        };
    }

    // The loop ran every allowed iteration and the last entry still redirected.
    bail!(
        "xslash entry {} redirects more than {MAX_REDIRECTS} times",
        path.display()
    )
}

/// Fill in a skeleton entry: apply `replacements` (each `MARKER`/`VALUE`
/// pair replaces every literal occurrence of `MARKER` in paths, provider URLs,
/// and compose `strip` subpaths; `{{NAME}}` is the recommended marker style but
/// any string works),
/// download every provider to compute its size and blake3 digest, and set the
/// `final` flag.
pub fn create(
    skeleton: XslashEntry,
    replacements: Vec<(String, String)>,
    is_final: bool,
) -> Result<XslashEntry> {
    let core = Core::from_env(false)?;
    let apply = |s: &str| {
        let mut s = s.to_string();
        for (marker, value) in &replacements {
            s = s.replace(marker, value);
        }
        s
    };

    // First pass: apply replacements and set the final flag - pure rewriting,
    // no network, so the entry is fully substituted before any probing.
    let mut entry = skeleton;
    // A false flag stays absent, never `Some(false)`.
    entry.is_final = is_final.then_some(true);
    for platform in entry.platforms.values_mut() {
        if let Some(path) = platform.path.as_mut() {
            *path = apply(path);
        }
        if let Some(providers) = platform.providers.as_mut() {
            for provider in providers.iter_mut() {
                if let Some(url) = provider.url.as_mut() {
                    *url = apply(url);
                }
            }
        }
        if let Some(compose) = platform.compose.as_mut() {
            for c in compose.iter_mut() {
                if let Some(strip) = c.strip.as_mut() {
                    *strip = apply(strip);
                }
                if let Some(path) = c.path.as_mut() {
                    *path = apply(path);
                }
                for provider in &mut c.providers {
                    if let Some(url) = provider.url.as_mut() {
                        *url = apply(url);
                    }
                }
            }
        }
    }

    // Second pass: probe each artifact to fill in its size/hash/digest,
    // recording the entry in the artifact's cache sidecar as it goes.
    core.probe(&mut entry)?;

    Ok(entry)
}

/// Download (but do not extract) the artifacts for `entry`'s `platform_key`,
/// returning the path of each downloaded artifact in the cache. An empty
/// `platform_key` selects the host platform.
pub fn download(entry: &XslashEntry, platform_key: Option<&str>) -> Result<Vec<PathBuf>> {
    let core = Core::from_env(false)?;
    core.ensure_downloaded(entry, platform_key)
}

/// Download and extract the host-platform artifacts for `entry`, returning the
/// path to the resolved executable. When `clean` is set, the downloaded
/// artifacts are removed afterward, keeping only the extracted files.
pub fn extract(entry: &XslashEntry, clean: bool) -> Result<PathBuf> {
    let core = Core::from_env(false)?;
    let exe = core.ensure_extracted(entry)?;

    if clean {
        let (_, platform) = entry.select_platform(None)?;
        for src in platform.components()? {
            let path = core.cache.download_path(src.hash, src.digest);
            ::std::fs::remove_file(&path).ok();
            ::std::fs::remove_file(path.with_extension("json")).ok();
        }
    }
    Ok(exe)
}

/// Remove cached state for `entry`'s host platform: its downloaded artifacts
/// (when `downloaded`) and/or its extracted files (when `extracted`).
pub fn clean(entry: &XslashEntry, downloaded: bool, extracted: bool) -> Result<()> {
    let cache = Cache::from_env(false)?;
    if downloaded {
        cache.clean_downloaded_entry(entry)?;
    }
    if extracted {
        cache.clean_extracted_entry(entry)?;
    }
    Ok(())
}

/// Remove cached state whose last access time is before `before`: downloaded
/// artifacts (when `downloaded`) and/or extracted files (when `extracted`).
pub fn clean_before(before: SystemTime, downloaded: bool, extracted: bool) -> Result<()> {
    let cache = Cache::from_env(false)?;
    if downloaded {
        cache.clean_downloaded_before(before)?;
    }
    if extracted {
        cache.clean_extracted_before(before)?;
    }
    Ok(())
}
