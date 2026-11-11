//! Project config discovery: walk the logical working directory upward for
//! `.xslashrc`/`xslash.json` configs, gated by the user's trust registry.

use ::anyhow::Context;
use ::anyhow::Result;
use ::std::collections::BTreeMap;
use ::std::collections::BTreeSet;
use ::std::path::Component;
use ::std::path::Path;
use ::std::path::PathBuf;

/// Logical-path handling, the trust registry, and the `.xslashrc`/
/// `xslash.json` config walk they gate.
pub struct Discovery {
    /// Suppress the stderr announcements (e.g. untrusted-config notices).
    pub silent: bool,
}

impl Discovery {
    /// The logical current directory.
    pub fn logical_cwd(&self) -> Result<PathBuf> {
        let mut cwd = ::std::env::current_dir().context("getting current directory")?;
        if let Some(pwd) = ::std::env::var_os("PWD").map(PathBuf::from)
            && pwd.is_absolute()
            && ::std::fs::canonicalize(&pwd).ok().as_deref() == Some(&cwd)
        {
            cwd = pwd;
        }
        Ok(cwd)
    }

    /// Absolutize `path` logically.
    pub fn logical_absolutize(&self, path: &Path) -> Result<PathBuf> {
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.logical_cwd()?.join(path)
        };
        let mut abs = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::ParentDir => {
                    abs.pop();
                }
                component => abs.push(component),
            }
        }
        Ok(abs)
    }

    /// Load the trust registry.
    pub fn trust_registry(&self) -> Result<(PathBuf, Option<BTreeSet<String>>)> {
        let config_home = match ::std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
            Some(xdg_config_home) => PathBuf::from(xdg_config_home),
            None => {
                let home = ::std::env::var_os("HOME")
                    .filter(|s| !s.is_empty())
                    .context("neither XDG_CONFIG_HOME nor HOME is set")?;
                PathBuf::from(home).join(".config")
            }
        };
        let trust_registry_path = config_home.join("xslash").join("trust");
        let trusted_paths: Option<BTreeSet<String>> =
            match ::std::fs::read_to_string(&trust_registry_path) {
                Ok(contents) => Some(
                    contents
                        .split('\n')
                        .filter(|l| !l.is_empty())
                        .map(str::to_string)
                        .collect(),
                ),
                Err(err) if err.kind() == ::std::io::ErrorKind::NotFound => None,
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("reading {}", trust_registry_path.display()));
                }
            };
        Ok((trust_registry_path, trusted_paths))
    }

    /// Whether the trust list covers `config_path`.
    pub fn check_trusted(&self, trusted_paths: &BTreeSet<String>, config_path: &Path) -> bool {
        let Some(config) = config_path.to_str() else {
            return false;
        };
        trusted_paths
            .iter()
            .any(|entry| match entry.ends_with('/') {
                true => config.starts_with(entry),
                false => config == entry,
            })
    }

    /// Collect every config (`.xslashrc` when present, `xslash.json`
    /// otherwise) on the path from the filesystem root down to `start`,
    /// ordered outermost (root) first and `start` last, along with the
    /// project dir: the nearest config's parent, or `None` when no config is
    /// found (in which case the config list is also empty).
    pub fn find_ancient_configs(&self, start: &Path) -> Result<(Option<PathBuf>, Vec<PathBuf>)> {
        let (_, trusted_paths) = self.trust_registry()?;
        let mut configs = Vec::new();
        let mut dir = Some(start);
        while let Some(d) = dir {
            let rc = d.join(".xslashrc");
            let candidate = match rc.is_file() {
                true => rc,
                false => d.join("xslash.json"),
            };
            if candidate.is_file() {
                if let Some(trusted_paths) = &trusted_paths
                    && !self.check_trusted(trusted_paths, &candidate)
                {
                    if !self.silent {
                        eprintln!("xslash: ignore {}", candidate.display());
                        eprintln!(
                            "xslash: trust with `xslash --trust .` or `xslash --trust {}`",
                            candidate.display()
                        );
                    }
                } else {
                    configs.push(candidate);
                }
            }
            dir = d.parent();
        }
        // We walked from `start` upward, so `configs[0]` is the nearest
        // (deepest) config; its parent is the project dir.
        let project_dir = configs
            .first()
            .and_then(|config| config.parent())
            .map(Path::to_path_buf);
        // Reverse so the outermost config comes first.
        configs.reverse();
        Ok((project_dir, configs))
    }

    /// Load a config file into its name -> entry-path map.
    pub fn load_config(&self, path: &Path) -> Result<BTreeMap<String, String>> {
        let contents = ::std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let map = match path.file_name().is_some_and(|f| f == ".xslashrc") {
            true => {
                let mut map = BTreeMap::new();
                for (i, line) in contents.split('\n').enumerate() {
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let (name, entry) = line.split_once(' ').with_context(|| {
                        format!(
                            "parsing {} line {}: no space separator",
                            path.display(),
                            i + 1
                        )
                    })?;
                    map.insert(name.to_string(), entry.to_string());
                }
                map
            }
            false => ::serde_json::from_str(&contents)
                .with_context(|| format!("parsing {}", path.display()))?,
        };
        Ok(map)
    }

    /// Search the trusted configs from the logical current directory up to
    /// the root for one that maps `name`, returning the resolved entry path.
    /// The nearest config to the current directory wins; outer configs act as
    /// a fallback for names a nearer config doesn't map.
    pub fn find_project_entry(&self, name: &str) -> Result<Option<PathBuf>> {
        let cwd = self.logical_cwd()?;
        let (_, configs) = self.find_ancient_configs(&cwd)?;
        // `configs` runs root -> cwd; scan from the nearest (deepest) config
        // upward and take the first that maps `name`.
        for config in configs.iter().rev() {
            let map = self.load_config(config)?;
            if let Some(rel) = map.get(name) {
                let dir = config.parent().unwrap_or_else(|| Path::new("."));
                return Ok(Some(dir.join(rel)));
            }
        }
        Ok(None)
    }
}
