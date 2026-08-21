use std::io::{Cursor, Read};

use cfb::CompoundFile;

use super::error::DocumentError;
use super::extractor::DocumentExtractor;
use super::ExtractedDocument;
use crate::email::Attachment;

/// Extracts text from legacy binary Word documents (`.doc`, Word 97-2003).
///
/// The file is an OLE2/CFB compound file. Text is located through the piece
/// table (CLX in the table stream), which also handles fast-saved documents.
/// Only the main body text is extracted (`ccpText` range): headers, footers,
/// footnotes and text boxes are skipped.
pub struct LegacyDocExtractor;

const W_IDENT: u16 = 0xA5EC;

// FIB (File Information Block) offsets used below, per [MS-DOC].
const OFF_W_IDENT: usize = 0x00;
const OFF_FIB_FLAGS: usize = 0x0A;
const OFF_CCP_TEXT: usize = 0x4C;
const OFF_FC_CLX: usize = 0x1A2;
const OFF_LCB_CLX: usize = 0x1A6;

const FLAG_ENCRYPTED: u16 = 0x0100;
const FLAG_WHICH_TBL_STM: u16 = 0x0200;

const FC_COMPRESSED: u32 = 0x4000_0000;
const FC_MASK: u32 = 0x3FFF_FFFF;

impl DocumentExtractor for LegacyDocExtractor {
    fn extract(&self, attachment: &Attachment) -> Result<ExtractedDocument, DocumentError> {
        let cursor = Cursor::new(attachment.contents().as_bytes());
        let mut compound = CompoundFile::open(cursor).map_err(|err| {
            DocumentError::Corrupt(format!("not a valid OLE2 compound file: {err}"))
        })?;

        let word_doc = read_stream(&mut compound, "/WordDocument")?;
        let w_ident = u16_at(&word_doc, OFF_W_IDENT)
            .ok_or_else(|| corrupt("WordDocument stream is too short"))?;
        if w_ident != W_IDENT {
            return Err(corrupt(format!(
                "missing Word 97 binary signature (found {w_ident:#06x})"
            )));
        }

        let flags = u16_at(&word_doc, OFF_FIB_FLAGS).unwrap_or(0);
        if flags & FLAG_ENCRYPTED != 0 {
            return Err(DocumentError::Encrypted);
        }

        // fWhichTblStm selects between the "0Table" and "1Table" streams.
        let table_name = if flags & FLAG_WHICH_TBL_STM != 0 {
            "/1Table"
        } else {
            "/0Table"
        };
        let table = read_stream(&mut compound, table_name)?;

        let fc_clx =
            u32_at(&word_doc, OFF_FC_CLX).ok_or_else(|| corrupt("FIB is truncated"))? as usize;
        let lcb_clx =
            u32_at(&word_doc, OFF_LCB_CLX).ok_or_else(|| corrupt("FIB is truncated"))? as usize;
        if lcb_clx == 0 {
            return Err(corrupt("piece table (CLX) is missing"));
        }
        let clx = slice(&table, fc_clx, lcb_clx)
            .ok_or_else(|| corrupt("CLX lies outside of the table stream"))?;

        let plc = locate_plcpcd(clx)?;
        let ccp_text = u32_at(&word_doc, OFF_CCP_TEXT).unwrap_or(0) as usize;
        let text = assemble_text(&word_doc, plc, ccp_text)?;

        Ok(ExtractedDocument::new(split_paragraphs(&text)))
    }
}

fn read_stream<F: Read + std::io::Seek>(
    compound: &mut CompoundFile<F>,
    name: &str,
) -> Result<Vec<u8>, DocumentError> {
    let mut stream = compound
        .open_stream(name)
        .map_err(|_| corrupt(format!("stream `{name}` not found in compound file")))?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|err| DocumentError::Extraction(format!("cannot read stream `{name}`: {err}")))?;
    Ok(buf)
}

/// Finds the PlcPcd inside the CLX blob, skipping Prc records on the way.
fn locate_plcpcd(clx: &[u8]) -> Result<&[u8], DocumentError> {
    let mut pos = 0usize;
    while pos < clx.len() {
        match clx[pos] {
            0x01 => {
                let len = u16_at(clx, pos + 1)
                    .ok_or_else(|| corrupt("truncated CLX"))?
                    as usize;
                pos += 3 + len;
            }
            0x02 => {
                let lcb = u32_at(clx, pos + 1)
                    .ok_or_else(|| corrupt("truncated CLX"))?
                    as usize;
                return slice(clx, pos + 5, lcb).ok_or_else(|| corrupt("PlcPcd out of bounds"));
            }
            tag => return Err(corrupt(format!("unexpected CLX tag {tag:#04x}"))),
        }
    }
    Err(corrupt("piece table descriptor not found"))
}

