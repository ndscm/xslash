//! Adjusting the permission bits of cache files, abstracted over platform.
//!
//! The mode transforms ([`Chmod::add_executable`], [`Chmod::remove_writable`])
//! are plain `u32 -> u32` maps applied to a path's bits by [`Chmod::chmod`] (one
//! path) or [`Chmod::chmod_recursive`] (a whole tree).

use ::anyhow::Context;
use ::anyhow::Result;
use ::std::path::Path;

/// Namespace for the permission-bit transforms and the functions that apply
/// them to cache files.
pub struct Chmod;

impl Chmod {
    /// Add the executable bit for each class (user, group, other) that already
    /// has the read bit. Mirroring read into execute keeps a file runnable by
    /// exactly whoever may read it.
    pub fn add_executable(mode: u32) -> u32 {
        // Read bits are 0o444, execute bits 0o111; shifting read right by 2
        // lands each class's read bit on its execute bit.
        mode | ((mode & 0o444) >> 2)
    }

    /// Clear the writable bit for every class (user, group, other), e.g. so a
    /// shared cache entry can't be mutated.
    pub fn remove_writable(mode: u32) -> u32 {
        mode & !0o222
    }

    /// Add the owner's writable bit, e.g. to make a locked-down tree removable
    /// again - unlinking a directory's contents needs write permission on it.
    pub fn add_writable(mode: u32) -> u32 {
        mode | 0o200
    }

    /// Clear the permission bits a crafted archive must not be able to set on a
    /// cache file: setuid/setgid and the group/other write bits. Owner and
    /// execute bits are left intact, so an extracted executable stays runnable
    /// and a mutable entry stays writable - a read-only entry's tree is locked
    /// down wholesale at publish time instead.
    pub fn sanitize(mode: u32) -> u32 {
        // setuid|setgid = 0o6000, group/other write = 0o022.
        mode & !0o6022
    }

    /// Rewrite `path`'s permission bits through `f`. Symlinks are left untouched.
    #[cfg(unix)]
    pub fn chmod(path: &Path, f: impl Fn(u32) -> u32) -> Result<()> {
        use ::std::os::unix::fs::PermissionsExt;

        let meta = ::std::fs::symlink_metadata(path)
            .with_context(|| format!("stat {}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Ok(());
        }

        let mode = meta.permissions().mode();
        let new = f(mode);
        if new != mode {
            ::std::fs::set_permissions(path, ::std::fs::Permissions::from_mode(new))
                .with_context(|| format!("chmod {}", path.display()))?;
        }
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn chmod(_path: &Path, _f: impl Fn(u32) -> u32) -> Result<()> {
        Ok(())
    }

    /// Rewrite the permission bits of `path` and everything under it through
    /// `f`. Symlinks are neither rewritten nor followed.
    #[cfg(unix)]
    pub fn chmod_recursive(path: &Path, f: impl Fn(u32) -> u32 + Copy) -> Result<()> {
        Self::chmod(path, f)?;
        // Recurse into real directories only; `chmod` already skipped symlinks.
        let meta = ::std::fs::symlink_metadata(path)
            .with_context(|| format!("stat {}", path.display()))?;
        if meta.is_dir() {
            for entry in
                ::std::fs::read_dir(path).with_context(|| format!("reading {}", path.display()))?
            {
                Self::chmod_recursive(&entry?.path(), f)?;
            }
        }
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn chmod_recursive(_path: &Path, _f: impl Fn(u32) -> u32 + Copy) -> Result<()> {
        Ok(())
    }
}
