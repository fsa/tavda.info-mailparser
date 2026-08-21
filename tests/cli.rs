//! End-to-end CLI tests on the bundled fixtures in `tests/data/`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BINARY: &str = env!("CARGO_BIN_EXE_tavda-mail-parser");

fn data_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn run_with_stdin(input: &[u8], args: &[&str]) -> std::process::Output {
    let mut child = Command::new(BINARY)
        .args(args)
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

#[test]
fn reads_message_from_stdin() {
    let input = std::fs::read(data_file("simple.eml")).unwrap();
    let output = run_with_stdin(&input, &[]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["message_id"], "<simple-001@example.com>");
    assert_eq!(json["subject"], "Test message");
    assert_eq!(json["sender"], "alice@example.com");
    assert_eq!(json["date"], "2026-01-01T12:00:00Z");
    assert_eq!(json["documents"].as_array().unwrap().len(), 0);
}

#[test]
fn dash_argument_reads_stdin() {
    let input = std::fs::read(data_file("simple.eml")).unwrap();
    let output = run_with_stdin(&input, &["-"]);
    assert!(output.status.success());
    assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_ok());
}

#[test]
fn decodes_utf8_headers_and_docx_from_file_argument() {
    let output = Command::new(BINARY)
        .arg(data_file("utf8_headers.eml"))
        .output()
        .unwrap();
    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["message_id"], "<utf8-002@example.com>");
    assert_eq!(json["subject"], "О направлении документов");
    assert_eq!(json["sender"], "sender@example.com");
    assert_eq!(json["date"], "2026-08-21T16:30:00Z");

    let documents = json["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0]["status"], "ok");
    // RFC 2047-encoded attachment name is decoded
    assert_eq!(documents[0]["filename"], "письмо.docx");
    assert_eq!(
        documents[0]["paragraphs"],
        serde_json::json!([
            "Уважаемые коллеги!",
            "Направляем вам документы.",
            "С уважением,"
        ])
    );
}

#[test]
fn multi_attachment_message_reports_each_document() {
    let output = Command::new(BINARY)
        .arg(data_file("multi_attachments.eml"))
        .output()
        .unwrap();
    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let documents = json["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 3);
    assert_eq!(documents[0]["status"], "ok");
    assert_eq!(
        documents[0]["paragraphs"],
        serde_json::json!(["Первый абзац", "Второй абзац"])
    );
    assert_eq!(documents[1]["status"], "unsupported");
    assert_eq!(documents[1]["filename"], "notes.txt");
    assert_eq!(documents[2]["status"], "unsupported");
    assert_eq!(documents[2]["filename"], "unknown.xyz");
}

#[test]
fn broken_document_does_not_fail_the_whole_message() {
    let output = Command::new(BINARY)
        .arg(data_file("broken_doc.eml"))
        .output()
        .unwrap();
    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let doc = &json["documents"][0];
    assert_eq!(doc["status"], "error");
    assert_eq!(doc["filename"], "broken.doc");
    assert_eq!(doc["error"]["code"], "corrupt_document");
}

#[test]
fn msg_extension_is_parsed_like_eml() {
    let output = Command::new(BINARY)
        .arg(data_file("sample.msg"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["message_id"], "<msg-005@example.com>");
}

#[test]
fn version_flag_prints_name_and_version() {
    let output = Command::new(BINARY).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("tavda-mail-parser {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_flag_prints_usage() {
    let output = Command::new(BINARY).arg("--help").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Usage:"));
    assert!(text.contains("FILE"));
}

#[test]
fn missing_file_fails_with_stderr_diagnostic_and_clean_stdout() {
    let output = Command::new(BINARY)
        .arg("/nonexistent/message.eml")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "stdout must stay empty on fatal errors");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot open input file"), "stderr: {stderr}");
}

#[test]
fn unparsable_input_fails_with_stderr_diagnostic_and_clean_stdout() {
    let output = run_with_stdin(b"", &[]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a parseable"), "stderr: {stderr}");
}
