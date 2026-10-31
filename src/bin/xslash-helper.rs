//! The xslash helper command-line interface.
//!
//! A thin companion to the `xslash` binary that exposes only the cache-
//! management subcommands - `create`, `download`, `extract`, and `clean` -
//! invoked directly (`xslash-helper <command>`) rather than behind the `--`
//! separator the `xslash` launcher uses. It carries no entry-run or directory-
//! hook behaviour; like `xslash`, it parses arguments and does file I/O, then
//! delegates the real work to the `xslash` library functions.

use ::anyhow::Context;
use ::anyhow::Result;
use ::anyhow::bail;
use ::clap::Args;
use ::clap::Parser;
use ::clap::Subcommand;
use ::std::ffi::OsStr;
use ::std::path::Path;
use ::std::process::ExitCode;
use ::std::time::Duration;
use ::std::time::SystemTime;

use ::xslash::Chmod;
use ::xslash::XslashEntry;

fn read_entry(path: &str) -> Result<XslashEntry> {
    let contents = ::std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    XslashEntry::parse(&contents)
}

/// Create a symlink at `link` pointing at `target`, replacing whatever is
/// already at `link`.
#[cfg(unix)]
fn symlink(target: &OsStr, link: &Path) -> Result<()> {
    // Overwrite unconditionally: symlink() fails if the path exists, so unlink
    // first. remove_file drops a symlink without touching its target and also
    // removes a regular (executable) file; a missing path is not an error.
    if let Err(err) = ::std::fs::remove_file(link)
        && err.kind() != ::std::io::ErrorKind::NotFound
    {
        return Err(err).with_context(|| format!("removing {}", link.display()));
    }
    ::std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("linking {} -> {}", link.display(), target.to_string_lossy()))
}

#[cfg(not(unix))]
fn symlink(target: &OsStr, link: &Path) -> Result<()> {
    bail!(
        "cannot link {} -> {}: symlinks are only supported on unix",
        link.display(),
        target.to_string_lossy()
    )
}

// ---------------------------------------------------------------------------
// command
// ---------------------------------------------------------------------------

#[derive(Args)]
struct CreateArgs {
    /// Skeleton entry file to fill in.
    #[arg(long, required = true)]
    skeleton: String,

    /// Replace a literal marker in the skeleton with a value, as
    /// `{{TAG}}=VALUE` (e.g. `{{TAG}}=1.2.3`). The marker is matched verbatim;
    /// `{{...}}` is just the recommended convention. Repeatable.
    #[arg(long = "replace", value_name = "{{TAG}}=VALUE")]
    replace: Vec<String>,

    /// Set the `final` flag in the generated entry (absent means not final).
    #[arg(long = "final")]
    is_final: bool,

    /// Path to write the generated entry to.
    #[arg(long = "out", required = true)]
    out: String,

    /// Create a symbolic link beside --out that points at it.
    #[arg(long = "symlink", value_name = "NAME")]
    symlink: Vec<String>,
}

#[derive(Args)]
struct DownloadArgs {
    /// Download every platform, not just the host's.
    #[arg(long)]
    all: bool,

    entry: String,
}

#[derive(Args)]
struct ExtractArgs {
    /// Remove the downloaded artifact after extracting.
    #[arg(long)]
    clean: bool,

    entry: String,
}

#[derive(Args)]
struct CleanArgs {
    /// Remove downloaded artifacts. Defaults to both when neither
    /// --downloaded nor --extracted is given.
    #[arg(long)]
    downloaded: bool,

    /// Remove extracted files. Defaults to both when neither --downloaded nor
    /// --extracted is given.
    #[arg(long)]
    extracted: bool,

    /// An entry file, or a duration like `2d`, `3m`, `4y`.
    target: String,
}

#[derive(Subcommand)]
enum Command {
    /// Create or update xslash entries from skeleton files.
    Create(CreateArgs),

    /// Download the archive without extracting it.
    Download(DownloadArgs),

    /// Download and extract artifacts.
    Extract(ExtractArgs),

    /// Remove extracted files (or downloads) to reclaim disk space.
    Clean(CleanArgs),
}

/// xslash-helper - simplified distribution of prebuilt executables
#[derive(Parser)]
#[command(name = "xslash-helper", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

