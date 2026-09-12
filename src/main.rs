use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::Path;
use std::process::ExitCode;
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

mod cli;
mod document;
mod email;
mod error;
mod output;

use clap::Parser;

use cli::Cli;
use email::RawMessage;
use error::AppError;

const STDERR_FD: RawFd = 2;

/// Message-IDs may carry arbitrary characters; keep a generous cap so the
/// generated file name cannot exceed `PATH_MAX`.
const MAX_BASE_LEN: usize = 100;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match &cli.log_dir {
        Some(dir) => run_with_logging(&cli, dir),
        None => run_plain(&cli),
    }
}

/// Default mode: JSON to stdout, diagnostics to stderr. `--quiet` silences
/// the diagnostics; `--log-errors` requires `--log-dir`, so it never gets here.
fn run_plain(cli: &Cli) -> ExitCode {
    if cli.quiet && let Err(err) = redirect_stderr_devnull() {
        eprintln!("error: cannot silence stderr: {err}");
        return ExitCode::FAILURE;
    }
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            report_error(&err);
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), AppError> {
    let raw = cli.read_input()?;
    let parsed = email::parser::parse(RawMessage::new(&raw))?;
    finish(parsed, cli.pretty, None)
}

/// Builds the JSON result, delivers it on stdout, and optionally archives a
/// copy to `DIR/<base>-<pid>.json`. stdout is written first so the consuming
/// application always gets the result even if the archive copy fails.
fn finish(
    parsed: email::ParsedEmail,
    pretty: bool,
    archive: Option<(&Path, &str)>,
) -> Result<(), AppError> {
    let documents = document::process_attachments(parsed.attachments());
    let message = parsed.into_email(documents);
    let json = output::render_json(&message, pretty)?;

    let mut stdout = io::stdout().lock();
    stdout.write_all(json.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    drop(stdout);

    if let Some((dir, base)) = archive {
        let json_path = dir.join(format!("{base}-{}.json", std::process::id()));
        fs::write(&json_path, json.as_bytes())?;
    }
    Ok(())
}

fn report_error(err: &AppError) {
    eprintln!("error: {err}");
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        eprintln!("  caused by: {cause}");
        source = cause.source();
    }
}

/// `--log-dir` mode: the JSON result is always delivered on stdout, and a
/// copy is archived to `DIR/<base>-<pid>.json`.
///
/// Diagnostics keep flowing to stderr as usual unless `--quiet` suppresses
/// them. With `--log-errors` they are additionally mirrored into
/// `DIR/<base>-<pid>.log`, which is only written when some were produced.
fn run_with_logging(cli: &Cli, dir: &Path) -> ExitCode {
    let raw = cli.read_input();

    let mut parsed: Option<email::ParsedEmail> = None;
    let mut pending: Option<AppError> = None;
    match raw {
        Ok(bytes) => match email::parser::parse(RawMessage::new(&bytes)) {
            Ok(message) => parsed = Some(message),
            Err(err) => pending = Some(err.into()),
        },
        Err(err) => pending = Some(err),
    }

    let base = parsed
        .as_ref()
        .and_then(|message| message.message_id())
        .map(|id| sanitize_base(id.as_str()))
        .filter(|base| !base.is_empty())
        .unwrap_or_else(fallback_base);

    if let Err(err) = fs::create_dir_all(dir) {
        eprintln!("error: cannot set up log directory `{}`: {err}", dir.display());
        return ExitCode::FAILURE;
    }

    let capture = match (cli.log_errors, cli.quiet) {
        (true, quiet) => match capture_stderr(quiet) {
            Ok(capture) => Some(capture),
            Err(err) => {
                eprintln!("error: cannot capture diagnostics: {err}");
                return ExitCode::FAILURE;
            }
        },
        (false, true) => {
            if let Err(err) = redirect_stderr_devnull() {
                eprintln!("error: cannot silence stderr: {err}");
                return ExitCode::FAILURE;
            }
            None
        }
        (false, false) => None,
    };

    let exit = match (parsed, pending) {
        (Some(message), _) => match finish(message, cli.pretty, Some((dir, &base))) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                report_error(&err);
                ExitCode::FAILURE
            }
        },
        (None, Some(err)) => {
            report_error(&err);
            ExitCode::FAILURE
        }
        (None, None) => ExitCode::FAILURE,
    };

    if let Some(capture) = capture {
        let bytes = finalize_capture(capture);
        if !bytes.is_empty() {
            let log_path = dir.join(format!("{base}-{}.log", std::process::id()));
            if let Err(err) = fs::write(&log_path, &bytes) {
                eprintln!(
                    "error: cannot write diagnostic log `{}`: {err}",
                    log_path.display()
                );
            }
        }
    }
    exit
}

