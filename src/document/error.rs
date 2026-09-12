use serde::Serialize;

/// Errors produced while processing a single document.
///
/// These never abort the whole message; they are rendered into the
/// `status: "error"` JSON variant of a document.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("document is encrypted")]
    Encrypted,
    #[error("document structure is corrupt: {0}")]
    Corrupt(String),
    #[error("text extraction failed: {0}")]
    Extraction(String),
    #[error("no decoder is installed for this format: `{0}`")]
    MissingDecoder(String),
}

/// Machine-readable error code used in the JSON output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    TextExtractionFailed,
    CorruptDocument,
    EncryptedDocument,
    MissingDecoder,
}

/// Serialized form of a document processing error.
#[derive(Debug, Clone, Serialize)]
pub struct DocumentErrorInfo {
    pub code: ErrorCode,
    pub message: String,
}

impl From<&DocumentError> for DocumentErrorInfo {
    fn from(err: &DocumentError) -> Self {
        let (code, message) = match err {
            DocumentError::Encrypted => (ErrorCode::EncryptedDocument, err.to_string()),
            DocumentError::Corrupt(_) => (ErrorCode::CorruptDocument, err.to_string()),
            DocumentError::Extraction(_) => (ErrorCode::TextExtractionFailed, err.to_string()),
            DocumentError::MissingDecoder(_) => (ErrorCode::MissingDecoder, err.to_string()),
        };
        Self {
            code,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_errors_to_stable_codes() {
        let cases = [
            (DocumentError::Encrypted, ErrorCode::EncryptedDocument),
            (DocumentError::Corrupt("x".into()), ErrorCode::CorruptDocument),
            (
                DocumentError::Extraction("y".into()),
                ErrorCode::TextExtractionFailed,
            ),
            (DocumentError::MissingDecoder("pdftotext".into()), ErrorCode::MissingDecoder),
        ];
        for (err, expected) in cases {
            let info = DocumentErrorInfo::from(&err);
            assert_eq!(info.code, expected);
            assert!(!info.message.is_empty());
        }
    }

    #[test]
    fn codes_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_value(ErrorCode::TextExtractionFailed).unwrap(),
            "text_extraction_failed"
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::EncryptedDocument).unwrap(),
            "encrypted_document"
        );
    }
}
