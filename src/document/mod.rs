pub mod detector;
pub mod doc;
pub mod docx;
pub mod error;
pub mod extractor;

use serde::Serialize;

use super::email::Attachment;
use error::DocumentErrorInfo;

/// A single text paragraph of an extracted document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Paragraph(pub String);

/// Text extracted from a document: paragraphs in document order.
///
/// Empty paragraphs are dropped; this is the normalized form used everywhere
/// downstream (JSON output today, rules/regex processing later).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedDocument {
    paragraphs: Vec<Paragraph>,
}

impl ExtractedDocument {
    pub fn new(paragraphs: Vec<String>) -> Self {
        Self {
            paragraphs: paragraphs.into_iter().map(Paragraph).collect(),
        }
    }

    pub fn into_paragraphs(self) -> Vec<Paragraph> {
        self.paragraphs
    }
}

/// Processing result for one attachment.
///
/// The `status` tag mirrors the three possible outcomes: successful text
/// extraction, unsupported format, or a per-document error.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Document {
    Ok {
        filename: Option<String>,
        paragraphs: Vec<Paragraph>,
    },
    Unsupported {
        filename: Option<String>,
    },
    Error {
        filename: Option<String>,
        error: DocumentErrorInfo,
    },
}

/// Runs every attachment through the detector + extractor pipeline.
pub fn process_attachments(attachments: &[Attachment]) -> Vec<Document> {
    attachments.iter().map(process_attachment).collect()
}

fn process_attachment(attachment: &Attachment) -> Document {
    let filename = attachment.filename().map(|name| name.as_str().to_string());

    let Some(format) = detector::detect(attachment) else {
        warn(&filename, "unsupported format");
        return Document::Unsupported { filename };
    };

    let Some(extractor) = extractor::extractor_for(format) else {
        warn(&filename, "no extractor registered");
        return Document::Unsupported { filename };
    };

    match extractor.extract(attachment) {
        Ok(document) => Document::Ok {
            filename,
            paragraphs: document.into_paragraphs(),
        },
        Err(err) => {
            warn(&filename, &err.to_string());
            Document::Error {
                filename,
                error: (&err).into(),
            }
        }
    }
}

fn warn(filename: &Option<String>, reason: &str) {
    match filename {
        Some(name) => eprintln!("warning: document `{name}`: {reason}"),
        None => eprintln!("warning: unnamed document: {reason}"),
    }
}

/// Helpers shared by unit tests of the `document` modules.
#[cfg(test)]
pub(crate) mod testutil {
    use crate::email::{Attachment, AttachmentContents, FileName};

    pub(crate) fn attachment(name: &str, contents: &[u8]) -> Attachment {
        Attachment::new(
            Some(FileName(name.to_string())),
            None,
            AttachmentContents::new(contents.to_vec()),
        )
    }

    /// Builds an in-memory `.docx` (ZIP with only `word/document.xml`).
    pub(crate) fn docx(paragraphs: &[&str]) -> Vec<u8> {
        use std::io::Write as _;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let body = paragraphs
            .iter()
            .map(|p| format!("<w:p><w:r><w:t>{p}</w:t></w:r></w:p>"))
            .collect::<String>();
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
             <w:body>{body}</w:body></w:document>"
        );

        let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::testutil;
    use super::*;
    use serde_json::json;

    #[test]
    fn produces_ok_unsupported_and_error_statuses() {
        let attachments = vec![
            testutil::attachment("letter.docx", &testutil::docx(&["Первый", "Второй"])),
            testutil::attachment("unknown.xyz", &[0x00, 0x01, 0x02]),
            // CFB magic but garbage inside -> per-document error
            testutil::attachment(
                "broken.doc",
                &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0x00, 0x00],
            ),
        ];

        let documents = process_attachments(&attachments);
        assert_eq!(documents.len(), 3);
        assert!(matches!(documents[0], Document::Ok { .. }));
        assert!(matches!(documents[1], Document::Unsupported { .. }));
        assert!(matches!(documents[2], Document::Error { .. }));
    }

    #[test]
    fn json_shapes_match_spec() {
        let attachments = vec![
            testutil::attachment("letter.docx", &testutil::docx(&["Один", "Два"])),
            testutil::attachment("unknown.xyz", b"???"),
            testutil::attachment("broken.doc", &[0xD0, 0xCF, 0x11, 0xE0]),
        ];
        let documents = process_attachments(&attachments);

        let ok = serde_json::to_value(&documents[0]).unwrap();
        assert_eq!(
            ok,
            json!({
                "status": "ok",
                "filename": "letter.docx",
                "paragraphs": ["Один", "Два"],
            })
        );

        let unsupported = serde_json::to_value(&documents[1]).unwrap();
        assert_eq!(
            unsupported,
            json!({ "status": "unsupported", "filename": "unknown.xyz" })
        );

        let error = serde_json::to_value(&documents[2]).unwrap();
        assert_eq!(error["status"], "error");
        assert_eq!(error["filename"], "broken.doc");
        assert_eq!(error["error"]["code"], "corrupt_document");
        assert!(!error["error"]["message"].as_str().unwrap().is_empty());
    }
}
