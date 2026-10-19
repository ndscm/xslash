//! The glue between [`crate::http`] (network) and [`crate::cache`] (local fs):
//! fetching artifacts into the cache and resolving an entry's executable.

use ::anyhow::Context;
use ::anyhow::Result;
use ::anyhow::anyhow;
use ::anyhow::bail;
use ::std::collections::BTreeMap;
use ::std::fs::File;
use ::std::io::BufReader;
use ::std::io::Cursor;
use ::std::io::Read;
use ::std::io::Write;
use ::std::path::Path;
use ::std::path::PathBuf;

use crate::cache::Cache;
use crate::cache::Probe;
use crate::chmod::Chmod;
use crate::entry::Component;
use crate::entry::Format;
use crate::entry::Platform;
use crate::entry::Provider;
use crate::entry::XslashEntry;
use crate::hash::Hash;
use crate::http::Http;

/// Merge `entry` into the name-keyed metadata sidecar at `path`, preserving any
/// other names that already share this cache artifact and writing the names
/// sorted. This is the schema shared by downloaded and extracted sidecars.
fn merge_metadata(path: &Path, entry: &XslashEntry) -> Result<()> {
    let mut map: BTreeMap<String, XslashEntry> = ::std::fs::read(path)
        .ok()
        .and_then(|bytes| ::serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    map.insert(entry.name.clone(), entry.clone());
    let mut file = File::create(path)?;
    ::serde_json::to_writer_pretty(&mut file, &map).context("serializing cache metadata")?;
    file.write_all(b"\n")?;
    Ok(())
}

fn copy_file(file: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        ::std::fs::create_dir_all(parent)?;
    }
    ::std::fs::copy(file, target).with_context(|| format!("copying to {}", target.display()))?;
    Chmod::chmod(target, Chmod::add_executable)?;
    Ok(())
}

fn write_file(target: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = target.parent() {
        ::std::fs::create_dir_all(parent)?;
    }
    ::std::fs::write(target, bytes).with_context(|| format!("writing {}", target.display()))?;
    Chmod::chmod(target, Chmod::add_executable)?;
    Ok(())
}

fn decompress_to_file<R: Read>(mut reader: R, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        ::std::fs::create_dir_all(parent)?;
    }
    let mut out = File::create(target).with_context(|| format!("creating {}", target.display()))?;
    ::std::io::copy(&mut reader, &mut out)
        .with_context(|| format!("writing {}", target.display()))?;
    Chmod::chmod(target, Chmod::add_executable)?;
    Ok(())
}

fn xz_decode(file: &Path) -> Result<Vec<u8>> {
    let mut reader = BufReader::new(File::open(file)?);
    let mut out = Vec::new();
    ::lzma_rs::xz_decompress(&mut reader, &mut out).context("xz decompress")?;
    Ok(out)
}

fn unpack_tar<R: Read>(reader: R, strip: &str, target: &Path) -> Result<()> {
    ::std::fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    if strip.is_empty() {
        ::tar::Archive::new(reader)
            .unpack(target)
            .with_context(|| format!("unpacking tar into {}", target.display()))?;
        Chmod::chmod_recursive(target, Chmod::sanitize)
            .with_context(|| format!("sanitizing modes under {}", target.display()))?;
        return Ok(());
    }
    let prefix = Path::new(strip);
    // Rebase onto `target`: keep only entries under `prefix`, drop that prefix,
    // and skip everything else (the archive's installer metadata and sibling
    // components).
    let mut archive = ::tar::Archive::new(reader);
    for entry in archive.entries().context("reading tar entries")? {
        let mut entry = entry.context("reading tar entry")?;
        let path = entry.path().context("reading tar entry path")?.into_owned();
        let Ok(rel) = path.strip_prefix(prefix) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }

        // Reject path traversal: a hostile entry (one matching the pinned digest)
        // could strip to a `rel` that escapes `target` once joined. `unpack` here
        // bypasses `tar::Archive::unpack`'s built-in guard, so enforce it - the
        // zip branch gets this for free via `enclosed_name()`.
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, ::std::path::Component::ParentDir))
        {
            continue;
        }

        let dst = target.join(rel);
        if let Some(parent) = dst.parent() {
            ::std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        entry
            .unpack(&dst)
            .with_context(|| format!("unpacking {} into {}", path.display(), dst.display()))?;
    }
    // Mask modes a crafted archive may carry: setuid/setgid and group/other write.
    Chmod::chmod_recursive(target, Chmod::sanitize)
        .with_context(|| format!("sanitizing modes under {}", target.display()))
}

