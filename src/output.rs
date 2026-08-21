use std::io::{self, Write};

use crate::email::Email;

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("writing to stdout failed: {0}")]
    Io(#[from] io::Error),
}

/// Writes the single JSON document to stdout.
///
/// Diagnostics never go here: stdout must stay machine-parseable.
pub fn write_json(email: &Email) -> Result<(), OutputError> {
    let json = to_pretty_json(email)?;
    let mut lock = io::stdout().lock();
    lock.write_all(json.as_bytes())?;
    lock.write_all(b"\n")?;
    lock.flush()?;
    Ok(())
}

pub(crate) fn to_pretty_json(email: &Email) -> Result<String, OutputError> {
    Ok(serde_json::to_string_pretty(email)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, ExtractedDocument};
    use crate::email::{EmailMetadata, IsoDateTime, MessageId, Sender, Subject};

    fn sample_email() -> Email {
        let metadata = EmailMetadata {
            message_id: Some(MessageId("<x@y>".into())),
            subject: Some(Subject("Тема письма".into())),
            sender: Some(Sender("s@y".into())),
            date: Some(IsoDateTime("2026-08-21T16:30:00Z".into())),
        };
        let documents = vec![
            Document::Ok {
                filename: Some("a.docx".into()),
                paragraphs: ExtractedDocument::new(vec!["Один".into(), "Два".into()])
                    .into_paragraphs(),
            },
            Document::Unsupported { filename: None },
        ];
        Email::new(metadata, documents)
    }

    #[test]
    fn renders_utf8_and_structure() {
        let json = to_pretty_json(&sample_email()).unwrap();
        assert!(json.contains("\"message_id\": \"<x@y>\""));
        assert!(json.contains("\"subject\": \"Тема письма\""));
        assert!(json.contains("\"date\": \"2026-08-21T16:30:00Z\""));
        assert!(json.contains("\"status\": \"ok\""));
        assert!(json.contains("\"filename\": null"));
        // binary data must never leak into the output
        assert!(!json.contains('\u{0}'));
    }

    #[test]
    fn output_is_valid_json() {
        let json = to_pretty_json(&sample_email()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["documents"].as_array().unwrap().len(), 2);
    }
}
