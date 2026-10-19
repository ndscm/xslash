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
use ::std::fs::File;
use ::std::io::Write;
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
  xslash <entry> [args...]          Run the executable defined in the entry
  xslash -- <command> [options]     Run a helper command
  xslash --hook                     Print shell code to install the directory hook
  xslash --help                     Print this help
  xslash --version                  Print the build version

Helper commands (after `--`):
  create   --skeleton <entry> [--replace {{TAG}}=x.y.z]... --final true|false --out <path>
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
    let display = Path::new(entry_path).display().to_string();
    let contents = ::std::fs::read_to_string(entry_path)
        .with_context(|| format!("reading entry {display}"))?;
    let mut entry = XslashEntry::parse(&contents).with_context(|| format!("in entry {display}"))?;

    // A non-final entry defers to a project-level xslash.json when one maps its
    // name, so project tooling (git rebase --exec, claude, ...) picks up the
    // project's pinned executable.
    if !entry.is_final
        && let Some(path) = find_project_entry(&entry.name)?
    {
        let contents = ::std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        entry = XslashEntry::parse(&contents)
            .with_context(|| format!("in entry {}", path.display()))?;
    }

    // The policy above has resolved which entry to run; execv_final runs it
    // directly without re-consulting the `final` field. argv[0] stays the path
    // the launcher was invoked as, so the binary behaves exactly as a symlink
    // in its place would (e.g. `python` vs `python3` sharing one entry).
    xslash::execv_final(&entry, entry_path, forward)
}

// ---------------------------------------------------------------------------
// hook
// ---------------------------------------------------------------------------

struct HookArgs {
    /// Internal: reconcile the shim dir and print shell code (a PATH update and
    /// the carried-over state) for the shell to `eval`. Invoked by the shell
    /// hook on every prompt.
    shell: bool,

    /// Suppress the hook/inject/evict status messages on stderr. Without
    /// --shell, the installed hook snippet runs `--shell --silent`.
    silent: bool,
}

impl HookArgs {
    /// Parse the tokens after `--hook`: only the optional `--shell` and
    /// `--silent` flags, in any order.
    fn parse(args: &[OsString]) -> Result<HookArgs> {
        let mut parsed = HookArgs {
            shell: false,
            silent: false,
        };
        for arg in args {
            match arg.to_str() {
                Some("--shell") => parsed.shell = true,
                Some("--silent") => parsed.silent = true,
                _ => bail!(
                    "xslash --hook: unexpected argument {:?}",
                    arg.to_string_lossy()
                ),
            }
        }
        Ok(parsed)
    }
}

const HOOK_SNIPPET: &str = r#"_xslash_hook() {
  local es=$?
  eval "$(xslash --hook --shell)"
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

/// Rebuild a `PATH` string for the `eval`'d hook: drop every occurrence of the
/// previously hooked dir, then prepend the new one (unless already present).
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

