use std::io;

use crate::email::Email;

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("writing to stdout failed: {0}")]
    Io(#[from] io::Error),
}

/// Renders the single JSON document for a message.
///
/// Compact by default; `pretty` switches to 2-space indentation. The caller
/// decides where the bytes go — the result is always delivered on stdout and
/// may additionally be archived by `--log-dir`.
pub fn render_json(email: &Email, pretty: bool) -> Result<String, OutputError> {
    if pretty {
        to_pretty_json(email)
    } else {
        to_compact_json(email)
    }
}

pub(crate) fn to_pretty_json(email: &Email) -> Result<String, OutputError> {
    Ok(serde_json::to_string_pretty(email)?)
}

pub(crate) fn to_compact_json(email: &Email) -> Result<String, OutputError> {
    Ok(serde_json::to_string(email)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, ExtractedDocument};
    use crate::email::{EmailMetadata, IsoDateTime, MessageId, Recipient, Sender, Subject};

    fn sample_email() -> Email {
        let metadata = EmailMetadata {
            message_id: Some(MessageId("<x@y>".into())),
            subject: Some(Subject("Тема письма".into())),
            sender: Some(Sender("s@y".into())),
            to: Some(Recipient("robot@y".into())),
            date: Some(IsoDateTime("2026-08-21T16:30:00Z".into())),
        };
        let documents = vec![
            Document::Ok {
                filename: Some("a.docx".into()),
                content_type: Some(
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                        .into(),
                ),
                detected_content_type: Some(
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                        .into(),
                ),
                paragraphs: ExtractedDocument::new(vec!["Один".into(), "Два".into()])
                    .into_paragraphs(),
            },
            Document::Unsupported {
                filename: None,
                content_type: None,
            },
        ];
        Email::new(
            metadata,
            Some("Текст письма".to_string()),
            documents,
        )
    }

    #[test]
    fn renders_utf8_and_structure() {
        let json = to_pretty_json(&sample_email()).unwrap();
        assert!(json.contains("\"message_id\": \"<x@y>\""));
        assert!(json.contains("\"subject\": \"Тема письма\""));
        assert!(json.contains("\"date\": \"2026-08-21T16:30:00Z\""));
        assert!(json.contains("\"body\": \"Текст письма\""));
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

    #[test]
    fn compact_json_has_no_padding() {
        let json = to_compact_json(&sample_email()).unwrap();
        assert!(!json.contains(": "));
        assert!(!json.contains('\n'));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["documents"].as_array().unwrap().len(), 2);
    }
}
