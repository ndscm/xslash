//! The per-user cache home: resolving, downloading, extracting, and cleaning.
//!
//! ```text
//! CACHE_HOME/
//!   download/probe.json               probed provider URL -> hash/digest/size
//!   download/<hash>/<digest>          downloaded artifacts (+ <digest>.json)
//!   ro/<digest>/                         extracted shared entries (+ <digest>.json)
//!   rw/<digest>/                         extracted mutable entries (+ <digest>.json)
//!   hook/<digest>/<name>              per-directory PATH shims
//! ```
//!
//! Time-based cleanup (`clean <duration>`) treats each entry's access time as
//! its last use, refreshed explicitly on every cache hit. A shared entry in a
//! global cache may be owned by another user, so its refresh is best effort;
//! `noatime` mounts are unsupported, since there such an entry's atime never
//! advances and cleanup may reap it while still in use.

use ::anyhow::Context;
use ::anyhow::Result;
use ::serde::Deserialize;
use ::serde::Serialize;
use ::std::collections::BTreeMap;
use ::std::fs::File;
use ::std::hash::BuildHasher;
use ::std::hash::Hasher;
use ::std::hash::RandomState;
use ::std::io::Write;
use ::std::path::Path;
use ::std::path::PathBuf;
use ::std::sync::atomic::AtomicU64;
use ::std::sync::atomic::Ordering;
use ::std::time::Duration;
use ::std::time::Instant;
use ::std::time::SystemTime;

use crate::chmod::Chmod;
use crate::entry::Component;
use crate::entry::Platform;
use crate::entry::XslashEntry;
use crate::hash::Hash;
use crate::signal::TerminatingSignals;

/// Environment variable overriding the per-user cache home.
pub const XSLASH_CACHE_HOME: &str = "XSLASH_CACHE_HOME";
/// Environment variable overriding the shared (global) cache home.
pub const XSLASH_GLOBAL_CACHE_HOME: &str = "XSLASH_GLOBAL_CACHE_HOME";

fn default_cache_home() -> Result<PathBuf> {
    // Root shares one system-wide cache so artifacts it extracts are visible to
    // every user, rather than hiding under root's home.
    #[cfg(unix)]
    if ::rustix::process::getuid().is_root() {
        return Ok(PathBuf::from("/var/cache/xslash"));
    }

    // An explicit XDG_CACHE_HOME wins on every platform.
    if let Some(xdg_cache_home) = ::std::env::var_os("XDG_CACHE_HOME").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(xdg_cache_home).join("xslash"));
    }

    // Windows: the per-user, non-roaming cache lives under %LOCALAPPDATA%.
    #[cfg(windows)]
    {
        let local = ::std::env::var_os("LOCALAPPDATA")
            .filter(|s| !s.is_empty())
            .context("none of XSLASH_CACHE_HOME, XDG_CACHE_HOME, or LOCALAPPDATA is set")?;
        Ok(PathBuf::from(local).join("xslash"))
    }

    // macOS: caches live under ~/Library/Caches.
    #[cfg(target_os = "macos")]
    {
        let home = ::std::env::var_os("HOME")
            .filter(|s| !s.is_empty())
            .context("none of XSLASH_CACHE_HOME, XDG_CACHE_HOME, or HOME is set")?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("xslash"))
    }

    // Other unix: fall back to ~/.cache.
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let home = ::std::env::var_os("HOME")
            .filter(|s| !s.is_empty())
            .context("none of XSLASH_CACHE_HOME, XDG_CACHE_HOME, or HOME is set")?;
        Ok(PathBuf::from(home).join(".cache").join("xslash"))
    }
}

/// Remove `path`, whether it is a file or a directory tree.
fn remove_path(path: &Path) {
    // A published read-only entry has its write bits stripped; restore them
    // so each directory allows its contents to be unlinked.
    let _ = Chmod::chmod_recursive(path, Chmod::add_writable);
    if path.is_dir() {
        ::std::fs::remove_dir_all(path).ok();
    } else {
        ::std::fs::remove_file(path).ok();
    }
}

// ---------------------------------------------------------------------------
// lock
// ---------------------------------------------------------------------------

/// A held per-artifact lock, serializing a download or an extraction of the
/// same cache entry. Carries the final (`dest`), staging (`tmp`), and metadata
/// (`.json`) paths beside the `.lock` file; dropping it removes the lock file.
pub struct CacheLock {
    lock: PathBuf,

    /// Final home of the artifact, published from `tmp` on a successful release.
    pub dest: PathBuf,

    /// Staging path the caller writes to while holding the lock.
    pub tmp: PathBuf,

