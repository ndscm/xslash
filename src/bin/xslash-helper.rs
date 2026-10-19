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

    /// Set the `final` flag in the generated entry.
    #[arg(long = "final", action = ::clap::ArgAction::Set, value_name = "true|false")]
    is_final: bool,

    /// Path to write the generated entry to.
    #[arg(long = "out", required = true)]
    out: String,
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

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"#!/usr/bin/env xslash\n");
    ::serde_json::to_writer_pretty(&mut out, &entry)?;
    out.push(b'\n');

    let path = Path::new(&args.out);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        ::std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    ::std::fs::write(path, &out).with_context(|| format!("writing {}", path.display()))?;
    Chmod::chmod(path, Chmod::add_executable)?;
    eprintln!("xslash-helper: created {}", path.display());

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
                eprintln!("xslash-helper: downloaded [{key}] {}", path.display());
            }
        }
    } else {
        for path in ::xslash::download(&entry, None)? {
            eprintln!("xslash-helper: downloaded {}", path.display());
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
    eprintln!("xslash-helper: extracted {}", exe.display());
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
