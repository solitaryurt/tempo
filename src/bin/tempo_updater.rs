//! Tempo self-update helper.
//!
//! Runs as a small standalone process that the main `tempo` binary
//! spawns when the user clicks "Install update". The helper:
//!
//! 1. Waits for the supplied parent PID to exit.
//! 2. Replaces the on-disk binary at `--to` with the file at `--from`.
//! 3. Re-launches the new binary (when invoked with `--restart`).
//!
//! This file is intentionally dependency-light: only `anyhow` from
//! the workspace, plus the standard library. Anything heavier risks
//! a long compile chain on a binary that is already built once for
//! every release; the goal is "tiny, boring, robust".
//!
//! ## CLI
//!
//! ```text
//! tempo-updater --pid <PID> --from <SRC> --to <DST> [--restart] [--log <FILE>]
//! ```
//!
//! ## Behaviour notes
//!
//! - On Linux we wait for `kill -0 <pid>` to fail (process gone) or
//!   for a 30 s timeout, whichever comes first. The timeout exists
//!   so a zombie / orphaned helper never sits forever; in practice
//!   the parent exits within a few hundred milliseconds of spawning
//!   us.
//! - If the rename succeeds the helper exits with status 0. Any
//!   failure path writes the diagnostic to `--log` (when provided)
//!   and stderr, then exits non-zero. The main app surfaces the log
//!   on next launch when present.
//! - The helper deliberately does **not** spawn a long-lived
//!   replacement process: it runs the new `tempo` and immediately
//!   detaches. Tempo itself owns its own lifecycle from there.

use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};

const PARENT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const PARENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const RENAME_RETRY_TIMEOUT: Duration = Duration::from_secs(10);
const RENAME_RETRY_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
struct Args {
    pid: u32,
    from: PathBuf,
    to: PathBuf,
    restart: bool,
    log: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("tempo-updater: {err:#}");
            return ExitCode::from(2);
        }
    };

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let message = format!("tempo-updater: {err:#}\n");
            if let Some(log) = &args.log {
                let _ = append_log(log, &message);
            }
            eprintln!("{message}");
            ExitCode::from(1)
        }
    }
}

fn run(args: &Args) -> Result<()> {
    if !args.from.is_file() {
        bail!("source file does not exist: {}", args.from.display());
    }

    wait_for_parent_exit(args.pid)?;

    replace_binary(&args.from, &args.to)?;

    if args.restart {
        restart_app(&args.to);
    }
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut iter = env::args().skip(1);
    let mut pid: Option<u32> = None;
    let mut from: Option<PathBuf> = None;
    let mut to: Option<PathBuf> = None;
    let mut restart = false;
    let mut log: Option<PathBuf> = None;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--pid" => {
                let value = iter.next().context("--pid requires a value")?;
                pid = Some(value.parse().context("--pid must be numeric")?);
            }
            "--from" => {
                from = Some(PathBuf::from(
                    iter.next().context("--from requires a path")?,
                ));
            }
            "--to" => {
                to = Some(PathBuf::from(iter.next().context("--to requires a path")?));
            }
            "--restart" => restart = true,
            "--log" => {
                log = Some(PathBuf::from(iter.next().context("--log requires a path")?));
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    Ok(Args {
        pid: pid.ok_or_else(|| anyhow!("missing --pid"))?,
        from: from.ok_or_else(|| anyhow!("missing --from"))?,
        to: to.ok_or_else(|| anyhow!("missing --to"))?,
        restart,
        log,
    })
}

fn print_usage() {
    eprintln!(
        "Usage: tempo-updater --pid <PID> --from <SRC> --to <DST> [--restart] [--log <FILE>]"
    );
}

