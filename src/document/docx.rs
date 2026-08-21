use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

use super::error::DocumentError;
use super::extractor::DocumentExtractor;
use super::ExtractedDocument;
use crate::email::Attachment;

/// Extracts text from Office Open XML `.docx` files.
///
/// Only `word/document.xml` is read: body paragraphs, including those inside
/// tables. Headers, footers, footnotes and text boxes are not extracted.
pub struct DocxExtractor;

impl DocumentExtractor for DocxExtractor {
    fn extract(&self, attachment: &Attachment) -> Result<ExtractedDocument, DocumentError> {
        let reader = Cursor::new(attachment.contents().as_bytes());
        let mut archive =
            ZipArchive::new(reader).map_err(|err| corrupt(format!("invalid ZIP container: {err}")))?;

        let mut entry = archive
            .by_name(MAIN_DOCUMENT)
            .map_err(|_| corrupt("word/document.xml is missing; not a WordprocessingML document"))?;

        let mut xml = String::with_capacity(entry.size() as usize);
        entry
            .read_to_string(&mut xml)
            .map_err(|err| corrupt(format!("cannot read word/document.xml: {err}")))?;

        Ok(ExtractedDocument::new(paragraphs_from_xml(&xml)?))
    }
}

const MAIN_DOCUMENT: &str = "word/document.xml";

/// Streams through the WordprocessingML and collects `<w:t>` runs per `<w:p>`.
fn paragraphs_from_xml(xml: &str) -> Result<Vec<String>, DocumentError> {
    let mut reader = Reader::from_str(xml);
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    // Depth counters instead of a full element stack: only two elements matter.
    let mut in_paragraph = false;
    let mut text_depth: usize = 0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => match element.name().local_name().as_ref() {
                b"p" => {
                    current.clear();
                    in_paragraph = true;
                }
                b"t" => text_depth += 1,
                b"tab" if in_paragraph => current.push('\t'),
                b"br" | b"cr" if in_paragraph => current.push('\n'),
                _ => {}
            },
            Ok(Event::Empty(element)) => match element.name().local_name().as_ref() {
                b"tab" if in_paragraph => current.push('\t'),
                b"br" | b"cr" if in_paragraph => current.push('\n'),
                _ => {}
            },
            Ok(Event::End(element)) => match element.name().local_name().as_ref() {
                b"p" => {
                    in_paragraph = false;
                    let paragraph = current.trim();
                    if !paragraph.is_empty() {
                        paragraphs.push(paragraph.to_string());
                    }
                }
                b"t" => text_depth = text_depth.saturating_sub(1),
                _ => {}
            },
            Ok(Event::Text(text)) => {
                if in_paragraph && text_depth > 0 {
                    let decoded = text
                        .decode()
                        .map_err(|err| corrupt(format!("malformed XML text: {err}")))?;
                    current.push_str(&decoded);
                }
            }
            // quick-xml emits `&amp;`-style references as separate events
            Ok(Event::GeneralRef(reference)) => {
                if in_paragraph && text_depth > 0 {
                    if let Some(resolved) = resolve_reference(&reference) {
                        current.push_str(&resolved);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(corrupt(format!("malformed XML: {err}"))),
            _ => {}
        }
    }

    Ok(paragraphs)
}

/// Resolves an XML entity reference to its text: character references
/// (`&#38;`, `&#x26;`) plus the five predefined XML entities. Unknown named
/// entities are skipped — WordprocessingML content only uses predefined ones.
fn resolve_reference(reference: &quick_xml::events::BytesRef<'_>) -> Option<String> {
    if let Ok(Some(ch)) = reference.resolve_char_ref() {
        return Some(ch.to_string());
    }
    match reference.decode().as_deref() {
        Ok("amp") => Some("&".into()),
        Ok("lt") => Some("<".into()),
        Ok("gt") => Some(">".into()),
        Ok("apos") => Some("'".into()),
        Ok("quot") => Some("\"".into()),
        _ => None,
    }
}

fn corrupt(message: impl Into<String>) -> DocumentError {
    DocumentError::Corrupt(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Paragraph, testutil};
    use crate::email::{Attachment, AttachmentContents, FileName};

    fn attachment(bytes: Vec<u8>) -> Attachment {
        Attachment::new(
            Some(FileName("letter.docx".to_string())),
            None,
            AttachmentContents::new(bytes),
        )
    }

    #[test]
    fn collects_paragraphs_runs_entities_and_breaks() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:p><w:r><w:t>Пер</w:t></w:r><w:r><w:t>вый</w:t></w:r></w:p>
<w:p><w:r><w:t>a&amp;b</w:t><w:tab/><w:t>c</w:t></w:r></w:p>
<w:p/>
<w:p><w:r><w:t>line<w:br/>break</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>в таблице</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<n:p xmlns:n="urn:other"><n:t>другой префикс</n:t></n:p>
</w:body>
</w:document>"#;

        assert_eq!(
            paragraphs_from_xml(xml).unwrap(),
            vec![
                "Первый".to_string(),
                "a&b\tc".to_string(),
                "line\nbreak".to_string(),
                "в таблице".to_string(),
                "другой префикс".to_string(),
            ]
        );
    }

    #[test]
    fn drops_empty_and_whitespace_only_paragraphs() {
        let xml = "<w:document xmlns:w=\"urn\"><w:body>\
                   <w:p><w:r><w:t>  </w:t></w:r></w:p>\
                   <w:p><w:r><w:t>текст</w:t></w:r></w:p>\
                   </w:body></w:document>";
        assert_eq!(paragraphs_from_xml(xml).unwrap(), vec!["текст".to_string()]);
    }

    #[test]
    fn extracts_text_from_zip_container() {
        let bytes = testutil::docx(&["Абзац один", "Абзац два"]);
        let document = DocxExtractor.extract(&attachment(bytes)).unwrap();
        assert_eq!(
            document.into_paragraphs(),
            vec![Paragraph("Абзац один".into()), Paragraph("Абзац два".into())]
        );
    }

    #[test]
    fn zip_without_main_document_is_corrupt() {
        let mut zip = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("other.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, b"<x/>").unwrap();
        let bytes = zip.finish().unwrap().into_inner();

        assert!(matches!(
            DocxExtractor.extract(&attachment(bytes)),
            Err(DocumentError::Corrupt(_))
        ));
    }

    #[test]
    fn non_zip_input_is_corrupt() {
        assert!(matches!(
            DocxExtractor.extract(&attachment(b"not a zip".to_vec())),
            Err(DocumentError::Corrupt(_))
        ));
    }
}