/// Walks the piece table and concatenates up to `ccp_text` characters of
/// main document text.
fn assemble_text(word_doc: &[u8], plc: &[u8], ccp_text: usize) -> Result<String, DocumentError> {
    if plc.len() < 4 || (plc.len() - 4) % 12 != 0 {
        return Err(corrupt("malformed piece table"));
    }
    let pieces = (plc.len() - 4) / 12;
    let mut text = String::with_capacity(ccp_text);
    let mut remaining = ccp_text;

    for piece in 0..pieces {
        if remaining == 0 {
            break;
        }
        let cp_start = u32_at(plc, piece * 4).ok_or_else(|| corrupt("truncated CP array"))?;
        let cp_end = u32_at(plc, (piece + 1) * 4).ok_or_else(|| corrupt("truncated CP array"))?;
        let pcd_off = 4 * (pieces + 1) + piece * 8;
        let fc = u32_at(plc, pcd_off + 2).ok_or_else(|| corrupt("truncated PCD array"))?;

        let take = (cp_end.saturating_sub(cp_start) as usize).min(remaining);
        if take == 0 {
            continue;
        }
        append_piece(word_doc, fc, take, &mut text)?;
        remaining -= take;
    }
    Ok(text)
}

fn append_piece(
    word_doc: &[u8],
    fc: u32,
    char_count: usize,
    text: &mut String,
) -> Result<(), DocumentError> {
    if fc & FC_COMPRESSED != 0 {
        // 8-bit ANSI piece; the byte offset is stored halved.
        let offset = ((fc & FC_MASK) / 2) as usize;
        let bytes = slice(word_doc, offset, char_count)
            .ok_or_else(|| corrupt("text piece out of bounds"))?;
        text.extend(bytes.iter().map(|&byte| cp1252_char(byte)));
    } else {
        let offset = (fc & FC_MASK) as usize;
        let bytes = slice(word_doc, offset, char_count * 2)
            .ok_or_else(|| corrupt("text piece out of bounds"))?;
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        text.extend(char::decode_utf16(units).flatten());
    }
    Ok(())
}

/// Word paragraph marks (`\r`) and table cell ends (`\x07`) become paragraph
/// separators; soft breaks stay inside a paragraph; stray control characters
/// are dropped or replaced.
fn split_paragraphs(text: &str) -> Vec<String> {
    let cleaned: String = text.chars().filter(|&c| c != '\x1F').collect();
    let normalized: String = cleaned
        .chars()
        .map(|c| match c {
            '\r' | '\n' | '\x07' => '\r',
            '\x0B' => '\n',
            '\x1E' => '-',
            c if c.is_control() && c != '\t' => ' ',
            c => c,
        })
        .collect();

    normalized
        .split('\r')
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(str::to_string)
        .collect()
}

/// Windows-1252 decoding for compressed pieces. Note: real documents use the
/// ANSI codepage of the authoring system (e.g. cp1251 for Russian), which we
/// do not detect; non-Latin-1 text in 8-bit pieces may be garbled.
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
    '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
];

fn cp1252_char(byte: u8) -> char {
    match byte {
        0x80..=0x9F => CP1252_HIGH[(byte - 0x80) as usize],
        _ => byte as char,
    }
}

fn u16_at(buf: &[u8], offset: usize) -> Option<u16> {
    let bytes = slice(buf, offset, 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes = slice(buf, offset, 4)?;
    Some(u32::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
    ]))
}

/// Overflow-safe `&buf[offset..offset + len]`.
fn slice(buf: &[u8], offset: usize, len: usize) -> Option<&[u8]> {
    buf.get(offset..offset.checked_add(len)?)
}

