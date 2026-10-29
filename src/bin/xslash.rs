//! The xslash command-line interface.
//!
//! The common invocation is `xslash <entry>`, so every subcommand is placed
//! after a `--` separator to keep it from being mistaken for an entry path.
//!
//! This layer is deliberately thin: it parses arguments and does file I/O,
//! then delegates the real work to the `xslash` library functions. The
//! cache-management subcommands after `--` are handed off wholesale to the
//! `xslash-helper` binary.

use ::anyhow::Context;
use ::anyhow::Result;
use ::anyhow::bail;
use ::std::collections::BTreeMap;
use ::std::collections::BTreeSet;
use ::std::ffi::OsStr;
use ::std::ffi::OsString;
use ::std::path::Path;
use ::std::path::PathBuf;
use ::std::process::Command;
use ::std::process::ExitCode;

use ::xslash::Cache;
use ::xslash::Chmod;
use ::xslash::Hash;
use ::xslash::XslashEntry;

const HELP: &str = "\
xslash - simplified distribution of prebuilt executables

Usage:
  xslash <entry> [args...]          Run the prebuilt executable the entry defines
  xslash -- <command> [options]     Run a helper command (see below)
  xslash --export [--silent]        Generate PATH-sync code for shell to `eval`
  xslash --hook [--silent]          Generate code to hook `--export` for interactive shells
  xslash --help                     Print this help
  xslash --version                  Print the build version

Helper commands (after `--`):
  create   --skeleton <entry> [--replace {{TAG}}=x.y.z]... [--final] --out <path> [--symlink <name>]...
  download [--all] <entry>
  extract  [--clean] <entry>
  clean    [--downloaded|--extracted] <entry>|<duration>
";

/// Collect every `xslash.json` on the path from the filesystem root down to
/// `start`, ordered outermost (root) first and `start` last, along with the
/// project dir: the nearest config's parent, or `None` when no config is found
/// (in which case the config list is also empty).
fn find_ancient_configs(start: &Path) -> (Option<PathBuf>, Vec<PathBuf>) {
    let name = "xslash.json";
    let mut configs = Vec::new();
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join(name);
        if candidate.is_file() {
            configs.push(candidate);
        }
        dir = d.parent();
    }
    // We walked from `start` upward, so `configs[0]` is the nearest (deepest)
    // config; its parent is the project dir.
    let project_dir = configs
        .first()
        .and_then(|json| json.parent())
        .map(Path::to_path_buf);
    // Reverse so the outermost config comes first.
    configs.reverse();
    (project_dir, configs)
}

/// Search the `xslash.json` files from the current directory up to the root for
/// one that maps `name`, returning the resolved entry path. The nearest config
/// to the current directory wins; outer configs act as a fallback for names a
/// nearer config doesn't map.
fn find_project_entry(name: &str) -> Result<Option<PathBuf>> {
    let cwd = ::std::env::current_dir().context("getting current directory")?;
    let (_, configs) = find_ancient_configs(&cwd);
    // `configs` runs root -> cwd; scan from the nearest (deepest) config upward
    // and take the first that maps `name`.
    for json in configs.iter().rev() {
        let contents = ::std::fs::read_to_string(json)
            .with_context(|| format!("reading {}", json.display()))?;
        let map: BTreeMap<String, String> = ::serde_json::from_str(&contents)
            .with_context(|| format!("parsing {}", json.display()))?;
        if let Some(rel) = map.get(name) {
            let dir = json.parent().unwrap_or_else(|| Path::new("."));
            return Ok(Some(dir.join(rel)));
        }
    }
    Ok(None)
}

