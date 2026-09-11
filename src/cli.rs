use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use clap::Parser;

/// Parse one email message and print normalized JSON (metadata + document text) to stdout.
///
/// Diagnostics are printed to stderr; stdout always contains a single JSON
/// document. With `--log-dir` the result goes to a file there; `--log-errors`
/// additionally records diagnostics in a `.log` file, and `--quiet` suppresses
/// diagnostic output on stderr.
#[derive(Debug, Parser)]
#[command(name = "tavda-mail-parser", version, long_about = None)]
pub struct Cli {
    /// Path to the raw message file (.eml). Use '-' or omit to read from stdin.
    #[arg(value_name = "FILE")]
    pub file: Option<PathBuf>,

    /// Pretty-print the JSON document with 2-space indentation instead of the default compact form.
    #[arg(long)]
    pub pretty: bool,

    /// Save the JSON result to `DIR/<base>-<pid>.json`.
    ///
    /// The base name is derived from the message Message-ID (characters
    /// outside `[A-Za-z0-9._-]` replaced with `_`, angle brackets stripped);
    /// messages without a Message-ID use a `YYYYMMDD-HHMMSS` timestamp.
    #[arg(long, value_name = "DIR")]
    pub log_dir: Option<PathBuf>,

    /// Additionally record diagnostics in `DIR/<base>-<pid>.log` (with
    /// `--log-dir`). The terminal output is kept; combine with `--quiet` to
    /// write the log only. The `.log` file is written only when diagnostics
    /// were produced.
    #[arg(long, requires = "log_dir")]
    pub log_errors: bool,

    /// Suppress diagnostics on stderr. With `--log-errors --log-dir` the
    /// diagnostics still reach the `.log` file; otherwise they are discarded.
    #[arg(long)]
    pub quiet: bool,
}

impl Cli {
    /// Reads the whole raw message either from the given file or from stdin.
    ///
    /// The rest of the pipeline only sees bytes, so the input source can later
    /// be replaced by a spool directory or an HTTP body without refactoring.
    pub fn read_input(&self) -> Result<Vec<u8>, crate::error::AppError> {
        let from_file = match self.file.as_deref() {
            Some(path) if path != Path::new("-") => Some(path),
            _ => None,
        };

        let mut buf = Vec::new();
        match from_file {
            Some(path) => {
                File::open(path)
                    .map_err(|source| crate::error::AppError::InputOpen {
                        path: path.display().to_string(),
                        source,
                    })?
                    .read_to_end(&mut buf)?;
            }
            None => {
                io::stdin().lock().read_to_end(&mut buf)?;
            }
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;

    fn temp_message(contents: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("tavda-cli-test-{}.eml", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn reads_whole_file_contents() {
        let path = temp_message(b"Subject: t\r\n\r\nbody");
        let cli = Cli {
            file: Some(path.clone()),
            pretty: false,
            log_dir: None,
            log_errors: false,
            quiet: false,
        };
        assert_eq!(cli.read_input().unwrap(), b"Subject: t\r\n\r\nbody");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn missing_file_error_mentions_path() {
        let cli = Cli {
            file: Some(PathBuf::from("/definitely/not/here.eml")),
            pretty: false,
            log_dir: None,
            log_errors: false,
            quiet: false,
        };
        match cli.read_input() {
            Err(AppError::InputOpen { path, .. }) => assert_eq!(path, "/definitely/not/here.eml"),
            other => panic!("expected InputOpen, got {other:?}"),
        }
    }
}