fn create(args: CreateArgs) -> Result<()> {
    let mut replacements: Vec<(String, String)> = Vec::new();
    for replacement in &args.replace {
        let (marker, value) = replacement
            .split_once('=')
            .with_context(|| format!("invalid --replace, expected {{TAG}}=VALUE: {replacement}"))?;
        replacements.push((marker.to_string(), value.to_string()));
    }

    let contents = ::std::fs::read_to_string(&args.skeleton)
        .with_context(|| format!("reading {}", args.skeleton))?;
    let skeleton = XslashEntry::parse(&contents)
        .with_context(|| format!("parsing skeleton {}", args.skeleton))?;

    let entry = ::xslash::create(skeleton, replacements, args.is_final)?;

    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"#!/usr/bin/env xslash\n");
    ::serde_json::to_writer_pretty(&mut buf, &entry)?;
    buf.push(b'\n');

    let out_path = Path::new(&args.out);
    if let Some(parent) = out_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        ::std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    ::std::fs::write(out_path, &buf).with_context(|| format!("writing {}", out_path.display()))?;
    Chmod::chmod(out_path, Chmod::add_executable)?;
    println!("{}", out_path.display());

    // Create each requested side-by-side symlink: `ln -s -f <basename(out)> NAME`
    // run from --out's directory, so the link points at the entry by a relative
    // name and resolves wherever the directory is moved.
    let out_dir = out_path.parent().filter(|p| !p.as_os_str().is_empty());
    let out_name = out_path
        .file_name()
        .with_context(|| format!("--out has no file name: {}", out_path.display()))?;
    for link_name in &args.symlink {
        if Path::new(link_name).file_name() != Some(OsStr::new(link_name)) {
            bail!("--symlink NAME must be a bare file name, not a path: {link_name}");
        }
        let link_path = match out_dir {
            Some(dir) => dir.join(link_name),
            None => Path::new(link_name).to_path_buf(),
        };
        if link_path == out_path {
            bail!(
                "--symlink {link_name} collides with --out {}",
                out_path.display()
            );
        }

        symlink(out_name, &link_path)?;

        println!("[symlink] {}", link_path.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// download
// ---------------------------------------------------------------------------

fn download(args: DownloadArgs) -> Result<()> {
    let entry = read_entry(&args.entry)?;

    if args.all {
        // --all is a CLI convenience that downloads each platform block in turn.
        for key in entry.platforms.keys() {
            for path in ::xslash::download(&entry, Some(key))? {
                println!("[{key}] {}", path.display());
            }
        }
    } else {
        for path in ::xslash::download(&entry, None)? {
            println!("{}", path.display());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

fn extract(args: ExtractArgs) -> Result<()> {
    let entry = read_entry(&args.entry)?;
    let exe = ::xslash::extract(&entry, args.clean)?;
    println!("{}", exe.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// clean
// ---------------------------------------------------------------------------

/// Parse a duration like `2d`, `3m`, `4y` (days, months, years).
fn parse_age(s: &str) -> Result<Duration> {
    let (digits, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .context("duration has no unit")?,
    );
    if digits.is_empty() {
        bail!("duration has no number: {s}");
    }
    let n: u64 = digits
        .parse()
        .with_context(|| format!("invalid duration: {s}"))?;
    let secs_per = match unit {
        "d" => 86_400,
        "m" => 30 * 86_400,
        "y" => 365 * 86_400,
        other => bail!("unknown duration unit {other:?} (use d, m, or y)"),
    };
    // Cap at 10 years, which also keeps the multiply from overflowing.
    const MAX_AGE_SECS: u64 = 10 * 365 * 86_400;
    let secs = n
        .checked_mul(secs_per)
        .filter(|&secs| secs <= MAX_AGE_SECS)
        .with_context(|| format!("duration too large (max 10y): {s}"))?;
    Ok(Duration::from_secs(secs))
}

fn clean(args: CleanArgs) -> Result<()> {
    // With neither flag given, clean both; otherwise clean exactly what's asked.
    let (downloaded, extracted) = if !args.downloaded && !args.extracted {
        (true, true)
    } else {
        (args.downloaded, args.extracted)
    };

    if let Ok(age) = parse_age(&args.target) {
        let before = SystemTime::now()
            .checked_sub(age)
            .with_context(|| format!("age {} predates the epoch", args.target))?;
        ::xslash::clean_before(before, downloaded, extracted)?;
        return Ok(());
    }

    let entry = read_entry(&args.target)?;
    ::xslash::clean(&entry, downloaded, extracted)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Create(args) => create(args),
        Command::Download(args) => download(args),
        Command::Extract(args) => extract(args),
        Command::Clean(args) => clean(args),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xslash-helper: {err:#}");
            ExitCode::FAILURE
        }
    }
}
