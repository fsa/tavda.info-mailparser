use super::detector::Format;
use super::error::DocumentError;
use super::ExtractedDocument;
use crate::email::Attachment;

/// A text extractor for one document format.
///
/// Adding support for a new format means implementing this trait (usually in
/// a new module) and registering the instance in [`extractor_for`]; nothing
/// else in the application needs to change.
pub trait DocumentExtractor: Sync {
    fn extract(&self, attachment: &Attachment) -> Result<ExtractedDocument, DocumentError>;
}

static DOCX_EXTRACTOR: super::docx::DocxExtractor = super::docx::DocxExtractor;
static LEGACY_DOC_EXTRACTOR: super::doc::LegacyDocExtractor = super::doc::LegacyDocExtractor;
static PDF_EXTRACTOR: super::pdf::PdfExtractor = super::pdf::PdfExtractor;

pub fn extractor_for(format: Format) -> Option<&'static dyn DocumentExtractor> {
    match format {
        Format::Docx => Some(&DOCX_EXTRACTOR),
        Format::LegacyDoc => Some(&LEGACY_DOC_EXTRACTOR),
        Format::Pdf => Some(&PDF_EXTRACTOR),
    }
}