fn corrupt(message: impl Into<String>) -> DocumentError {
    DocumentError::Corrupt(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Paragraph;
    use crate::email::{Attachment, AttachmentContents, FileName};
    use std::io::Write as _;

    const TEXT_OFF_UTF16: usize = 0x200;
    const TEXT_OFF_COMPRESSED: usize = 0x300;

    fn attachment(bytes: Vec<u8>) -> Attachment {
        Attachment::new(
            Some(FileName("test.doc".to_string())),
            None,
            AttachmentContents::new(bytes),
        )
    }

    /// Builds a CLX blob with a single piece covering `char_count` characters.
    fn build_clx(fc: u32, char_count: u32) -> Vec<u8> {
        let mut plc = Vec::new();
        plc.extend_from_slice(&0u32.to_le_bytes());
        plc.extend_from_slice(&char_count.to_le_bytes());
        plc.extend_from_slice(&0u16.to_le_bytes()); // pcd flags
        plc.extend_from_slice(&fc.to_le_bytes()); // fc
        plc.extend_from_slice(&0u16.to_le_bytes()); // prm

        let mut clx = vec![0x02u8];
        clx.extend_from_slice(&(plc.len() as u32).to_le_bytes());
        clx.extend_from_slice(&plc);
        clx
    }

    /// Assembles a minimal but structurally valid Word 97 compound file.
    fn build_doc(flags: u16, ccp_text: usize, text_offset: usize, text: &[u8], clx: &[u8]) -> Vec<u8> {
        let mut wd = vec![0u8; 0x400];
        wd[OFF_W_IDENT..OFF_W_IDENT + 2].copy_from_slice(&W_IDENT.to_le_bytes());
        wd[OFF_FIB_FLAGS..OFF_FIB_FLAGS + 2].copy_from_slice(&flags.to_le_bytes());
        wd[OFF_CCP_TEXT..OFF_CCP_TEXT + 4].copy_from_slice(&(ccp_text as u32).to_le_bytes());
        wd[OFF_FC_CLX..OFF_FC_CLX + 4].copy_from_slice(&0u32.to_le_bytes());
        wd[OFF_LCB_CLX..OFF_LCB_CLX + 4].copy_from_slice(&(clx.len() as u32).to_le_bytes());
        wd[text_offset..text_offset + text.len()].copy_from_slice(text);

        let mut cfb = CompoundFile::create(std::io::Cursor::new(Vec::new())).unwrap();
        for (name, data) in [("/WordDocument", &wd[..]), ("/1Table", clx)] {
            let mut stream = cfb.create_new_stream(name).unwrap();
            stream.write_all(data).unwrap();
        }
        cfb.into_inner().into_inner()
    }

    #[test]
    fn extracts_utf16_cyrillic_text() {
        let text = "Первый\rВторой";
        let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let bytes = build_doc(
            FLAG_WHICH_TBL_STM,
            text.chars().count(),
            TEXT_OFF_UTF16,
            &utf16,
            &build_clx(TEXT_OFF_UTF16 as u32, text.chars().count() as u32),
        );

        let document = LegacyDocExtractor.extract(&attachment(bytes)).unwrap();
        assert_eq!(
            document.into_paragraphs(),
            vec![Paragraph("Первый".into()), Paragraph("Второй".into())]
        );
    }

    #[test]
    fn extracts_compressed_ascii_text() {
        let text = b"Hello\rWorld";
        let fc = FC_COMPRESSED | ((TEXT_OFF_COMPRESSED as u32) * 2);
        let bytes = build_doc(
            FLAG_WHICH_TBL_STM,
            text.len(),
            TEXT_OFF_COMPRESSED,
            text,
            &build_clx(fc, text.len() as u32),
        );

        let document = LegacyDocExtractor.extract(&attachment(bytes)).unwrap();
        assert_eq!(
            document.into_paragraphs(),
            vec![Paragraph("Hello".into()), Paragraph("World".into())]
        );
    }

    #[test]
    fn table_cell_marks_split_paragraphs() {
        let text = b"cell1\x07cell2\x07\rnext";
        let fc = FC_COMPRESSED | ((TEXT_OFF_COMPRESSED as u32) * 2);
        let bytes = build_doc(
            FLAG_WHICH_TBL_STM,
            text.len(),
            TEXT_OFF_COMPRESSED,
            text,
            &build_clx(fc, text.len() as u32),
        );

        let document = LegacyDocExtractor.extract(&attachment(bytes)).unwrap();
        assert_eq!(
            document.into_paragraphs(),
            vec![
                Paragraph("cell1".into()),
                Paragraph("cell2".into()),
                Paragraph("next".into())
            ]
        );
    }

    #[test]
    fn encrypted_document_is_reported() {
        let bytes = build_doc(FLAG_WHICH_TBL_STM | FLAG_ENCRYPTED, 0, 0, b"", &[]);
        assert!(matches!(
            LegacyDocExtractor.extract(&attachment(bytes)),
            Err(DocumentError::Encrypted)
        ));
    }

    #[test]
    fn invalid_signature_is_corrupt() {
        let mut wd = vec![0u8; 0x400];
        wd[OFF_W_IDENT..OFF_W_IDENT + 2].copy_from_slice(&0x1234u16.to_le_bytes());
        let mut cfb = CompoundFile::create(std::io::Cursor::new(Vec::new())).unwrap();
        let mut stream = cfb.create_new_stream("/WordDocument").unwrap();
        stream.write_all(&wd).unwrap();
        let bytes = cfb.into_inner().into_inner();

        assert!(matches!(
            LegacyDocExtractor.extract(&attachment(bytes)),
            Err(DocumentError::Corrupt(_))
        ));
    }

    #[test]
    fn non_compound_file_input_is_corrupt() {
        assert!(matches!(
            LegacyDocExtractor.extract(&attachment(b"garbage".to_vec())),
            Err(DocumentError::Corrupt(_))
        ));
    }
}