fn unpack_zip(file: &Path, strip: &str, target: &Path) -> Result<()> {
    ::std::fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    let mut archive = ::zip::ZipArchive::new(File::open(file)?).context("opening zip")?;
    if strip.is_empty() {
        archive
            .extract(target)
            .with_context(|| format!("extracting zip into {}", target.display()))?;
        Chmod::chmod_recursive(target, Chmod::sanitize)
            .with_context(|| format!("sanitizing modes under {}", target.display()))?;
        return Ok(());
    }
    let prefix = Path::new(strip);
    for i in 0..archive.len() {
        let mut zf = archive.by_index(i).context("reading zip entry")?;
        let Some(name) = zf.enclosed_name() else {
            continue;
        };
        let Ok(rel) = name.strip_prefix(prefix) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let dst = target.join(rel);
        if zf.is_dir() {
            ::std::fs::create_dir_all(&dst)
                .with_context(|| format!("creating {}", dst.display()))?;
            continue;
        }
        if let Some(parent) = dst.parent() {
            ::std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut out = File::create(&dst).with_context(|| format!("creating {}", dst.display()))?;
        ::std::io::copy(&mut zf, &mut out).with_context(|| format!("writing {}", dst.display()))?;
        #[cfg(unix)]
        if let Some(mode) = zf.unix_mode() {
            use ::std::os::unix::fs::PermissionsExt;
            ::std::fs::set_permissions(&dst, ::std::fs::Permissions::from_mode(mode))
                .with_context(|| format!("setting mode on {}", dst.display()))?;
        }
    }
    // Mask modes a crafted archive may carry: setuid/setgid and group/other write.
    Chmod::chmod_recursive(target, Chmod::sanitize)
        .with_context(|| format!("sanitizing modes under {}", target.display()))
}

/// Extract (or copy) one source into `into`. When `strip` is set for an
/// archive, only the entries under that leading in-archive subpath are
/// extracted, with the prefix dropped; it is ignored for single-file formats.
/// The result is placed at `dest` (a relative subpath, or the root when empty).
fn extract_component(
    file: &Path,
    format: Option<Format>,
    strip: &str,
    into: &Path,
    dest: &str,
) -> Result<()> {
    let target = if dest.is_empty() {
        into.to_path_buf()
    } else {
        into.join(dest)
    };

    match format {
        None => copy_file(file, &target)?,
        Some(Format::Tar) => unpack_tar(File::open(file)?, strip, &target)?,
        Some(Format::TarGz) => unpack_tar(
            ::flate2::read::GzDecoder::new(File::open(file)?),
            strip,
            &target,
        )?,
        Some(Format::TarXz) => unpack_tar(Cursor::new(xz_decode(file)?), strip, &target)?,
        Some(Format::TarZst) => unpack_tar(
            ::ruzstd::decoding::StreamingDecoder::new(File::open(file)?)?,
            strip,
            &target,
        )?,
        Some(Format::Gz) => {
            decompress_to_file(::flate2::read::GzDecoder::new(File::open(file)?), &target)?
        }
        Some(Format::Xz) => write_file(&target, &xz_decode(file)?)?,
        Some(Format::Zst) => decompress_to_file(
            ::ruzstd::decoding::StreamingDecoder::new(File::open(file)?)?,
            &target,
        )?,
        Some(Format::Zip) => unpack_zip(file, strip, &target)?,
    }
    Ok(())
}

/// The network + cache pair that drives all fetching. Construct one with
/// [`Core::from_env`] and call its methods rather than threading `http`/`cache`
/// through every function.
pub struct Core {
    /// HTTP client for cache-server and provider requests.
    pub http: Http,

    /// Local cache homes where artifacts are downloaded and extracted.
    pub cache: Cache,
}

impl Core {
    /// Resolve the HTTP client and cache homes from the environment.
    pub fn from_env(silent: bool) -> Result<Core> {
        Ok(Core {
            http: Http::from_env(silent),
            cache: Cache::from_env(silent)?,
        })
    }

    /// Download `src` into `tmp`, trying the cache server first and then each
    /// provider in order, verifying each attempt against `src`'s hash, digest,
    /// and size. Returns whether the bytes came from a provider (and so should
    /// be PUT back to the cache server).
    fn http_download(&self, src: &Component, tmp: &Path) -> Result<bool> {
        if self
            .http
            .cache_server_get(src.hash.as_str(), src.digest, tmp)
            && src
                .hash
                .verify_file(tmp, src.digest, Some(src.size))
                .is_ok()
        {
            return Ok(false);
        }
        let _ = ::std::fs::remove_file(tmp);

        let mut last_err = None;
        for provider in src.providers {
            let Some(provider_url) = provider.url.as_deref() else {
                continue;
            };
            match self
                .http
                .get(provider_url, tmp)
                .and_then(|()| src.hash.verify_file(tmp, src.digest, Some(src.size)))
            {
                Ok(()) => return Ok(true),
                Err(err) => {
                    let _ = ::std::fs::remove_file(tmp);
                    last_err = Some(err.context(format!("provider {provider_url}")));
                }
            }
        }
        match last_err {
            Some(err) => Err(err).context(format!("downloading {}", src.digest)),
            None => bail!("no providers listed for {}", src.digest),
        }
    }

    /// Download `entry`'s artifacts for `platform_key` into the cache (without
    /// extracting), returning the path of each. An empty `platform_key` selects
    /// the host platform. Reuses cached copies.
    ///
    /// Each artifact is fetched by trying the cache server first, then each
    /// provider in order; a successful provider fetch is PUT back to the cache
    /// server, and the entry is merged (by name) into the artifact's `.json`
    /// sidecar. Downloads of a given artifact are serialized by a lock, which is
    /// held across the sidecar update.
    pub fn ensure_downloaded(
        &self,
        entry: &XslashEntry,
        platform_key: Option<&str>,
    ) -> Result<Vec<PathBuf>> {
        let (key, platform) = entry.select_platform(platform_key)?;
        let metadata = XslashEntry {
            name: entry.name.clone(),
            is_final: entry.is_final,
            platforms: BTreeMap::from([(key, platform.clone())]),
        };

        let mut paths = Vec::new();
        for src in platform.components()? {
            if let Some(path) = self.cache.downloaded_path(&src) {
                paths.push(path);
                continue;
            }

            // Serialize downloads of this artifact.
            let (downloaded, lock) = self
                .cache
                .acquire_download_lock(src.hash, src.digest.to_string())?;

            // Another holder may have published it while we waited; trust it
            // only if it still verifies - a corrupt or truncated copy is removed
            // and re-downloaded below.
            if downloaded
                && src
                    .hash
                    .verify_file(&lock.dest, src.digest, Some(src.size))
                    .is_ok()
            {
                let dest = lock.dest.clone();
                lock.release(false, true)?;
                paths.push(dest);
                continue;
            }
            let _ = ::std::fs::remove_file(&lock.dest);

            // Download into the lock's staging path. The bool tracks whether to
            // PUT back to the server.
            let from_provider = match self.http_download(&src, &lock.tmp) {
                Ok(from_provider) => from_provider,
                Err(err) => {
                    lock.release(false, true)?;
                    return Err(err);
                }
            };
            let dest = lock.dest.clone();

            // Merge our entry into the `.json` sidecar under its name, while the
            // lock is still held so concurrent downloaders of this digest don't
            // clobber each other.
            merge_metadata(&lock.metadata, &metadata).ok();

            lock.release(true, true)?;
            if from_provider {
                self.http
                    .cache_server_put(src.hash.as_str(), src.digest, &dest);
            }
            paths.push(dest);
        }
        Ok(paths)
    }

    /// Extract the already-downloaded source `files` (parallel to
    /// `platform.components()`) into `entry`'s extract dir, returning the
    /// resolved executable path.
    fn extract(
        &self,
        entry: &XslashEntry,
        platform_key: &str,
        platform: &Platform,
        files: &[PathBuf],
    ) -> Result<PathBuf> {
        // Fail fast before touching the filesystem if the executable path is
        // missing; every extracted entry runs from `<dir>/<path>`.
        let exepath = platform.exepath()?;
        let digest = platform.extract_digest()?;

        // Serialize extractions of this entry; the lock is removed when dropped.
        let (extracted, lock) = self.cache.acquire_extract_lock(platform.is_rw(), &digest)?;

        // Another holder may have extracted it while we waited for the lock.
        if extracted {
            let exe = lock.dest.join(exepath);
            lock.release(false, platform.is_rw())?;
            Chmod::chmod(&exe, Chmod::add_executable)?;
            return Ok(exe);
        }

        // Extract into the lock's staging dir; releasing moves it into place.
        if lock.tmp.exists() {
            ::std::fs::remove_dir_all(&lock.tmp).ok();
        }
        ::std::fs::create_dir_all(&lock.tmp)
            .with_context(|| format!("creating {}", lock.tmp.display()))?;

        let outcome = (|| {
            for (src, file) in platform.components()?.iter().zip(files) {
                extract_component(file, src.format, src.strip, &lock.tmp, src.dest)
                    .with_context(|| format!("extracting {}", src.digest))?;
            }
            Ok(())
        })();
        if let Err(err) = outcome {
            lock.release(false, platform.is_rw())?;
            return Err(err);
        }

        // Write the sidecar while the lock is still held, then move into place -
        // the same name-keyed schema as the download sidecar.
        let metadata = XslashEntry {
            name: entry.name.clone(),
            is_final: entry.is_final,
            platforms: BTreeMap::from([(platform_key.to_string(), platform.clone())]),
        };
        merge_metadata(&lock.metadata, &metadata).ok();
        let exe = lock.dest.join(exepath);
        lock.release(true, platform.is_rw())?;
        Chmod::chmod(&exe, Chmod::add_executable)?;
        Ok(exe)
    }

    /// Ensure `entry`'s host-platform executable is fetched into the cache home,
    /// downloading and extracting only what's missing, and return the path to
    /// run.
    ///
    /// Each extracting branch checks the cache first: if the entry is already
    /// extracted (its `<key>` dir/file exists in the cache home), it returns
    /// that without any network access.
    pub fn ensure_extracted(&self, entry: &XslashEntry) -> Result<PathBuf> {
        let (key, platform) = entry.select_platform(None)?;

        // Every entry extracts into a directory - a raw binary and a single-file
        // compressor are placed at `path` within it just as an archive is
        // unpacked into it - so there is a single extraction path.
        if let Some(exe) = self.cache.extracted_dir(platform)? {
            return Ok(exe);
        }

        let files = self.ensure_downloaded(entry, None)?;
        self.extract(entry, &key, platform, &files)
    }

    /// Download the first working provider, compute its size and blake3 digest,
    /// and store it in the download cache (also PUTting it to the cache server).
    /// Drives [`probe`](Self::probe).
    ///
    /// A provider URL already in the [probe cache](Cache::get_probe) short-
    /// circuits the download, returning the recorded size and digest. Otherwise
    /// the fresh result is recorded there before returning.
    ///
    /// Writing the artifact's `.json` sidecar is left to [`probe`](Self::probe),
    /// which has the fully-resolved entry.
    fn probe_component(&self, providers: &[Provider]) -> Result<(u64, String)> {
        // The digest isn't known until the artifact is downloaded, so stage it
        // under a throwaway lock (empty digest), then settle it under the real
        // digest lock.
        let (_, staging) = self
            .cache
            .acquire_download_lock(Hash::Unknown, String::new())?;
        let mut last_err = None;
        for provider in providers {
            // Only http(s) URL providers are supported.
            match provider.provider_type.as_deref() {
                None | Some("http") => {}
                Some(other) => {
                    last_err = Some(anyhow!("unsupported provider type: {other}"));
                    continue;
                }
            }
            let Some(provider_url) = provider.url.as_deref() else {
                continue;
            };

            // Reuse an earlier probe of this URL rather than downloading it
            // again just to recompute the digest.
            if let Some(probe) = self.cache.get_probe(provider_url) {
                staging.release(false, true)?;
                return Ok((probe.size, probe.digest));
            }

            if let Err(err) = self.http.get(provider_url, &staging.tmp) {
                last_err = Some(err.context(format!("provider {provider_url}")));
                continue;
            }

            let size = ::std::fs::metadata(&staging.tmp)
                .with_context(|| format!("stat {}", staging.tmp.display()))?
                .len();
            let digest = Hash::Blake3.hash_file(&staging.tmp)?;

            let (downloaded, lock) = self
                .cache
                .acquire_download_lock(Hash::Blake3, digest.clone())?;
            if downloaded {
                // Another holder already cached it; our staging copy is dropped
                // with the staging lock below.
                lock.release(false, true)?;
            } else {
                let moved = ::std::fs::rename(&staging.tmp, &lock.tmp).with_context(|| {
                    format!("moving {} to {}", staging.tmp.display(), lock.tmp.display())
                });
                if let Err(err) = moved {
                    // Release both locks (dropping the staging `.tmp`) before
                    // bailing so a failed move doesn't leak the staging copy.
                    lock.release(false, true)?;
                    staging.release(false, true)?;
                    return Err(err);
                }
                let dest = lock.dest.clone();
                lock.release(true, true)?;
                self.http
                    .cache_server_put(Hash::Blake3.as_str(), &digest, &dest);
            }

            // Record this URL's result so a later probe can skip the download.
            self.cache
                .update_probe(
                    provider_url,
                    &Probe {
                        hash: Hash::Blake3,
                        digest: digest.clone(),
                        size,
                    },
                )
                .ok();
            staging.release(false, true)?;
            return Ok((size, digest));
        }
        staging.release(false, true)?;
        Err(last_err.unwrap_or_else(|| anyhow!("no providers listed")))
    }

    /// Probe every platform of `entry`, downloading each artifact to compute
    /// and fill in its `size`/`hash`/`digest` in place, and recording the entry
    /// in each artifact's `.json` cache sidecar. Used by `create` to fill in
    /// skeleton entries whose digests aren't yet known.
    ///
    /// A platform is handled in two passes because a compose platform's
    /// component digests aren't all known until every component is probed: first
    /// probe each artifact and fill in its digest, then - once the whole
    /// platform is resolved - write the complete entry into every artifact's
    /// sidecar.
    pub fn probe(&self, entry: &mut XslashEntry) -> Result<()> {
        let name = entry.name.clone();
        for (platform_key, platform) in entry.platforms.iter_mut() {
            // First pass: probe each of the platform's artifacts, filling in its
            // size/hash/digest. A compose platform has several, a single-artifact
            // platform has one, and a name-only platform has none to probe.
            if let Some(compose) = platform.compose.as_mut() {
                for c in compose.iter_mut() {
                    let (size, digest) = self.probe_component(&c.providers)?;
                    c.size = Some(size);
                    c.hash = Some(Hash::Blake3);
                    c.digest = Some(digest);
                }
            } else if let Some(providers) = platform.providers.as_deref() {
                let (size, digest) = self.probe_component(providers)?;
                platform.hash = Some(Hash::Blake3);
                platform.digest = Some(digest);
                platform.size = Some(size);
            } else {
                continue;
            }

            // Second pass: the platform is fully resolved now, so record the
            // complete entry in each artifact's `.json` sidecar - held under the
            // artifact's download lock so concurrent writers of the same digest
            // don't clobber each other.
            let entry = XslashEntry {
                name: name.clone(),
                // The cache sidecar records the artifact, not a resolution
                // target, so it's never final.
                is_final: false,
                platforms: BTreeMap::from([(platform_key.clone(), platform.clone())]),
            };
            for src in platform.components()? {
                let (_, lock) = self
                    .cache
                    .acquire_download_lock(src.hash, src.digest.to_string())?;
                merge_metadata(&lock.metadata, &entry).ok();
                lock.release(false, true)?;
            }
        }
        Ok(())
    }
}