fn execv_entry(entry_path: &OsStr, forward: &[OsString]) -> Result<()> {
    let path = Path::new(entry_path);
    let display = path.display().to_string();
    let contents =
        ::std::fs::read_to_string(path).with_context(|| format!("reading entry {display}"))?;
    let entry = XslashEntry::parse(&contents).with_context(|| format!("in entry {display}"))?;

    let resolved = if !entry.is_final()
        && let Some(path) = find_project_entry(&entry.name)?
    {
        ::xslash::read_actual(&path)?
    } else if entry.actual().is_some() {
        ::xslash::read_actual(path)?
    } else {
        entry
    };

    // The policy above has resolved which entry to run; execv_final runs it
    // directly without re-consulting the `final` field. argv[0] stays the path
    // the launcher was invoked as, so the binary behaves exactly as a symlink
    // in its place would (e.g. `python` vs `python3` sharing one entry).
    ::xslash::execv_final(&resolved, entry_path, forward)
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

/// Rebuild a `PATH` string for the shell to `eval`: drop every occurrence of the
/// previously injected dir, then prepend the new one (unless already present).
fn inject_path(path: &str, old: Option<&str>, new: Option<&str>) -> String {
    let mut entries: Vec<&str> = path.split(':').filter(|e| Some(*e) != old).collect();
    if let Some(new) = new
        && !entries.contains(&new)
    {
        entries.insert(0, new);
    }
    entries.join(":")
}

/// Single-quote `s` for safe `eval` in a POSIX shell: wrap in `'...'`, writing
/// each embedded `'` as `'\''`.
fn sh_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Reconcile the shim dir `resolve_dir` against `configs` (root -> cwd),
/// returning a digest of the resolved name -> entry-path map.
///
/// Does nothing when `old_digest` (carried from the previous prompt via
/// `XSLASH_RESOLVE_DIGEST`) still matches and the dir exists; otherwise rebuilds
/// the shims, announcing changes on stderr unless `silent`.
fn reconcile_resolve_dir(
    resolve_dir: &Path,
    configs: &[PathBuf],
    old_digest: Option<&str>,
    silent: bool,
) -> Result<String> {
    // Build the name -> resolved entry path map the shims bake into `actual`.
    // `configs` runs root -> cwd, so inserting in order lets a nearer config
    // override an outer one (nearest wins, as at runtime).
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    for json in configs {
        let contents = ::std::fs::read_to_string(json)
            .with_context(|| format!("reading {}", json.display()))?;
        let entries: BTreeMap<String, String> = ::serde_json::from_str(&contents)
            .with_context(|| format!("parsing {}", json.display()))?;
        let dir = json.parent().unwrap_or_else(|| Path::new("."));
        for (name, rel) in entries {
            resolved.insert(name, dir.join(rel).display().to_string());
        }
    }

    // Judge freshness by one digest carried in the environment (direnv style):
    // blake3 over the resolved map as compact, sorted-key JSON. Any added,
    // removed, or repointed name moves it; an edit that leaves the map identical
    // does not.
    let serialized = ::serde_json::to_string(&resolved).context("serializing resolved map")?;
    let digest = Hash::Blake3.hash_bytes(serialized.as_bytes())?;

    // Reconcile only when the digest moved, or the dir is missing (e.g. the cache
    // was cleared): otherwise the shims already match and this is a no-op.
    if !(resolve_dir.is_dir() && old_digest == Some(digest.as_str())) {
        ::std::fs::create_dir_all(&resolve_dir)
            .with_context(|| format!("creating {}", resolve_dir.display()))?;

        // Announce (direnv-style, on stderr) which configs are now in effect.
        if !silent {
            for config in configs {
                eprintln!("xslash: load {}", config.display());
            }
        }

        // Reconcile against the mapped names instead of rebuilding wholesale: add
        // or refresh a shim for each mapped name, remove one for each name no
        // longer mapped, leaving unchanged shims (and anyone mid-exec) untouched.
        let names: BTreeSet<String> = resolved.keys().cloned().collect();
        let mut existing: BTreeSet<String> = BTreeSet::new();
        for entry in ::std::fs::read_dir(&resolve_dir)
            .with_context(|| format!("reading {}", resolve_dir.display()))?
        {
            if let Some(name) = entry?.file_name().to_str() {
                existing.insert(name.to_string());
            }
        }

        for (name, actual) in &resolved {
            let shim_path = resolve_dir.join(name);
            let shim_entry = ::serde_json::to_string_pretty(&XslashEntry {
                name: name.clone(),
                is_final: None,
                actual: Some(actual.clone()),
                platforms: BTreeMap::new(),
            })?;
            let script = format!("#!/usr/bin/env xslash\n{shim_entry}\n");
            // Write only when the shim differs from disk, leaving unchanged shims
            // (and anyone mid-exec) untouched while refreshing a repointed `actual`.
            if ::std::fs::read_to_string(&shim_path).ok().as_deref() == Some(script.as_str()) {
                continue;
            }
            if !silent {
                eprintln!("xslash: inject {name}");
            }
            ::std::fs::write(&shim_path, &script)
                .with_context(|| format!("writing {}", shim_path.display()))?;
            Chmod::chmod(&shim_path, Chmod::add_executable)?;
        }

        for name in existing.difference(&names) {
            let shim = resolve_dir.join(name);
            if !silent {
                eprintln!("xslash: evict {name}");
            }
            ::std::fs::remove_file(&shim)
                .with_context(|| format!("removing {}", shim.display()))?;
        }
    }

    Ok(digest)
}

/// Reconcile the current project's shim dir and print shell code (a PATH update
/// and the carried-over state) for the shell to `eval`, syncing PATH with the
/// current project. Run by hand or from a non-interactive script.
fn export(silent: bool) -> Result<()> {
    let cwd = ::std::env::current_dir().context("getting current directory")?;
    let (project_dir, configs) = find_ancient_configs(&cwd);

    let resolve_dir = match &project_dir {
        Some(project_dir) => Some(Cache::from_env(false)?.resolve_dir(project_dir)?),
        None => None,
    };
    let new_resolve_dir = resolve_dir.as_ref().map(|dir| dir.display().to_string());

    let old_resolve_dir = ::std::env::var("XSLASH_RESOLVE_DIR")
        .ok()
        .filter(|s| !s.is_empty());

    if !silent && new_resolve_dir != old_resolve_dir {
        if old_resolve_dir.is_some() {
            eprintln!("xslash: exit");
        }
        if let Some(project_dir) = &project_dir {
            eprintln!("xslash: enter {}", project_dir.display());
        }
    }

    let old_resolve_digest = ::std::env::var("XSLASH_RESOLVE_DIGEST")
        .ok()
        .filter(|s| !s.is_empty());
    let new_resolve_digest = match &resolve_dir {
        Some(resolve_dir) => Some(reconcile_resolve_dir(
            resolve_dir,
            &configs,
            old_resolve_digest.as_deref(),
            silent,
        )?),
        None => None,
    };

    // Touch PATH only on an actual change, then print shell code to `eval`.
    if new_resolve_dir != old_resolve_dir {
        let path = ::std::env::var("PATH").context("reading PATH")?;
        let updated_path = inject_path(
            &path,
            old_resolve_dir.as_deref(),
            new_resolve_dir.as_deref(),
        );
        println!("export PATH={}", sh_quote(&updated_path));
        match &new_resolve_dir {
            Some(new_resolve_dir) => {
                println!("export XSLASH_RESOLVE_DIR={}", sh_quote(new_resolve_dir));
            }
            None => {
                println!("unset XSLASH_RESOLVE_DIR");
            }
        }
    }

    if new_resolve_digest != old_resolve_digest {
        match &new_resolve_digest {
            Some(digest) => println!("export XSLASH_RESOLVE_DIGEST={}", sh_quote(digest)),
            None => println!("unset XSLASH_RESOLVE_DIGEST"),
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// hook
// ---------------------------------------------------------------------------

const HOOK_SNIPPET: &str = r#"
_xslash_hook() {
  local es=$?
  eval "$(xslash --export)"
  return $es
}
if [ -n "${ZSH_VERSION:-}" ]; then
  typeset -ag precmd_functions
  precmd_functions+=(_xslash_hook)
  _xslash_hook
elif [ -n "${BASH_VERSION:-}" ]; then
  case "${PROMPT_COMMAND:-}" in
    *_xslash_hook*) ;;
    *) PROMPT_COMMAND="_xslash_hook${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
  esac
fi
"#;

/// Print the shell snippet that wires `xslash --export` into the shell's
/// per-prompt hook. Sourced once (e.g. from `.zshrc`/`.bashrc`).
fn hook(silent: bool) -> Result<()> {
    // Propagate --silent into the command the installed hook runs each prompt,
    // so a silent install stays silent.
    if silent {
        print!("{}", HOOK_SNIPPET.replace("--export", "--export --silent"));
    } else {
        print!("{HOOK_SNIPPET}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

/// The `xslash-helper` entry baked in at build time (see `build.rs`), used by
/// `xslash --` to resolve and run the helper instead of trusting `$PATH`. Empty
/// on non-release builds, which fall back to `$PATH`.
const HELPER_ENTRY: &str = include_str!(concat!(env!("OUT_DIR"), "/xslash-helper.xslash"));

/// Parse the tokens after `--hook` or `--export`: only the optional `--silent`
/// flag.
fn parse_silent(args: &[OsString]) -> Result<bool> {
    let mut silent = false;
    for arg in args {
        match arg.to_str() {
            Some("--silent") => silent = true,
            _ => bail!("unexpected argument {:?}", arg.to_string_lossy()),
        }
    }
    Ok(silent)
}

fn run() -> Result<()> {
    let args: Vec<OsString> = ::std::env::args_os().collect();
    let rest = &args[1..];

    match rest.first() {
        None => {
            print!("{HELP}");
            Ok(())
        }
        Some(first) => match first.to_str() {
            Some("--export") => {
                let silent = parse_silent(&rest[1..])?;
                export(silent)
            }
            Some("--hook") => {
                let silent = parse_silent(&rest[1..])?;
                hook(silent)
            }
            Some("--help") => {
                print!("{HELP}");
                Ok(())
            }
            Some("--version") => {
                println!("xslash {}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
            Some("--") => {
                // Use the baked-in entry if present (release builds), else fall
                // back to running xslash-helper from `$PATH`.
                let forward = &rest[1..];
                if !HELPER_ENTRY.trim().is_empty() {
                    let entry = XslashEntry::parse(HELPER_ENTRY)
                        .context("parsing the embedded xslash-helper entry")?;
                    return ::xslash::execv_final(&entry, OsStr::new("xslash-helper"), forward);
                }
                let status = Command::new("xslash-helper")
                    .args(forward)
                    .status()
                    .context("running xslash-helper")?;
                ::std::process::exit(status.code().unwrap_or(1));
            }
            _ => execv_entry(first, &rest[1..]),
        },
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xslash: {err:#}");
            ExitCode::FAILURE
        }
    }
}
