use std::process::ExitCode;

mod cli;
mod document;
mod email;
mod error;
mod output;

use clap::Parser;

use cli::Cli;
use email::RawMessage;
use error::AppError;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
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
    let documents = document::process_attachments(parsed.attachments());
    let message = parsed.into_email(documents);
    output::write_json(&message, cli.pretty)?;
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