/// Block until the supplied PID is no longer alive, or until
/// [`PARENT_WAIT_TIMEOUT`] elapses. Returns on either condition; we
/// then proceed to the rename step regardless because the timeout
/// case usually means the parent crashed and won't be coming back.
fn wait_for_parent_exit(pid: u32) -> Result<()> {
    let started = Instant::now();
    while started.elapsed() < PARENT_WAIT_TIMEOUT {
        if !is_process_alive(pid) {
            return Ok(());
        }
        thread::sleep(PARENT_POLL_INTERVAL);
    }
    // Don't fail hard: even if the parent is still alive, the
    // rename loop below will retry while the file is busy. Returning
    // here lets us at least try.
    Ok(())
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    // `kill(pid, 0)` returns 0 if the process exists and we have
    // permission to signal it, ESRCH (3) if no such process, EPERM
    // (1) if it exists but we can't signal. We treat EPERM as
    // "alive" because that's strictly more conservative.
    use std::io::ErrorKind;
    // SAFETY: kill with signal 0 has no side effects beyond errno.
    let result = unsafe { libc_kill(pid as i32, 0) };
    if result == 0 {
        return true;
    }
    // libc::ESRCH == 3 on Linux/macOS/BSD.
    let last = std::io::Error::last_os_error();
    !matches!(last.raw_os_error(), Some(3)) && last.kind() != ErrorKind::NotFound
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    // Conservative default: assume alive so the helper waits the
    // full timeout before trying to replace the binary.
    true
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
unsafe fn libc_kill(pid: i32, sig: i32) -> i32 {
    unsafe { kill(pid, sig) }
}

/// Replace `to` with `from`. We try a plain rename first; on Linux
/// this is atomic and works for already-running executables (the
/// kernel keeps the running text segment mapped via the open
/// inode, the new file gets a fresh inode under the same path).
///
/// If rename fails we retry for [`RENAME_RETRY_TIMEOUT`] in case
/// the OS still holds the file briefly (Windows pattern more than
/// Linux, but cheap insurance everywhere).
///
/// As a last resort we fall back to `copy + remove + rename`: copy
/// `from` to a sibling temp path on the target's filesystem, swap
/// it over the destination, then remove the source. This handles
/// the rare case where `from` and `to` end up on different
/// filesystems despite the main app's same-fs check.
fn replace_binary(from: &Path, to: &Path) -> Result<()> {
    let started = Instant::now();
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err) => {
                if started.elapsed() > RENAME_RETRY_TIMEOUT {
                    // Final attempt: cross-fs copy.
                    return copy_then_swap(from, to)
                        .with_context(|| format!("rename retried failed: {err}"));
                }
                thread::sleep(RENAME_RETRY_INTERVAL);
            }
        }
    }
}

fn copy_then_swap(from: &Path, to: &Path) -> Result<()> {
    let parent = to
        .parent()
        .ok_or_else(|| anyhow!("destination {} has no parent directory", to.display()))?;
    // `tempfile::NamedTempFile` would be ideal, but we deliberately
    // avoid extra deps. A unique sibling name based on PID +
    // monotonic clock is good enough.
    let stamp = format!(
        ".tempo-updater-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos(),
    );
    let staging = parent.join(stamp);
    std::fs::copy(from, &staging)
        .with_context(|| format!("copy {} -> {}", from.display(), staging.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Ok(meta) = std::fs::metadata(&staging) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&staging, perms);
        }
    }

    std::fs::rename(&staging, to)
        .with_context(|| format!("rename {} -> {}", staging.display(), to.display()))?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// Re-launch the freshly installed binary and detach. We never
/// wait for it: the helper's job ends here.
fn restart_app(target: &Path) {
    let mut command = std::process::Command::new(target);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Detach from the helper's session so the new process
        // survives if the helper itself is reaped first.
        unsafe {
            command.pre_exec(|| {
                // SAFETY: setsid has no side effects beyond placing
                // the process in a new session; failure is non-fatal.
                let _ = setsid_libc();
                Ok(())
            });
        }
    }

    // Don't propagate stdio: the helper might be running detached
    // already, and we don't want the new process to inherit pipes
    // that the OS will close behind us.
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());

    let _ = command.spawn();
}

#[cfg(unix)]
fn setsid_libc() -> std::io::Result<()> {
    unsafe extern "C" {
        fn setsid() -> i32;
    }
    let result = unsafe { setsid() };
    if result == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn append_log(path: &Path, message: &str) -> Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open log {}", path.display()))?;
    file.write_all(message.as_bytes())
        .with_context(|| format!("write log {}", path.display()))?;
    Ok(())
}
