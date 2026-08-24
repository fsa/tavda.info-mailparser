pub mod parser;

use serde::Serialize;

/// Raw bytes of an RFC 5322 / MIME message exactly as received.
///
/// Wrapping the bytes in a dedicated type keeps the boundary between
/// "transport" (stdin, file, later: spool or HTTP) and parsing explicit.
#[derive(Debug, Clone, Copy)]
pub struct RawMessage<'a>(&'a [u8]);

impl<'a> RawMessage<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &'a [u8] {
        self.0
    }
}

/// `Message-ID` header value, e.g. `<abc123@example.com>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MessageId(pub String);

/// Decoded (RFC 2047) subject line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Subject(pub String);

/// Sender address, e.g. `sender@example.com`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Sender(pub String);

/// Primary recipient address from the `To` header (first entry; the display
/// name is used only when the bare address is missing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Recipient(pub String);

/// Message date normalized to UTC ISO 8601 (`2026-08-21T16:30:00Z`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct IsoDateTime(pub String);

/// Attachment file name decoded from MIME headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileName(pub String);

impl FileName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Lower-cased extension without the leading dot, if present.
    pub fn extension(&self) -> Option<String> {
        let raw = self.0.rsplit_once('.')?.1;
        if raw.is_empty() {
            None
        } else {
            Some(raw.to_ascii_lowercase())
        }
    }
}

/// MIME content type of an attachment, e.g. `application/msword`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeType(pub String);

impl MimeType {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Binary contents of a single attachment. Never serialized to the output.
#[derive(Debug, Clone)]
pub struct AttachmentContents(Vec<u8>);

impl AttachmentContents {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A single attachment extracted from the message.
#[derive(Debug, Clone)]
pub struct Attachment {
    filename: Option<FileName>,
    content_type: Option<MimeType>,
    contents: AttachmentContents,
}

impl Attachment {
    pub fn new(
        filename: Option<FileName>,
        content_type: Option<MimeType>,
        contents: AttachmentContents,
    ) -> Self {
        Self {
            filename,
            content_type,
            contents,
        }
    }

    pub fn filename(&self) -> Option<&FileName> {
        self.filename.as_ref()
    }

    pub fn content_type(&self) -> Option<&MimeType> {
        self.content_type.as_ref()
    }

    pub fn contents(&self) -> &AttachmentContents {
        &self.contents
    }
}

/// Fatal errors of message-level parsing.
#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    #[error("input is not a parseable RFC 5322 / MIME message")]
    Unparsable,
}

/// Metadata parsed from the message headers, before document processing.
#[derive(Debug, Clone, Default)]
pub struct EmailMetadata {
    pub message_id: Option<MessageId>,
    pub subject: Option<Subject>,
    pub sender: Option<Sender>,
    pub to: Option<Recipient>,
    pub date: Option<IsoDateTime>,
}

/// Intermediate parse result: metadata plus raw attachments.
///
/// Document extraction happens after parsing so that the two stages stay
/// independent (see `document::process_attachments`).
#[derive(Debug)]
pub struct ParsedEmail {
    metadata: EmailMetadata,
    attachments: Vec<Attachment>,
}

impl ParsedEmail {
    pub fn new(metadata: EmailMetadata, attachments: Vec<Attachment>) -> Self {
        Self {
            metadata,
            attachments,
        }
    }

    pub fn attachments(&self) -> &[Attachment] {
        &self.attachments
    }

    pub fn into_email(self, documents: Vec<crate::document::Document>) -> Email {
        Email::new(self.metadata, documents)
    }
}

/// Final normalized representation of one message; this is what gets serialized.
#[derive(Debug, Serialize)]
pub struct Email {
    message_id: Option<MessageId>,
    subject: Option<Subject>,
    sender: Option<Sender>,
    to: Option<Recipient>,
    date: Option<IsoDateTime>,
    documents: Vec<crate::document::Document>,
}

impl Email {
    pub fn new(metadata: EmailMetadata, documents: Vec<crate::document::Document>) -> Self {
        Self {
            message_id: metadata.message_id,
            subject: metadata.subject,
            sender: metadata.sender,
            to: metadata.to,
            date: metadata.date,
            documents,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FileName, MimeType};

    #[test]
    fn extension_is_lowercased() {
        assert_eq!(FileName("Report.DOCX".into()).extension().as_deref(), Some("docx"));
        assert_eq!(FileName("archive.tar.gz".into()).extension().as_deref(), Some("gz"));
        assert_eq!(FileName("noext".into()).extension(), None);
        assert_eq!(FileName("trailing.".into()).extension(), None);
    }

    #[test]
    fn mime_type_keeps_subtype() {
        assert_eq!(MimeType("application/msword".into()).as_str(), "application/msword");
    }
}
