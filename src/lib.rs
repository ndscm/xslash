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

fn absolutize_path_components(path: &Path) -> Result<Vec<OsString>> {
    let absolute =
        ::std::path::absolute(path).with_context(|| format!("resolving {}", path.display()))?;

    // `std::path::absolute` keeps interior `..` (it can't resolve them without
    // touching the filesystem), so collapse them lexically: drop `.` components
    // and fold each `..` into the preceding one.
    let mut normalized = PathBuf::new();
    for comp in absolute.components() {
        match comp {
            ::std::path::Component::CurDir => {}
            ::std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized
        .components()
        .map(|c| c.as_os_str().to_os_string())
        .collect())
}

fn rebase_path(base: &Path, target: &Path) -> Result<PathBuf> {
    let base = absolutize_path_components(base)?;
    let target = absolutize_path_components(target)?;
    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();

    let mut rel = PathBuf::new();
    for _ in common..base.len() {
        rel.push("..");
    }
    for comp in &target[common..] {
        rel.push(comp);
    }
    Ok(rel)
}

fn absolutize_fileurl(url: &str, fileurl_base: &Path) -> Result<String> {
    let Some(path) = url.strip_prefix("file:") else {
        return Ok(url.to_string());
    };
    if Path::new(path).is_absolute() {
        return Ok(url.to_string());
    }
    let absolute: PathBuf = absolutize_path_components(&fileurl_base.join(path))?
        .iter()
        .collect();
    Ok(format!("file:{}", absolute.display()))
}

fn rewrite_provider_urls(
    entry: &mut XslashEntry,
    mut f: impl FnMut(&str) -> Result<String>,
) -> Result<()> {
    for platform in entry.platforms.values_mut() {
        if let Some(providers) = platform.providers.as_mut() {
            for provider in providers.iter_mut() {
                if let Some(url) = provider.url.as_mut() {
                    *url = f(url)?;
                }
            }
        }
        if let Some(compose) = platform.compose.as_mut() {
            for c in compose.iter_mut() {
                for provider in &mut c.providers {
                    if let Some(url) = provider.url.as_mut() {
                        *url = f(url)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Resolve `entry` for the host platform and **replace** the current process
/// with it, resolving relative `file:` URLs against `fileurl_base`. Sets argv\[0\]
/// to `arg0` and forwards `forward` as its arguments; pass the path the launcher
/// was invoked as so the binary sees what a symlink in its place would.
///
/// The `final` field is ignored: this runs the given entry directly, not a
/// project-level `xslash.json`. On success it never returns.
pub fn execv_final_full(
    fileurl_base: &Path,
    entry: &XslashEntry,
    arg0: &OsStr,
    forward: &[OsString],
) -> Result<()> {
    ensure_resolved(entry)?;
    let core = Core::from_env(true)?;
    let mut entry = entry.clone();
    rewrite_provider_urls(&mut entry, |url| absolutize_fileurl(url, fileurl_base))?;
    let exe = core.ensure_extracted(&entry)?;
    match crate::exec::execv(&exe, arg0, forward)? {}
}

/// Like [`execv_final_full`], used for http(s) only entries.
pub fn execv_final(entry: &XslashEntry, arg0: &OsStr, forward: &[OsString]) -> Result<()> {
    execv_final_full(Path::new("."), entry, arg0, forward)
}

/// Like [`execv_final_full`], but run `entry` as a **subprocess** and wait for it
/// to exit, returning an error if it exits with a non-zero status.
pub fn spawn_final_full(
    fileurl_base: &Path,
    entry: &XslashEntry,
    arg0: &OsStr,
    forward: &[OsString],
) -> Result<()> {
    ensure_resolved(entry)?;
    let core = Core::from_env(true)?;
    let mut entry = entry.clone();
    rewrite_provider_urls(&mut entry, |url| absolutize_fileurl(url, fileurl_base))?;
    let exe = core.ensure_extracted(&entry)?;
    let status = crate::exec::spawn(&exe, arg0, forward)?;
    if !status.success() {
        bail!("{} exited with {status}", entry.name);
    }
    Ok(())
}

/// Like [`spawn_final_full`], used for http(s) only entries.
pub fn spawn_final(entry: &XslashEntry, arg0: &OsStr, forward: &[OsString]) -> Result<()> {
    spawn_final_full(Path::new("."), entry, arg0, forward)
}

/// Read the xslash entry at `path`, following each `actual` (a path, absolute or
/// relative to the file that named it) to the entry it resolves to.
///
/// Errors if a redirect renames the entry, if the chain exceeds `MAX_REDIRECTS`,
/// or if it cycles back to a file already read.
pub fn read_actual(path: &Path) -> Result<(PathBuf, XslashEntry)> {
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
            return Ok((canonical, entry));
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

/// How [`create`] handles a `file:` provider URL found in the skeleton. In every
/// mode an absolute `file:/abs/path` is written unchanged; the mode only governs
/// a relative path.
///
/// There is no single "keep + absolutize" mode (`keep` reads the path as relative
/// to the output dir, `absolutize` as relative to the skeleton dir). To get that
/// effect, run it in two rounds: first `keep` to produce the output entry, then
/// `absolutize` with that output as *both* the skeleton and `--out` (skeleton ==
/// out == the first round's output). In the second round `skeleton_dir ==
/// out_dir`, so the output-dir-relative path is resolved against that same
/// directory and rewritten absolute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ::clap::ValueEnum)]
pub enum CreateFileUrlMode {
    /// Treat a relative path as relative to the skeleton's directory and rebase
    /// it onto the output entry's directory, so the written entry names the same
    /// file by a relative path.
    Translate,

    /// Treat a relative path as relative to the skeleton's directory and write it
    /// as an absolute path.
    Absolutize,

    /// Keep a relative path verbatim (it is already relative to the output
    /// entry's directory).
    Keep,
}

/// Rewrite a `file:` URL into the form it takes in the finished entry, per `mode`
/// (see [`CreateFileUrlMode`]). A non-`file:` URL passes through; a `file:` URL
/// with no mode set is an error.
fn translate_fileurl(
    url: &str,
    mode: Option<CreateFileUrlMode>,
    skeleton_dir: &Path,
    out_dir: &Path,
) -> Result<String> {
    let Some(path) = url.strip_prefix("file:") else {
        return Ok(url.to_string());
    };
    let mode = mode.context(
        "skeleton uses a file: provider URL; pass --fileurl translate, absolutize, or keep to allow it",
    )?;
    if Path::new(path).is_absolute() || mode == CreateFileUrlMode::Keep {
        return Ok(url.to_string());
    }
    let target = skeleton_dir.join(path);
    let result: PathBuf = match mode {
        CreateFileUrlMode::Translate => rebase_path(out_dir, &target)?,
        CreateFileUrlMode::Absolutize => absolutize_path_components(&target)?.iter().collect(),
        CreateFileUrlMode::Keep => unreachable!("keep returns above"),
    };
    Ok(format!("file:{}", result.display()))
}

/// Fill in a skeleton entry: apply `replacements` (each `MARKER`/`VALUE`
/// pair replaces every literal occurrence of `MARKER` in paths, provider URLs,
/// and compose `strip` subpaths; `{{NAME}}` is the recommended marker style but
/// any string works),
/// download each provider to compute its size and blake3 digest, and set the
/// `final` flag. An artifact whose `size`, `hash`, and `digest` are all already
/// present in the skeleton is trusted as-is and not downloaded.
///
/// A `file:` provider URL is rewritten per `file_provider_mode` (see
/// [`CreateFileUrlMode`]; an unset mode rejects any `file:` URL) using
/// `skeleton_dir` and `out_dir`. For probing, each rewritten URL is resolved to
/// an absolute path against `out_dir`, then restored to its written form once the
/// digests are filled in - so [`Core`] only reads absolute `file:` URLs.
pub fn create_full(
    skeleton: XslashEntry,
    skeleton_dir: &Path,
    replacements: Vec<(String, String)>,
    file_provider_mode: Option<CreateFileUrlMode>,
    is_final: bool,
    out_dir: &Path,
) -> Result<XslashEntry> {
    let core = Core::from_env(false)?;
    let apply = |s: &str| {
        let mut s = s.to_string();
        for (marker, value) in &replacements {
            s = s.replace(marker, value);
        }
        s
    };

    // First pass: tag substitution over every string field - pure rewriting, no
    // network, so the entry is fully substituted before anything below.
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

    // Compute each URL's written form (applying the `--file` policy), then swap
    // in its absolute path - resolved against `out_dir`, the base for every mode
    // - so probing, and Core, only ever see absolute `file:` URLs.
    let mut final_urls: Vec<String> = Vec::new();
    rewrite_provider_urls(&mut entry, |url| {
        let final_url = translate_fileurl(url, file_provider_mode, skeleton_dir, out_dir)?;
        let absolute = absolutize_fileurl(&final_url, out_dir)?;
        final_urls.push(final_url);
        Ok(absolute)
    })?;

    // Probe each artifact to fill in its size/hash/digest, recording the entry in
    // the artifact's cache sidecar as it goes.
    core.probe(&mut entry)?;

    // Restore the finished form now that the digests are filled in.
    let mut finals = final_urls.into_iter();
    rewrite_provider_urls(&mut entry, |_| {
        finals
            .next()
            .context("provider URL count changed during probing")
    })?;

    Ok(entry)
}

/// Like [`create_full`], used for http(s) only entries.
pub fn create(
    skeleton: XslashEntry,
    replacements: Vec<(String, String)>,
    is_final: bool,
) -> Result<XslashEntry> {
    // With no mode set, no `file:` URL is accepted, so the directories are
    // never consulted.
    create_full(
        skeleton,
        Path::new("."),
        replacements,
        None,
        is_final,
        Path::new("."),
    )
}

/// Download (but do not extract) the artifacts for `entry`'s `platform_key`,
/// resolving relative `file:` URLs against `fileurl_base`. An empty
/// `platform_key` selects the host platform.
pub fn download_full(
    fileurl_base: &Path,
    entry: &XslashEntry,
    platform_key: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let core = Core::from_env(false)?;
    let mut entry = entry.clone();
    rewrite_provider_urls(&mut entry, |url| absolutize_fileurl(url, fileurl_base))?;
    core.ensure_downloaded(&entry, platform_key)
}

/// Like [`download_full`], used for http(s) only entries.
pub fn download(entry: &XslashEntry, platform_key: Option<&str>) -> Result<Vec<PathBuf>> {
    download_full(Path::new("."), entry, platform_key)
}

/// Download and extract `entry`'s host-platform artifacts, resolving relative
/// `file:` URLs against `fileurl_base`, and return the resolved executable path.
/// When `clean` is set, the downloaded artifacts are removed afterward, keeping
/// only the extracted files.
pub fn extract_full(fileurl_base: &Path, entry: &XslashEntry, clean: bool) -> Result<PathBuf> {
    let core = Core::from_env(false)?;
    let mut entry = entry.clone();
    rewrite_provider_urls(&mut entry, |url| absolutize_fileurl(url, fileurl_base))?;
    let exe = core.ensure_extracted(&entry)?;

    if clean {
        let (_, platform) = entry.select_platform(None)?;
        for src in platform.components()? {
            let path = core.cache()?.download_path(src.hash, src.digest);
            ::std::fs::remove_file(&path).ok();
            ::std::fs::remove_file(path.with_extension("json")).ok();
        }
    }
    Ok(exe)
}

/// Like [`extract_full`], used for http(s) only entries.
pub fn extract(entry: &XslashEntry, clean: bool) -> Result<PathBuf> {
    extract_full(Path::new("."), entry, clean)
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