/// Reconcile the shim dir for `configs` (the `xslash.json` files in effect,
/// root -> cwd), keyed on `project_dir`, against their mapped names, returning
/// the shim dir's path.
///
/// Announces the config set and each shim change on stderr (unless `silent`).
/// The shim files and the `<digest>.json` record are updated in place, so an
/// unchanged project is a no-op.
fn reconcile_hook_dir(project_dir: &Path, configs: &[PathBuf], silent: bool) -> Result<String> {
    // Read every config once: the union of mapped names (for the shims) and a
    // path -> content-digest map (to tell whether the shim dir is still
    // current). A shim needs only the name - a bare name-only entry xslash
    // resolves against the directory config at runtime.
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut digests: BTreeMap<String, String> = BTreeMap::new();
    for json in configs {
        let contents = ::std::fs::read_to_string(json)
            .with_context(|| format!("reading {}", json.display()))?;
        let entries: BTreeMap<String, String> = ::serde_json::from_str(&contents)
            .with_context(|| format!("parsing {}", json.display()))?;
        names.extend(entries.into_keys());
        let digest = Hash::Blake3.hash_bytes(contents.as_bytes())?;
        digests.insert(json.display().to_string(), digest);
    }

    // Key the hook dir on the project dir - the project root we're inside.
    let cache = Cache::from_env(false)?;
    let hook_dir = cache.hook_dir(project_dir)?;
    let metadata = hook_dir.with_extension("json");

    // The hook runs every prompt and a config can change without a `cd`, so
    // judge freshness by content: reuse the shim dir when it exists and the
    // recorded `<digest>.json` still matches every config. Any added, removed,
    // or edited config regenerates.
    let recorded: Option<BTreeMap<String, String>> = ::std::fs::read(&metadata)
        .ok()
        .and_then(|bytes| ::serde_json::from_slice(&bytes).ok());

    // Announce (direnv-style, on stderr) which configs are now in effect when
    // the *set* of config files changes: first load, or one added or removed.
    // A config merely edited in place keeps the same keys and stays quiet.
    let config_set_changed = match &recorded {
        None => true,
        Some(recorded) => !recorded.keys().eq(digests.keys()),
    };
    if config_set_changed && !silent {
        for config in digests.keys() {
            eprintln!("xslash: load {config}");
        }
    }

    if !(hook_dir.is_dir() && recorded.as_ref() == Some(&digests)) {
        ::std::fs::create_dir_all(&hook_dir)
            .with_context(|| format!("creating {}", hook_dir.display()))?;

        // Reconcile against the configs' names instead of rebuilding wholesale:
        // add a shim for each newly-mapped name, remove one for each name no
        // longer mapped, leaving unchanged shims (and anyone mid-exec) untouched.
        let mut existing: BTreeSet<String> = BTreeSet::new();
        for entry in ::std::fs::read_dir(&hook_dir)
            .with_context(|| format!("reading {}", hook_dir.display()))?
        {
            if let Some(name) = entry?.file_name().to_str() {
                existing.insert(name.to_string());
            }
        }

        for name in names.difference(&existing) {
            let shim = hook_dir.join(name);
            let entry = ::serde_json::to_string(&BTreeMap::from([("name", name.as_str())]))?;
            let script = format!("#!/usr/bin/env xslash\n{entry}\n");
            if !silent {
                eprintln!("xslash: inject {name}");
            }
            ::std::fs::write(&shim, script)
                .with_context(|| format!("writing {}", shim.display()))?;
            Chmod::chmod(&shim, Chmod::add_executable)?;
        }

        for name in existing.difference(&names) {
            let shim = hook_dir.join(name);
            if !silent {
                eprintln!("xslash: evict {name}");
            }
            ::std::fs::remove_file(&shim)
                .with_context(|| format!("removing {}", shim.display()))?;
        }

        // Record the digests last, so a crash mid-reconcile leaves a stale or
        // absent sidecar that forces the reconcile to run again next time.
        let mut file =
            File::create(&metadata).with_context(|| format!("writing {}", metadata.display()))?;
        ::serde_json::to_writer_pretty(&mut file, &digests).context("serializing hook metadata")?;
        file.write_all(b"\n")
            .with_context(|| format!("writing {}", metadata.display()))?;
    }

    Ok(hook_dir.display().to_string())
}

fn hook(args: HookArgs) -> Result<()> {
    if !args.shell {
        // Propagate --silent into the command the installed hook runs each
        // prompt, so a silent install stays silent.
        if args.silent {
            print!(
                "{}",
                HOOK_SNIPPET.replace("--hook --shell", "--hook --shell --silent")
            );
        } else {
            print!("{HOOK_SNIPPET}");
        }
        return Ok(());
    }

    let cwd = ::std::env::current_dir().context("getting current directory")?;
    let (project_dir, configs) = find_ancient_configs(&cwd);

    // Reconcile the current project's shim dir, or no dir when not in a project.
    let new_hook_dir = match &project_dir {
        Some(project_dir) => Some(reconcile_hook_dir(project_dir, &configs, args.silent)?),
        None => None,
    };

    // The previously hooked dir rides along in the environment (direnv style),
    // so the "did the hooked path change?" decision lives in the binary, not the
    // shell. Touch PATH only on an actual change - a cd into or out of a project,
    // or a config change that moved the dir - then print shell code to `eval`.
    let old_hook_dir = ::std::env::var("XSLASH_HOOK_PATH")
        .ok()
        .filter(|s| !s.is_empty());
    if new_hook_dir != old_hook_dir {
        let path = ::std::env::var("PATH").context("reading PATH")?;
        let updated = inject_path(&path, old_hook_dir.as_deref(), new_hook_dir.as_deref());
        println!("export PATH={}", sh_quote(&updated));
        // Always announce leaving the previous hooked dir, before entering the
        // new one.
        if let Some(_) = &old_hook_dir
            && !args.silent
        {
            eprintln!("xslash: exit");
        }

        match &new_hook_dir {
            Some(new_hook_dir) => {
                println!("export XSLASH_HOOK_PATH={}", sh_quote(new_hook_dir));
                if !args.silent
                    && let Some(project_dir) = &project_dir
                {
                    eprintln!("xslash: enter {}", project_dir.display());
                }
            }
            None => {
                println!("unset XSLASH_HOOK_PATH");
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn run() -> Result<()> {
    let args: Vec<OsString> = ::std::env::args_os().collect();
    let rest = &args[1..];

    match rest.first() {
        None => {
            print!("{HELP}");
            Ok(())
        }
        Some(first) => match first.to_str() {
            Some("--help") => {
                print!("{HELP}");
                Ok(())
            }
            Some("--version") => {
                println!("xslash {}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
            Some("--hook") => hook(HookArgs::parse(&rest[1..])?),
            Some("--") => {
                // TODO: use an xslash entry to run xslash-helper.
                use ::std::os::unix::process::CommandExt;
                let err = Command::new("xslash-helper").args(&rest[1..]).exec();
                Err(err).context("running xslash-helper")
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