    /// The `.json` metadata sidecar beside `dest`, written by the caller.
    pub metadata: PathBuf,

    /// Slot in the signal-cleanup table tracking this lock's `.lock` file, so a
    /// fatal signal can unlink it; `None` when tracking wasn't available.
    slot: Option<usize>,
}

impl Drop for CacheLock {
    /// Release the lock by removing its file when the guard goes out of scope.
    fn drop(&mut self) {
        let _ = ::std::fs::remove_file(&self.lock);
        // Stop signal-tracking after unlinking: a handler firing in between just
        // re-unlinks an already-gone path, which is harmless.
        if let Some(slot) = self.slot {
            TerminatingSignals::unregister_unlink(slot);
        }
    }
}

impl CacheLock {
    /// Lock the artifact at `dest`, whose staging (`.tmp`), metadata (`.json`),
    /// and `.lock` paths are its siblings. Creates `dest`'s parent, then blocks
    /// until the `.lock` file is ours.
    ///
    /// The returned `bool` reports whether `dest` already exists now that the
    /// lock is held (another holder may have produced it while we waited);
    /// dropping the guard removes the lock file.
    fn acquire(dest: PathBuf) -> Result<(bool, CacheLock)> {
        let parent = dest.parent().context("cache path has no parent")?;
        ::std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;

        let lock = dest.with_extension("lock");
        let tmp = dest.with_extension("tmp");
        let metadata = dest.with_extension("json");

        // Block until we can atomically create the lock file, polling every
        // 100ms and warning once after 60s so a stale lock is noticeable. The
        // lock is held until the file is removed by this guard's `Drop`.
        let start = Instant::now();
        let mut warned = false;
        loop {
            match ::std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock)
            {
                Ok(_) => break,
                Err(err) if err.kind() == ::std::io::ErrorKind::AlreadyExists => {
                    if !warned && start.elapsed() >= Duration::from_secs(60) {
                        eprintln!(
                            "waiting for lock at {}, please remove stale lock if necessary",
                            lock.display()
                        );
                        warned = true;
                    }
                    ::std::thread::sleep(Duration::from_millis(100));
                }
                Err(err) => {
                    return Err(err).with_context(|| format!("creating {}", lock.display()));
                }
            }
        }

        // Track the now-held lock so a fatal signal can unlink it.
        let slot = TerminatingSignals::register_unlink(&lock);

