use std::io;

use crate::email::EmailError;
use crate::output::OutputError;

/// Fatal, message-level errors. Per-document problems are reported inside the
/// JSON output instead and never abort processing of the whole message.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("cannot open input file `{path}`")]
    InputOpen {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("failed to read input message")]
    InputRead(#[source] io::Error),
    #[error(transparent)]
    Email(#[from] EmailError),
    #[error(transparent)]
    Output(#[from] OutputError),
}

impl From<io::Error> for AppError {
    fn from(source: io::Error) -> Self {
        AppError::InputRead(source)
    }
}
