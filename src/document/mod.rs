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
/// extraction, unsupported format, or a per-document error. `content_type`
/// carries the MIME type declared in the message headers (`null` when the
/// part had none); `detected_content_type` is the canonical MIME type of the
/// format actually used for decoding, so it diverges from `content_type`
/// whenever detection relied on extension or content sniffing. It is present
/// only when a decoder was picked (`ok`/`error`); nothing was decoded for
/// `unsupported`.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Document {
    Ok {
        filename: Option<String>,
        content_type: Option<String>,
        detected_content_type: Option<String>,
        paragraphs: Vec<Paragraph>,
    },
    Unsupported {
        filename: Option<String>,
        content_type: Option<String>,
    },
    Error {
        filename: Option<String>,
        content_type: Option<String>,
        detected_content_type: Option<String>,
        error: DocumentErrorInfo,
    },
}

/// Runs every attachment through the detector + extractor pipeline.
pub fn process_attachments(attachments: &[Attachment]) -> Vec<Document> {
    attachments.iter().map(process_attachment).collect()
}

fn process_attachment(attachment: &Attachment) -> Document {
    let filename = attachment.filename().map(|name| name.as_str().to_string());
    let content_type = attachment.content_type().map(|ct| ct.as_str().to_string());

    let Some(format) = detector::detect(attachment) else {
        warn(&filename, "unsupported format");
        return Document::Unsupported {
            filename,
            content_type,
        };
    };

    let Some(extractor) = extractor::extractor_for(format) else {
        warn(&filename, "no extractor registered");
        return Document::Unsupported {
            filename,
            content_type,
        };
    };
    let detected_content_type = format.mime_type().to_string();

    match extractor.extract(attachment) {
        Ok(document) => Document::Ok {
            filename,
            content_type,
            detected_content_type: Some(detected_content_type),
            paragraphs: document.into_paragraphs(),
        },
        Err(err) => {
            warn(&filename, &err.to_string());
            Document::Error {
                filename,
                content_type,
                detected_content_type: Some(detected_content_type),
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

    pub(crate) fn attachment_with_mime(name: &str, mime: &str, contents: &[u8]) -> Attachment {
        Attachment::new(
            Some(FileName(name.to_string())),
            Some(crate::email::MimeType(mime.to_string())),
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
            testutil::attachment_with_mime(
                "letter.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &testutil::docx(&["Один", "Два"]),
            ),
            testutil::attachment("unknown.xyz", b"???"),
            testutil::attachment_with_mime("broken.doc", "application/msword", &[0xD0, 0xCF, 0x11, 0xE0]),
        ];
        let documents = process_attachments(&attachments);

        let ok = serde_json::to_value(&documents[0]).unwrap();
        assert_eq!(
            ok,
            json!({
                "status": "ok",
                "filename": "letter.docx",
                "content_type": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "detected_content_type": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "paragraphs": ["Один", "Два"],
            })
        );

        let unsupported = serde_json::to_value(&documents[1]).unwrap();
        assert_eq!(
            unsupported,
            json!({
                "status": "unsupported",
                "filename": "unknown.xyz",
                "content_type": null,
            })
        );
        assert!(unsupported.get("detected_content_type").is_none());

        let error = serde_json::to_value(&documents[2]).unwrap();
        assert_eq!(error["status"], "error");
        assert_eq!(error["filename"], "broken.doc");
        assert_eq!(error["content_type"], "application/msword");
        assert_eq!(error["detected_content_type"], "application/msword");
        assert_eq!(error["error"]["code"], "corrupt_document");
        assert!(!error["error"]["message"].as_str().unwrap().is_empty());
    }

    #[test]
    fn detected_type_reflects_actual_decoder_not_declared_header() {
        // Declared octet-stream but detected as DOCX by extension: the
        // declared header value must be preserved verbatim, and the type of
        // the decoder actually used must be reported alongside it.
        let attachments = vec![testutil::attachment_with_mime(
            "letter.docx",
            "application/octet-stream",
            &testutil::docx(&["Текст"]),
        )];
        let ok = serde_json::to_value(&process_attachments(&attachments)[0]).unwrap();
        assert_eq!(ok["content_type"], "application/octet-stream");
        assert_eq!(ok["status"], "ok");
        assert_eq!(
            ok["detected_content_type"],
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
    }
}
