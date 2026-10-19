//! Running the resolved executable: either replacing the current process or
//! spawning it as a subprocess.

use ::anyhow::Error;
use ::anyhow::Result;
use ::std::convert::Infallible;
use ::std::ffi::OsStr;
use ::std::ffi::OsString;
use ::std::path::Path;
use ::std::process::Command;
use ::std::process::ExitStatus;

/// Spawn `program` as a subprocess (argv\[0\] = `arg0`) and wait for it to exit,
/// returning its status.
#[cfg(unix)]
pub fn spawn(program: &Path, arg0: &OsStr, args: &[OsString]) -> Result<ExitStatus> {
    use ::std::os::unix::process::CommandExt;
    Command::new(program)
        .arg0(arg0)
        .args(args)
        .status()
        .map_err(|err| Error::new(err).context(format!("spawn {}", program.display())))
}

/// Spawn `program` as a subprocess and wait for it to exit, returning its
/// status. `arg0` is ignored on platforms without `arg0` support.
#[cfg(not(unix))]
pub fn spawn(program: &Path, _arg0: &OsStr, args: &[OsString]) -> Result<ExitStatus> {
    Command::new(program)
        .args(args)
        .status()
        .map_err(|err| Error::new(err).context(format!("spawn {}", program.display())))
}

/// Replace the current process with `program`, with argv\[0\] set to `arg0`.
///
/// On unix this never returns on success, so the `Ok` variant is uninhabited.
/// On other platforms it spawns, waits, and exits with the child's status code.
#[cfg(unix)]
pub fn execv(program: &Path, arg0: &OsStr, args: &[OsString]) -> Result<Infallible> {
    use ::std::os::unix::process::CommandExt;
    let err = Command::new(program).arg0(arg0).args(args).exec();
    Err(Error::new(err).context(format!("exec {}", program.display())))
}

#[cfg(not(unix))]
pub fn execv(program: &Path, arg0: &OsStr, args: &[OsString]) -> Result<Infallible> {
    let status = spawn(program, arg0, args)?;
    ::std::process::exit(status.code().unwrap_or(1));
}
