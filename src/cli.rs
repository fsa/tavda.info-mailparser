use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use clap::Parser;

/// Parse one email message and print normalized JSON (metadata + document text) to stdout.
///
/// Diagnostics are printed to stderr; stdout always contains a single JSON document.
#[derive(Debug, Parser)]
#[command(name = "tavda-mail-parser", version, long_about = None)]
pub struct Cli {
    /// Path to the raw message file (.eml). Use '-' or omit to read from stdin.
    #[arg(value_name = "FILE")]
    pub file: Option<PathBuf>,
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
        };
        assert_eq!(cli.read_input().unwrap(), b"Subject: t\r\n\r\nbody");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn missing_file_error_mentions_path() {
        let cli = Cli {
            file: Some(PathBuf::from("/definitely/not/here.eml")),
        };
        match cli.read_input() {
            Err(AppError::InputOpen { path, .. }) => assert_eq!(path, "/definitely/not/here.eml"),
            other => panic!("expected InputOpen, got {other:?}"),
        }
    }
}
