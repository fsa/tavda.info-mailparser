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
    assert_eq!(json["body"], "Plain body without attachments.\r\n");
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
    assert_eq!(json["to"], "recipient@example.com");
    assert_eq!(json["date"], "2026-08-21T16:30:00Z");
    assert_eq!(json["body"], "См. вложение.");

    let documents = json["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0]["status"], "ok");
    // RFC 2047-encoded attachment name is decoded
    assert_eq!(documents[0]["filename"], "письмо.docx");
    assert_eq!(
        documents[0]["content_type"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    assert_eq!(
        documents[0]["detected_content_type"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
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
    // fixture has no To header
    assert!(json["to"].is_null());
    assert_eq!(json["body"], "Three attachments.");
    assert_eq!(documents.len(), 3);
    assert_eq!(documents[0]["status"], "ok");
    assert_eq!(
        documents[0]["content_type"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    assert_eq!(
        documents[0]["detected_content_type"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    assert_eq!(
        documents[0]["paragraphs"],
        serde_json::json!(["Первый абзац", "Второй абзац"])
    );
    assert_eq!(documents[1]["status"], "unsupported");
    assert_eq!(documents[1]["filename"], "notes.txt");
    assert_eq!(documents[1]["content_type"], "text/plain");
    assert_eq!(documents[2]["status"], "unsupported");
    assert_eq!(documents[2]["filename"], "unknown.xyz");
    assert_eq!(documents[2]["content_type"], "application/octet-stream");
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
    assert_eq!(doc["content_type"], "application/msword");
    assert_eq!(doc["detected_content_type"], "application/msword");
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
fn default_output_is_compact_single_line_json() {
    let output = Command::new(BINARY)
        .arg(data_file("simple.eml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.lines().count(), 1, "compact JSON must be one line");
    assert!(!stdout.contains(": "), "no padding after colons: {stdout}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["message_id"], "<simple-001@example.com>");
}

#[test]
fn pretty_flag_indents_json() {
    let output = Command::new(BINARY)
        .arg("--pretty")
        .arg(data_file("simple.eml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.lines().count() > 1, "pretty JSON must span lines");
    assert!(stdout.contains(": "));
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["message_id"], "<simple-001@example.com>");
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

fn temp_log_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    // the PID makes the directory unique across test invocations, so a dir
    // left behind by an aborted run is never accidentally reused
    std::env::temp_dir().join(format!("tavda-log-{tag}-{}-{seq}", std::process::id()))
}

fn dir_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    files
}

fn find_ext<'a>(files: &'a [PathBuf], ext: &str) -> &'a PathBuf {
    files
        .iter()
        .find(|path| path.extension().is_some_and(|e| e == ext))
        .expect("file with extension `{ext}`")
}

#[test]
fn log_dir_names_files_after_message_id_without_log_errors() {
    let dir = temp_log_dir("named");
    let output = Command::new(BINARY)
        .args(["--log-dir", dir.to_str().unwrap()])
        .arg(data_file("simple.eml"))
        .output()
        .unwrap();
    assert!(output.status.success());

    let files = dir_files(&dir);
    assert_eq!(files.len(), 1, "no --log-errors: only the .json result file exists");
    let name = files[0].file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("simple-001_example.com-"), "name: {name}");
    assert!(name.ends_with(".json"));
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
    assert_eq!(json["message_id"], "<simple-001@example.com>");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn log_errors_tees_diagnostics_to_stderr_and_log_file() {
    let dir = temp_log_dir("diagnostics");
    let output = Command::new(BINARY)
        .args(["--log-dir", dir.to_str().unwrap(), "--log-errors"])
        .arg(data_file("multi_attachments.eml"))
        .output()
        .unwrap();
    assert!(output.status.success());

    // diagnostics are still on the terminal ...
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported format"), "stderr: {stderr}");

    // ... and mirrored into the .log file
    let files = dir_files(&dir);
    assert_eq!(files.len(), 2);
    let result = find_ext(&files, "json");
    let log = find_ext(&files, "log");
    let prefix = "multi-003_example.com-";
    assert!(result.file_name().unwrap().to_str().unwrap().starts_with(prefix));
    assert!(log.file_name().unwrap().to_str().unwrap().starts_with(prefix));
    let text = std::fs::read_to_string(log).unwrap();
    assert!(text.contains("unsupported format"), "log: {text}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn log_errors_with_quiet_keeps_diagnostics_only_in_log_file() {
    let dir = temp_log_dir("quiet-log");
    let output = Command::new(BINARY)
        .args(["--log-dir", dir.to_str().unwrap(), "--log-errors", "--quiet"])
        .arg(data_file("multi_attachments.eml"))
        .output()
        .unwrap();
    assert!(output.status.success());

    // stderr is suppressed ...
    assert!(output.stderr.is_empty(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    // ... but the .log file still gets the diagnostics
    let files = dir_files(&dir);
    assert_eq!(files.len(), 2);
    let log = find_ext(&files, "log");
    let text = std::fs::read_to_string(log).unwrap();
    assert!(text.contains("unsupported format"), "log: {text}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn log_dir_without_log_errors_leaves_diagnostics_on_stderr_and_no_log_file() {
    let dir = temp_log_dir("no-log-errors");
    let output = Command::new(BINARY)
        .args(["--log-dir", dir.to_str().unwrap()])
        .arg(data_file("multi_attachments.eml"))
        .output()
        .unwrap();
    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported format"), "stderr: {stderr}");

    let files = dir_files(&dir);
    assert_eq!(files.len(), 1, "no --log-errors: no .log file is created");
    assert!(files[0].extension().is_some_and(|e| e == "json"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn quiet_suppresses_diagnostics_entirely() {
    let output = run_with_stdin(b"", &["--quiet"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn log_dir_records_failed_parse_in_error_log_with_fallback_name() {
    let dir = temp_log_dir("failed");
    let output = run_with_stdin(b"", &["--log-dir", dir.to_str().unwrap(), "--log-errors"]);
    assert!(!output.status.success());

    let files = dir_files(&dir);
    assert_eq!(files.len(), 2);
    let result = find_ext(&files, "json");
    let log = find_ext(&files, "log");
    // no Message-ID available: fallback name is a YYYYMMDD-HHMMSS timestamp
    for path in [result, log] {
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.len() > 20, "name: {name}");
        assert!(name.as_bytes()[..8].iter().all(|b| b.is_ascii_digit()), "name: {name}");
    }
    assert_eq!(std::fs::metadata(result).unwrap().len(), 0, "no JSON was produced");
    let text = std::fs::read_to_string(log).unwrap();
    assert!(text.contains("not a parseable"), "log: {text}");
    // the same error is mirrored to stderr
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a parseable"), "stderr: {stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}
