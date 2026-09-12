use crate::email::Attachment;

/// Document formats for which an extractor is registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Office Open XML (`.docx`), a ZIP container with `word/document.xml`.
    Docx,
    /// Legacy binary Word 97-2003 (`.doc`), an OLE2/CFB compound file.
    LegacyDoc,
    /// Portable Document Format (`.pdf`). There is no in-process decoder: text
    /// is extracted by the external `pdftotext` tool (poppler-utils).
    Pdf,
}

impl Format {
    /// Canonical MIME type of the format actually used for decoding.
    pub fn mime_type(self) -> &'static str {
        match self {
            Format::Docx => DOCX_MIME,
            Format::LegacyDoc => DOC_MIME,
            Format::Pdf => PDF_MIME,
        }
    }
}

const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const ZIP_MAGIC: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];
const PDF_MAGIC: [u8; 5] = *b"%PDF-";

const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const DOC_MIME: &str = "application/msword";
const PDF_MIME: &str = "application/pdf";

/// Decides which extractor should handle the attachment.
///
/// Detection is extension-based first (that is what users see), then the
/// declared MIME type, with content sniffing as a last resort for attachments
/// without a usable file name.
pub fn detect(attachment: &Attachment) -> Option<Format> {
    by_extension(attachment)
        .or_else(|| by_mime_type(attachment))
        .or_else(|| by_magic(attachment))
}

fn by_extension(attachment: &Attachment) -> Option<Format> {
    let ext = attachment.filename()?.extension()?;
    match ext.as_str() {
        "docx" | "docm" | "dotx" | "dotm" => Some(Format::Docx),
        "doc" | "dot" => Some(Format::LegacyDoc),
        "pdf" => Some(Format::Pdf),
        _ => None,
    }
}

fn by_mime_type(attachment: &Attachment) -> Option<Format> {
    match attachment.content_type()?.as_str() {
        DOCX_MIME => Some(Format::Docx),
        DOC_MIME => Some(Format::LegacyDoc),
        PDF_MIME => Some(Format::Pdf),
        _ => None,
    }
}

fn by_magic(attachment: &Attachment) -> Option<Format> {
    let bytes = attachment.contents().as_bytes();
    if bytes.starts_with(&CFB_MAGIC) {
        Some(Format::LegacyDoc)
    } else if bytes.starts_with(&ZIP_MAGIC) {
        Some(Format::Docx)
    } else if bytes.starts_with(&PDF_MAGIC) {
        Some(Format::Pdf)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::{Attachment, AttachmentContents, FileName};

    fn attachment(name: Option<&str>, bytes: &[u8]) -> Attachment {
        Attachment::new(
            name.map(|n| FileName(n.to_string())),
            None,
            AttachmentContents::new(bytes.to_vec()),
        )
    }

    const ZIP_BYTES: &[u8] = b"PK\x03\x04 followed by data";
    const CFB_BYTES: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0x00];
    const PDF_BYTES: &[u8] = b"%PDF-1.7";

    #[test]
    fn detects_office_extensions() {
        assert_eq!(detect(&attachment(Some("a.docx"), b"")), Some(Format::Docx));
        assert_eq!(detect(&attachment(Some("b.DOC"), b"")), Some(Format::LegacyDoc));
        assert_eq!(detect(&attachment(Some("c.docm"), b"")), Some(Format::Docx));
        assert_eq!(detect(&attachment(Some("d.dot"), b"")), Some(Format::LegacyDoc));
        assert_eq!(detect(&attachment(Some("e.pdf"), b"")), Some(Format::Pdf));
    }

    #[test]
    fn unknown_extension_falls_back_to_content_sniffing() {
        assert_eq!(detect(&attachment(Some("f.xyz"), ZIP_BYTES)), Some(Format::Docx));
        assert_eq!(detect(&attachment(Some("f.xyz"), CFB_BYTES)), Some(Format::LegacyDoc));
        assert_eq!(detect(&attachment(Some("f.xyz"), PDF_BYTES)), Some(Format::Pdf));
        assert_eq!(detect(&attachment(Some("f.xyz"), b"plain")), None);
    }

    #[test]
    fn detects_format_by_magic_without_name() {
        assert_eq!(detect(&attachment(None, ZIP_BYTES)), Some(Format::Docx));
        assert_eq!(detect(&attachment(None, CFB_BYTES)), Some(Format::LegacyDoc));
        assert_eq!(detect(&attachment(None, PDF_BYTES)), Some(Format::Pdf));
        assert_eq!(detect(&attachment(None, b"\x00\x01")), None);
    }

    #[test]
    fn extension_takes_priority_over_content() {
        assert_eq!(detect(&attachment(Some("real.doc"), ZIP_BYTES)), Some(Format::LegacyDoc));
    }

    #[test]
    fn mime_type_is_used_when_extension_is_unknown() {
        let docx = Attachment::new(
            Some(FileName("f.xyz".to_string())),
            Some(crate::email::MimeType(DOCX_MIME.to_string())),
            AttachmentContents::new(b"??".to_vec()),
        );
        assert_eq!(detect(&docx), Some(Format::Docx));

        let doc = Attachment::new(
            None,
            Some(crate::email::MimeType(DOC_MIME.to_string())),
            AttachmentContents::new(Vec::new()),
        );
        assert_eq!(detect(&doc), Some(Format::LegacyDoc));
    }
}
