//! Corpus test: runs the CLI against every message file in a corpus directory
//! and reports aggregate statistics.
//!
//! The corpus directory is resolved as:
//!   1. `$CORPUS_DIR` — a directory with your own `.eml`/`.msg` files;
//!   2. the bundled `tests/data/` fixtures otherwise.
//!
//! Every file must produce exit code 0 and a single schema-conformant JSON
//! document on stdout. Messages whose attachments yield no text (document
//! status `unsupported`, or per-document `error`) count as `Unsupported`:
//! that is valid modeled output of the CLI, not a failure. All files are
//! processed even after failures; the test fails at the end if any file did
//! not pass validation, and every failing file is listed with its full path.
//!
//! Statistics are written directly to stderr on every run — no `--nocapture`
//! needed.

use serde_json::Value;
use std::fs::File;
use std::io::Write as _;
use std::mem::ManuallyDrop;
use std::os::unix::io::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_tavda-mail-parser");
const DATA_DIR: &str = "tests/data";

fn corpus_dir() -> PathBuf {
    let dir = match std::env::var("CORPUS_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join(DATA_DIR),
    };
    dir.canonicalize().unwrap_or(dir)
}

fn corpus_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("cannot read corpus directory {}: {err}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => !name.starts_with('.') && !name.ends_with(".md"),
            None => false,
        })
        .collect();
    files.sort();
    files
}

fn display_name(file: &Path) -> String {
    file.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.display().to_string())
}

#[test]
fn every_corpus_file_parses_into_valid_json() {
    let dir = corpus_dir();
    let files = corpus_files(&dir);
    assert!(
        !files.is_empty(),
        "no test messages found in {}; place .eml/.msg files there",
        dir.display()
    );

    let mut processed = 0usize;
    let mut successful = 0usize;
    let mut unsupported = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for file in &files {
        processed += 1;
        match parse_message(file) {
            Ok(json) => {
                if has_documents_without_text(&json) {
                    unsupported += 1;
                } else {
                    successful += 1;
                }
            }
            Err(reason) => failures.push(format!("  - {}: {reason}", file.display())),
        }
    }

    print_line("Real mail test statistics:");
    print_stat("Files found:", files.len());
    print_stat("Processed:", processed);
    print_stat("Successful:", successful);
    print_stat("Failed:", failures.len());
    print_stat("Unsupported:", unsupported);

    if !failures.is_empty() {
        print_line("Failed files:");
        for line in &failures {
            print_line(line);
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{} corpus files failed validation",
        failures.len(),
        processed
    );
}

fn parse_message(file: &Path) -> Result<Value, String> {
    let output = Command::new(BINARY)
        .arg(file)
        .output()
        .map_err(|err| format!("cannot spawn {BINARY}: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "unexpected failure (exit code {:?}): {}",
            output.status.code(),
            stderr.trim()
        ));
    }

    let json: Value = serde_json::from_slice(&output.stdout).map_err(|err| {
        format!(
            "stdout is not a single valid JSON document: {err}; stderr: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })?;

    validate_schema(&json, &display_name(file))?;
    Ok(json)
}

/// A message counts as `Unsupported` when at least one attachment did not
/// yield text (`status: "unsupported"`) or failed extraction
/// (`status: "error"`). Both are valid, gracefully reported outcomes of the
/// CLI rather than processing failures.
fn has_documents_without_text(json: &Value) -> bool {
    json["documents"].as_array().is_some_and(|documents| {
        documents
            .iter()
            .any(|doc| doc.get("status").and_then(Value::as_str) != Some("ok"))
    })
}

/// The libtest harness captures both stdout and stderr of a passing test and
/// discards them unless `--nocapture` is passed. Writing straight to file
/// descriptor 2 bypasses that capture, so the report is always visible.
fn print_line(text: &str) {
    // SAFETY: fd 2 (stderr) stays open for the whole process lifetime, and the
    // `ManuallyDrop` wrapper makes sure this handle never closes it.
    let mut stderr = ManuallyDrop::new(unsafe { File::from_raw_fd(2) });
    let _ = writeln!(stderr, "{text}");
}

fn print_stat(label: &str, value: usize) {
    print_line(&format!("  {label:<16}{value:>4}"));
}

fn validate_schema(json: &Value, name: &str) -> Result<(), String> {
    let obj = json
        .as_object()
        .ok_or_else(|| format!("{name}: top level must be a JSON object"))?;

    for key in ["message_id", "subject", "sender", "date", "documents"] {
        if !obj.contains_key(key) {
            return Err(format!("{name}: missing key `{key}`"));
        }
    }

    check_optional_string(obj.get("message_id"), "message_id", name)?;
    check_optional_string(obj.get("subject"), "subject", name)?;
    check_optional_string(obj.get("sender"), "sender", name)?;

    if let Some(id) = obj.get("message_id").filter(|id| !id.is_null()) {
        let s = id
            .as_str()
            .ok_or_else(|| format!("{name}: message_id must be a string"))?;
        if !(s.starts_with('<') && s.ends_with('>')) {
            return Err(format!("{name}: message_id `{s}` must look like `<...>`"));
        }
    }

    if let Some(date) = obj.get("date").filter(|date| !date.is_null()) {
        let s = date
            .as_str()
            .ok_or_else(|| format!("{name}: date must be a string"))?;
        if !is_iso8601_utc(s) {
            return Err(format!("{name}: date `{s}` is not normalized ISO 8601 UTC"));
        }
    }

    let documents = obj["documents"]
        .as_array()
        .ok_or_else(|| format!("{name}: `documents` must be an array"))?;

    for (index, doc) in documents.iter().enumerate() {
        validate_document(doc, index, name)?;
    }

    Ok(())
}

fn validate_document(doc: &Value, index: usize, name: &str) -> Result<(), String> {
    let context = format!("{name}: documents[{index}]");
    let doc = doc
        .as_object()
        .ok_or_else(|| format!("{context}: must be an object"))?;

    if let Some(filename) = doc.get("filename") {
        if !(filename.is_null() || filename.is_string()) {
            return Err(format!("{context}: `filename` must be null or a string"));
        }
    }

    let status = doc
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context}: missing `status`"))?;

    match status {
        "ok" => {
            let paragraphs = doc
                .get("paragraphs")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("{context} (ok): must contain `paragraphs` array"))?;
            for paragraph in paragraphs {
                if !paragraph.is_string() {
                    return Err(format!("{context}: paragraphs must contain only strings"));
                }
            }
        }
        "unsupported" => {}
        "error" => {
            let error = doc
                .get("error")
                .and_then(Value::as_object)
                .ok_or_else(|| format!("{context} (error): must contain `error` object"))?;
            if !error.contains_key("code") {
                return Err(format!("{context} (error): missing `error.code`"));
            }
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{context} (error): missing `error.message`"))?;
            if message.is_empty() {
                return Err(format!("{context} (error): empty `error.message`"));
            }
        }
        other => return Err(format!("{context}: unknown status `{other}`")),
    }

    Ok(())
}

fn check_optional_string(value: Option<&Value>, key: &str, name: &str) -> Result<(), String> {
    if let Some(value) = value {
        if !(value.is_null() || value.is_string()) {
            return Err(format!("{name}: `{key}` must be null or a string"));
        }
    }
    Ok(())
}

/// `YYYY-MM-DDTHH:MM:SSZ`
fn is_iso8601_utc(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'Z'
        && b.iter().enumerate().all(|(i, &c)| {
            [4usize, 7, 10, 13, 16, 19].contains(&i) || c.is_ascii_digit()
        })
}
