//! Corpus test: runs the CLI against every message file in `tests/data/`.
//!
//! Drop any `.eml` / `.msg` files into `tests/data/` — they are picked up
//! automatically on the next `cargo test` run. Every file must be a parseable
//! RFC 822 message: the test asserts exit code 0, a single valid JSON document
//! on stdout, and conformance to the documented output schema.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_tavda-mail-parser");
const DATA_DIR: &str = "tests/data";

fn corpus_files() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(DATA_DIR);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("{DATA_DIR} directory must exist: {err}"))
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

#[test]
fn every_corpus_file_parses_into_valid_json() {
    let files = corpus_files();
    assert!(
        !files.is_empty(),
        "no test messages found in {DATA_DIR}; place .eml/.msg files there"
    );

    for file in &files {
        let output = Command::new(BINARY)
            .arg(file)
            .output()
            .unwrap_or_else(|err| panic!("cannot spawn {BINARY}: {err}"));

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{}: unexpected failure (exit code {:?}):\n{stderr}",
            file.display(),
            output.status.code()
        );

        let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{}: stdout is not a single valid JSON document: {err}\nstderr:\n{stderr}",
                file.display()
            )
        });

        validate_schema(&json, file);
    }
}

fn validate_schema(json: &Value, file: &Path) {
    let obj = json
        .as_object()
        .unwrap_or_else(|| panic!("{}: top level must be a JSON object", file.display()));

    for key in ["message_id", "subject", "sender", "date", "documents"] {
        assert!(obj.contains_key(key), "{}: missing key `{key}`", file.display());
    }

    check_optional_string(obj.get("message_id"), "message_id", file);
    check_optional_string(obj.get("subject"), "subject", file);
    check_optional_string(obj.get("sender"), "sender", file);

    if let Some(id) = obj.get("message_id").filter(|id| !id.is_null()) {
        let s = id.as_str().expect("message_id must be a string");
        assert!(
            s.starts_with('<') && s.ends_with('>'),
            "{}: message_id `{s}` must look like `<...>`",
            file.display()
        );
    }

    if let Some(date) = obj.get("date").filter(|date| !date.is_null()) {
        let s = date.as_str().expect("date must be a string");
        assert!(
            is_iso8601_utc(s),
            "{}: date `{s}` is not normalized ISO 8601 UTC",
            file.display()
        );
    }

    let documents = obj["documents"]
        .as_array()
        .unwrap_or_else(|| panic!("{}: `documents` must be an array", file.display()));

    for (index, doc) in documents.iter().enumerate() {
        validate_document(doc, index, file);
    }
}

fn validate_document(doc: &Value, index: usize, file: &Path) {
    let context = format!("{}: documents[{index}]", file.display());
    let doc = doc
        .as_object()
        .unwrap_or_else(|| panic!("{context}: must be an object"));

    if let Some(filename) = doc.get("filename") {
        assert!(
            filename.is_null() || filename.is_string(),
            "{context}: `filename` must be null or a string"
        );
    }

    match doc
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{context}: missing `status`"))
    {
        "ok" => {
            let paragraphs = doc
                .get("paragraphs")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("{context} (ok): must contain `paragraphs` array"));
            for paragraph in paragraphs {
                assert!(
                    paragraph.is_string(),
                    "{context}: paragraphs must contain only strings"
                );
            }
        }
        "unsupported" => {}
        "error" => {
            let error = doc
                .get("error")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("{context} (error): must contain `error` object"));
            assert!(
                error.contains_key("code"),
                "{context} (error): missing `error.code`"
            );
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{context} (error): missing `error.message`"));
            assert!(!message.is_empty(), "{context} (error): empty `error.message`");
        }
        other => panic!("{context}: unknown status `{other}`"),
    }
}

fn check_optional_string(value: Option<&Value>, key: &str, file: &Path) {
    if let Some(value) = value {
        assert!(
            value.is_null() || value.is_string(),
            "{}: `{key}` must be null or a string",
            file.display()
        );
    }
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