        let exists = dest.exists();
        Ok((
            exists,
            CacheLock {
                lock,
                dest,
                tmp,
                metadata,
                slot,
            },
        ))
    }

    /// Release the lock. When `finalize`, the staged `tmp` (a file for a
    /// download, a directory for an extraction) is published into its `dest`
    /// home; otherwise `tmp` is discarded, leaving `dest` untouched. The lock
    /// file is removed when `self` is dropped at the end of this call. Writing
    /// the `.json` sidecar is left to the caller, which holds the
    /// [`metadata`](CacheLock::metadata) path.
    ///
    /// A read-only entry (`!rw`) is stripped of its write bits recursively
    /// before being published, so the shared copy can't be mutated in place.
    pub fn release(self, finalize: bool, rw: bool) -> Result<()> {
        if !finalize {
            remove_path(&self.tmp);
            return Ok(());
        }
        match ::std::fs::rename(&self.tmp, &self.dest) {
            // Lock the published tree down in place for a read-only entry. Done
            // after the rename so `tmp`'s own cleared write bit can't block the
            // move (renaming into a different parent would need it).
            Ok(()) if !rw => Chmod::chmod_recursive(&self.dest, Chmod::remove_writable)?,
            Ok(()) => {}
            // Lost a race with a concurrent holder; theirs is fine.
            Err(_) if self.dest.exists() => {
                remove_path(&self.tmp);
            }
            Err(err) => {
                remove_path(&self.tmp);
                return Err(err).with_context(|| {
                    format!("moving {} to {}", self.tmp.display(), self.dest.display())
                });
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// cache
// ---------------------------------------------------------------------------

/// A cached probe result for a provider URL: the artifact's hash algorithm,
/// digest, and size. Lets `create` reuse an earlier probe of the same URL
/// instead of downloading it again to recompute the digest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Probe {
    /// Hash algorithm used for `digest`.
    pub hash: Hash,

    /// Digest of the artifact fetched from the URL, in lowercase hex.
    pub digest: String,

    /// Size of the artifact in bytes.
    pub size: u64,
}

/// The resolved cache locations.
pub struct Cache {
    /// Suppress xslash's messages.
    silent: bool,

    /// The user's own cache home; writable.
    pub home: PathBuf,

    /// The shared cache home; treated as read-only. Only consulted when
    /// explicitly configured via [`XSLASH_GLOBAL_CACHE_HOME`]; there is no
    /// default location.
    pub global: Option<PathBuf>,
}

impl Cache {
    /// Resolve the cache homes from the environment, with documented defaults.
    pub fn from_env(silent: bool) -> Result<Cache> {
        let home = match ::std::env::var_os(XSLASH_CACHE_HOME) {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => default_cache_home()?,
        };
        let global = match ::std::env::var_os(XSLASH_GLOBAL_CACHE_HOME) {
            Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
            _ => None,
        };
        Ok(Cache {
            silent,
            home,
            global,
        })
    }

    fn download_rel(hash: Hash, digest: &str) -> PathBuf {
        Path::new("download").join(hash.as_str()).join(digest)
    }

    /// Path to a downloaded artifact in the user's cache home.
    pub fn download_path(&self, hash: Hash, digest: &str) -> PathBuf {
        self.home.join(Self::download_rel(hash, digest))
    }

    /// A verified copy of `src` already in the download cache (global first,
    /// then local); or `None` when absent.
    pub fn downloaded_path(&self, component: &Component<'_>) -> Option<PathBuf> {
        let rel = Self::download_rel(component.hash, component.digest);

        if let Some(global_home) = &self.global {
            let global = global_home.join(&rel);
            if global.is_file()
                && component
                    .hash
                    .verify_file(&global, component.digest, Some(component.size))
                    .is_ok()
            {
                let _ = filetime::set_file_atime(&global, filetime::FileTime::now());
                return Some(global);
            }
        }

        let local = self.home.join(&rel);
        if local.is_file()
            && component
                .hash
                .verify_file(&local, component.digest, Some(component.size))
                .is_ok()
        {
            let _ = filetime::set_file_atime(&local, filetime::FileTime::now());
            return Some(local);
        }

        None
    }

    /// Acquire the per-artifact download lock, waiting for any existing
    /// `<digest>.lock` to disappear before atomically creating our own.
    ///
    /// The returned `bool` reports whether `dest` already exists now that the
    /// lock is held (another holder may have downloaded it while we waited). If
    /// so, the caller should skip the download and release the lock at once.
    pub fn acquire_download_lock(&self, hash: Hash, digest: String) -> Result<(bool, CacheLock)> {
        // An empty digest means the caller is staging an artifact whose digest
        // isn't known yet; give it a throwaway random name - a 16-char hex token
        // seeded from the OS and salted with the pid and a counter so concurrent
        // callers differ.
        let digest = if digest.is_empty() {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u32(::std::process::id());
            hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
            format!("{:016x}", hasher.finish())
        } else {
            digest
        };
        CacheLock::acquire(self.download_path(hash, &digest))
    }

    /// Path to the probe cache in the user's download area.
    fn probe_path(&self) -> PathBuf {
        self.home.join("download").join("probe.json")
    }

    /// Look up a previously-probed provider `url`, returning its recorded
    /// hash/digest/size when present. A missing or unreadable cache is treated
    /// as a miss.
    pub fn get_probe(&self, url: &str) -> Option<Probe> {
        let map: BTreeMap<String, Probe> = ::std::fs::read(self.probe_path())
            .ok()
            .and_then(|bytes| ::serde_json::from_slice(&bytes).ok())?;
        map.get(url).cloned()
    }

    /// Record `probe` for provider `url`, merging it into `download/probe.json`
    /// under the `download/probe.lock` so concurrent writers don't clobber each
    /// other. Holding the lock guard across the read-modify-write serializes it;
    /// dropping the guard releases the lock.
    pub fn update_probe(&self, url: &str, probe: &Probe) -> Result<()> {
        let path = self.probe_path();
        let (_, _lock) = CacheLock::acquire(path.clone())?;
        let mut map: BTreeMap<String, Probe> = ::std::fs::read(&path)
            .ok()
            .and_then(|bytes| ::serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        map.insert(url.to_string(), probe.clone());
        let mut file =
            File::create(&path).with_context(|| format!("writing {}", path.display()))?;
        ::serde_json::to_writer_pretty(&mut file, &map).context("serializing probe cache")?;
        file.write_all(b"\n")
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    fn extract_dir(&self, rw: bool, digest: &str) -> PathBuf {
        self.home.join(if rw { "rw" } else { "ro" }).join(digest)
    }

    /// The resolved executable if `platform`'s extraction directory already
    /// exists (global ro first, then local); else `None`.
    pub fn extracted_dir(&self, platform: &Platform) -> Result<Option<PathBuf>> {
        let digest = platform.extract_digest()?;

        // Prefer a globally-extracted ro entry, run directly without copying.
        if !platform.is_rw()
            && let Some(global_home) = &self.global
        {
            let global = global_home.join("ro").join(&digest);
            if global.is_dir() {
                let _ = filetime::set_file_atime(&global, filetime::FileTime::now());
                return Ok(Some(global.join(platform.exepath()?)));
            }
        }

        let dir = self.extract_dir(platform.is_rw(), &digest);
        if dir.is_dir() {
            let _ = filetime::set_file_atime(&dir, filetime::FileTime::now());
            return Ok(Some(dir.join(platform.exepath()?)));
        }

        Ok(None)
    }
    /// Acquire the per-extraction lock for `key` (under `rw/` when `rw`, else
    /// `ro/`), waiting for any existing `<key>.lock` to disappear before
    /// atomically creating our own.
    ///
    /// The returned `bool` reports whether `dest` already exists now that the
    /// lock is held (another holder may have extracted it while we waited). If
    /// so, the caller should skip extraction and release the lock at once.
    pub fn acquire_extract_lock(&self, rw: bool, digest: &str) -> Result<(bool, CacheLock)> {
        CacheLock::acquire(self.extract_dir(rw, digest))
    }

    /// Remove the entry at `path` (file or tree) and its `.json` sidecar,
    /// announcing `xslash: clean <path>` unless [`silent`](Self::silent).
    fn clean_entry(&self, path: &Path) {
        if !self.silent {
            eprintln!("xslash: clean {}", path.display());
        }
        remove_path(path);
        ::std::fs::remove_file(path.with_extension("json")).ok();
    }

    /// Remove `entry`'s host-platform downloaded artifacts from the cache.
    pub fn clean_downloaded_entry(&self, entry: &XslashEntry) -> Result<()> {
        let (_, platform) = entry.select_platform(None)?;
        for src in platform.components()? {
            self.clean_entry(&self.download_path(src.hash, src.digest));
        }
        Ok(())
    }

    /// Remove `entry`'s host-platform extracted files from the cache.
    pub fn clean_extracted_entry(&self, entry: &XslashEntry) -> Result<()> {
        let (_, platform) = entry.select_platform(None)?;
        let key = platform.extract_digest()?;
        for sub in ["ro", "rw"] {
            self.clean_entry(&self.home.join(sub).join(&key));
        }
        Ok(())
    }

    /// Remove each entry directly under `bucket` whose last access time is
    /// before `before`, along with its `.json` sidecar. Only the digest-named,
    /// extensionless children are considered, so the `.json`/`.lock`/`.tmp`
    /// siblings beside them are left alone (a sidecar is removed only with its
    /// entry).
    ///
    /// Liveness is judged by the entry's own atime - the download file or the
    /// extraction directory, the same path keyed by its digest, not any file
    /// within. [`Cache::downloaded_path`] and [`Cache::extracted_dir`] refresh
    /// that atime on every cache hit (a directory's atime doesn't otherwise
    /// advance when a binary inside it runs).
    fn clean_before(&self, bucket: &Path, before: SystemTime) {
        let Ok(entries) = ::std::fs::read_dir(bucket) else {
            return;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if path.extension().is_some() {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            // Default to `before` when the time is unavailable so the entry is kept.
            let accessed = meta
                .accessed()
                .or_else(|_| meta.modified())
                .unwrap_or(before);
            if accessed < before {
                self.clean_entry(&path);
            }
        }
    }

    /// Remove downloaded artifacts whose last access time is before `before`.
    pub fn clean_downloaded_before(&self, before: SystemTime) -> Result<()> {
        let base = self.home.join("download");
        if let Ok(entries) = ::std::fs::read_dir(&base) {
            for algo in entries.flatten() {
                self.clean_before(&algo.path(), before);
            }
        }
        Ok(())
    }

    /// Remove extracted files whose last access time is before `before`.
    pub fn clean_extracted_before(&self, before: SystemTime) -> Result<()> {
        for sub in ["ro", "rw"] {
            self.clean_before(&self.home.join(sub), before);
        }
        Ok(())
    }

    /// The per-directory shim dir for a project rooted at `project_dir`.
    pub fn hook_dir(&self, config_dir: &Path) -> Result<PathBuf> {
        let digest = Hash::Blake3.hash_bytes(config_dir.as_os_str().as_encoded_bytes())?;
        Ok(self.home.join("hook").join(digest))
    }
}
