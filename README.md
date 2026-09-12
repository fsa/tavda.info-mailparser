# tavda-mail-parser

Parses a single email message (`.eml` / `.msg`, from stdin or a file) and prints
a single normalized JSON document to stdout: message metadata, decoded body
text, and the extracted plain text of every supported attachment.

Docx and legacy `.doc` are decoded in-process. PDF text extraction is delegated
to an external decoder (see Dependency below).

## System dependencies

* `poppler-utils` — required to extract text from PDF attachments.

```sh
sudo apt-get install -y poppler-utils   # Debian 13 / trixie
```

The binary does not bundle a PDF decoder. When `pdftotext` is not installed,
PDF attachments are still recognized but reported with
`"status": "missing_decoder"` (and `"decoder": "pdftotext"`) instead of an
unsupported format — so consumers can distinguish "we do not know this format"
from "the host lacks the decoder".

## Build

```sh
cargo build --release
```

`cargo test` runs the test suite. The PDF integration test degrades gracefully:
when `pdftotext` is absent it verifies the `missing_decoder` signal and skips
the content assertions.

## Usage

```sh
tavda-mail-parser message.eml          # compact JSON on stdout
tavda-mail-parser < message.eml        # same, via stdin
tavda-mail-parser --pretty message.eml # indented JSON
tavda-mail-parser --log-dir /var/log/mail --log-errors message.eml
```

Diagnostics go to stderr; stdout always contains exactly one JSON document.
`--quiet` silences stderr diagnostics.

## Output contract

The exact shape of the JSON document — all fields, statuses (`ok`,
`unsupported`, `missing_decoder`, `error`) and error codes — is described in
[`contract/output.schema.json`](contract/output.schema.json) (JSON Schema
2020-12). Other applications should validate against it or use it to generate
typed consumers; the schema is the contract, this document is only a summary.

Per attachment, the `status` field means:

| status             | meaning                                                              |
| ------------------ | -------------------------------------------------------------------- |
| `ok`               | text extracted into `paragraphs[]`                                   |
| `unsupported`      | format not recognized; nothing decoded                               |
| `missing_decoder`  | format recognized, but the host lacks the decoder (`decoder` field)  |
| `error`            | extraction attempted and failed (`error.code` / `error.message`)     |