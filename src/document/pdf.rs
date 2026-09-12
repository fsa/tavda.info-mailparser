use std::io::{self, Write};
use std::process::{Command, Stdio};

use super::ExtractedDocument;
use super::error::DocumentError;
use super::extractor::DocumentExtractor;
use crate::email::Attachment;

/// Extracts text from PDF documents.
///
/// There is no PDF decoder inside the binary; extraction is delegated to the
/// `pdftotext` tool from the `poppler-utils` Debian package, which is
/// installed on the host and looked up on `PATH`. The attachment bytes are
/// piped to the tool via stdin, so no temporary files are needed, and the
/// `-layout` flag preserves the column structure of formatted documents.
///
/// When `pdftotext` is not installed the error [`DocumentError::MissingDecoder`]
/// is reported: the format is recognized and supported, but the host lacks the
/// decoder. Downstream consumers can tell this apart from an unsupported
/// format or a corrupt document via the `status: "missing_decoder"` variant.
pub struct PdfExtractor;

const PDFTOTEXT: &str = "pdftotext";

impl DocumentExtractor for PdfExtractor {
    fn extract(&self, attachment: &Attachment) -> Result<ExtractedDocument, DocumentError> {
        let mut child = Command::new(PDFTOTEXT)
            .args(["-layout", "-enc", "UTF-8", "-", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(start_error)?;

        child
            .stdin
            .take()
            .expect("stdin must be piped")
            .write_all(attachment.contents().as_bytes())
            .map_err(|err| {
                DocumentError::Extraction(format!("cannot write the PDF to {PDFTOTEXT}: {err}"))
            })?;

        let output = child
            .wait_with_output()
            .map_err(|err| DocumentError::Extraction(format!("cannot run {PDFTOTEXT}: {err}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DocumentError::Extraction(format!(
                "{PDFTOTEXT} failed (exit {:?}): {}",
                output.status.code(),
                stderr.trim()
            )));
        }

        let text = String::from_utf8(output.stdout).map_err(|err| {
            DocumentError::Extraction(format!("{PDFTOTEXT} produced non-UTF-8 output: {err}"))
        })?;
        let paragraphs = split_paragraphs(&text);
        if paragraphs.is_empty() {
            return Err(DocumentError::Extraction(
                "no text extracted; the PDF may be a scanned image".to_string(),
            ));
        }
        Ok(ExtractedDocument::new(paragraphs))
    }
}

/// Maps a spawn failure to `MissingDecoder` when the tool is simply absent.
fn start_error(err: io::Error) -> DocumentError {
    if err.kind() == io::ErrorKind::NotFound {
        DocumentError::MissingDecoder(PDFTOTEXT.to_string())
    } else {
        DocumentError::Extraction(format!("cannot start {PDFTOTEXT}: {err}"))
    }
}

/// Splits `pdftotext` output into paragraphs.
///
/// `pdftotext` separates pages with the form feed character and emits one
/// visual line per output line, so consecutive non-empty lines of the same
/// block are joined into a single paragraph (this keeps multi-line paragraphs
/// readable in the semi-structured `-layout` form). Blank lines and page
/// separators end a paragraph.
fn split_paragraphs(text: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    for page in text.split('\x0C') {
        let mut current = Vec::new();
        for line in page.split('\n') {
            let line = line.trim();
            if line.is_empty() {
                flush(&mut paragraphs, &mut current);
            } else {
                current.push(line.to_string());
            }
        }
        flush(&mut paragraphs, &mut current);
    }
    paragraphs
}

fn flush(paragraphs: &mut Vec<String>, current: &mut Vec<String>) {
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
        current.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_consecutive_lines_into_one_paragraph() {
        let out = "Первая строка\nвторая строка\n\nНовый абзац";
        assert_eq!(
            split_paragraphs(out),
            vec!["Первая строка вторая строка", "Новый абзац"]
        );
    }

    #[test]
    fn page_feed_starts_a_new_paragraph() {
        let out = "Страница один\x0CСтраница два";
        assert_eq!(split_paragraphs(out), vec!["Страница один", "Страница два"]);
    }

    #[test]
    fn trims_and_drops_empty_lines() {
        let out = "   с отступами  \n\n\n   \nвторая";
        assert_eq!(split_paragraphs(out), vec!["с отступами", "вторая"]);
    }

    #[test]
    fn empty_output_yields_no_paragraphs() {
        assert!(split_paragraphs("\n  \n\x0C\n").is_empty());
    }
}
