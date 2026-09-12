//! End-to-end test for PDF attachments.
//!
//! The fixture is a real MChS forecast PDF: it exercises Cyrillic decoding,
//! the custom font encodings (ToUnicode CMaps) and multi-line paragraph
//! grouping. Since text extraction delegates to the external `pdftotext`
//! tool, the test degrades gracefully: when the tool is absent the binary
//! still succeeds and reports `status: "missing_decoder"`, and the content
//! assertions are skipped with a note on stderr.

use serde_json::Value;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BINARY: &str = env!("CARGO_BIN_EXE_tavda-mail-parser");

const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn run_with_stdin(input: &[u8]) -> std::process::Output {
    let mut child = Command::new(BINARY)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot spawn binary");
    child
        .stdin
        .as_mut()
        .expect("stdin must be piped")
        .write_all(input)
        .expect("cannot write to stdin");
    child.wait_with_output().expect("cannot wait for child")
}

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/pdf/Детализированный прогноз на 13.09.2026.pdf")
}

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(BASE64[(n >> 18) as usize & 0x3F] as char);
        out.push(BASE64[(n >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 {
            BASE64[(n >> 6) as usize & 0x3F] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64[n as usize & 0x3F] as char
        } else {
            '='
        });
    }
    let mut wrapped = String::new();
    for (index, ch) in out.char_indices() {
        if index > 0 && index % 76 == 0 {
            wrapped.push_str("\r\n");
        }
        wrapped.push(ch);
    }
    wrapped
}

/// A minimal MIME message carrying the PDF fixture as a base64 attachment.
fn build_pdf_email(pdf: &[u8]) -> Vec<u8> {
    format!(
        "From: sender@example.com\r\n\
         To: recipient@example.com\r\n\
         Subject: Детализированный прогноз\r\n\
         Date: Sat, 12 Sep 2026 10:00:00 +0000\r\n\
         Message-ID: <pdf-006@example.com>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"XYZ\"\r\n\
         \r\n\
         --XYZ\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         См. вложение.\r\n\
         --XYZ\r\n\
         Content-Type: application/pdf; name=\"forecast.pdf\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         Content-Disposition: attachment; filename=\"forecast.pdf\"\r\n\
         \r\n\
         {body}\r\n\
         --XYZ--\r\n",
        body = base64_encode(pdf),
    )
    .into_bytes()
}

#[test]
fn pdf_attachment_reports_decoder_and_extracts_text() {
    let pdf = std::fs::read(fixture_path()).expect("PDF fixture must exist");
    let output = run_with_stdin(&build_pdf_email(&pdf));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let documents = json["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 1);

    let doc = &documents[0];
    assert_eq!(doc["filename"], "forecast.pdf");
    assert_eq!(doc["content_type"], "application/pdf");
    assert_eq!(doc["detected_content_type"], "application/pdf");

    match doc["status"].as_str() {
        Some("ok") => {
            let paragraphs: Vec<&str> = doc["paragraphs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap())
                .collect();
            let joined = paragraphs.join("\n");
            assert!(joined.contains("Свердловской области"), "text: {joined}");
            assert!(joined.contains("13 сентября 2026"), "text: {joined}");
            assert!(joined.contains("сильные дожди"), "text: {joined}");
        }
        Some("missing_decoder") => {
            // poppler-utils отсутствует на хосте: формат распознан, но
            // декодер не установлен. Это валидный, явный исход работы CLI —
            // проверяем сигнал и пропускаем проверку содержимого.
            assert_eq!(doc["decoder"], "pdftotext");
            eprintln!(
                "note: decoder `{}` is not installed; PDF content assertions skipped",
                doc["decoder"]
            );
        }
        other => panic!("unexpected document status: {other:?}"),
    }
}

#[test]
fn pdf_without_decoder_is_reported_not_unsupported() {
    // A PDF with no usable decoder on PATH must not be reported as an unknown
    // format: the format is recognized, only the decoder is missing. An empty
    // PATH guarantees `pdftotext` cannot be found on any host.
    let pdf = std::fs::read(fixture_path()).unwrap();
    let mut child = Command::new(BINARY)
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot spawn binary");
    child
        .stdin
        .as_mut()
        .expect("stdin must be piped")
        .write_all(&build_pdf_email(&pdf))
        .expect("cannot write to stdin");
    let output = child.wait_with_output().expect("cannot wait for child");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let doc = &json["documents"][0];
    assert_eq!(doc["status"], "missing_decoder");
    assert_eq!(doc["decoder"], "pdftotext");
    assert_eq!(doc["filename"], "forecast.pdf");
    assert_eq!(doc["content_type"], "application/pdf");
    assert_eq!(doc["detected_content_type"], "application/pdf");
}