/// A stderr capture: fd 2 is pointed at a pipe owned by `reader`.
///
/// The reader drains the pipe without blocking (so diagnostics larger than
/// the pipe buffer cannot deadlock), accumulating the bytes. Unless `quiet`
/// is set it also forwards every chunk to the original stderr in real time,
/// preserving the terminal output of the plain mode.
struct Capture {
    reader: Option<JoinHandle<Vec<u8>>>,
    saved_fd: RawFd,
}

/// Redirects fd 2 into a pipe drained by a background thread.
fn capture_stderr(quiet: bool) -> io::Result<Capture> {
    let saved_fd = unsafe { libc::dup(STDERR_FD) };
    if saved_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        let err = io::Error::last_os_error();
        let _ = unsafe { libc::close(saved_fd) };
        return Err(err);
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);

    if unsafe { libc::dup2(write_fd, STDERR_FD) } < 0 {
        let err = io::Error::last_os_error();
        let _ = unsafe { libc::close(saved_fd) };
        let _ = unsafe { libc::close(read_fd) };
        let _ = unsafe { libc::close(write_fd) };
        return Err(err);
    }
    let _ = unsafe { libc::close(write_fd) };

    let reader = std::thread::spawn(move || {
        let mut read = unsafe { File::from_raw_fd(read_fd) };
        // In quiet mode the saved fd is kept open as a plain RawFd: nobody
        // writes through it, but `finalize_capture` still needs it to restore
        // fd 2 (which sends the pipe EOF that ends this loop).
        let mut tee = if quiet {
            None
        } else {
            Some(unsafe { File::from_raw_fd(saved_fd) })
        };
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match read.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(writer) = tee.as_mut() {
                        let _ = writer.write_all(&chunk[..n]);
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                Err(_) => break,
            }
        }
        buf
    });

    Ok(Capture {
        reader: Some(reader),
        saved_fd,
    })
}

/// Restores fd 2 (which also closes the pipe write end, so the reader hits
/// EOF), waits for the captured bytes and returns them.
fn finalize_capture(capture: Capture) -> Vec<u8> {
    let _ = unsafe { libc::dup2(capture.saved_fd, STDERR_FD) };
    match capture.reader {
        Some(reader) => match reader.join() {
            Ok(bytes) => bytes,
            Err(_) => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// Points fd 2 (stderr) at `/dev/null` so diagnostics are discarded.
fn redirect_stderr_devnull() -> io::Result<()> {
    let null = fs::OpenOptions::new().write(true).open("/dev/null")?;
    set_fd(STDERR_FD, &null)
}

/// Puts `from` onto the file descriptor `to_fd`.
fn set_fd(to_fd: RawFd, from: &File) -> io::Result<()> {
    let rc = unsafe { libc::dup2(from.as_raw_fd(), to_fd) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Maps a Message-ID to a file-name-safe base: wrapping angle brackets are
/// stripped, characters outside `[A-Za-z0-9._-]` become `_`, and the result
/// is capped in length.
fn sanitize_base(id: &str) -> String {
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_BASE_LEN)
        .collect();
    sanitized.trim_matches('_').to_string()
}

/// Fallback name for messages without a usable Message-ID, in the
/// `YYYYMMDD-HHMMSS` format of the original shell script.
fn fallback_base() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let secs_of_day = timestamp % 86_400;
    let (year, month, day) = civil_from_days((timestamp / 86_400) as i64);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60,
    )
}

/// Howard Hinnant's `civil_from_days`: days since epoch to (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}