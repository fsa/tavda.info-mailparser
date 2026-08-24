use mail_parser::{Address, Addr, DateTime, MessageParser, MimeHeaders};

use super::{
    Attachment, AttachmentContents, EmailError, EmailMetadata, FileName, IsoDateTime, MessageId,
    MimeType, ParsedEmail, RawMessage, Recipient, Sender, Subject,
};

/// Parses raw message bytes into metadata and attachments.
///
/// MIME decoding (multipart, transfer encodings, RFC 2047/2231 encoded
/// headers and charsets) is delegated to the `mail-parser` crate.
pub fn parse(raw: RawMessage<'_>) -> Result<ParsedEmail, EmailError> {
    let message = MessageParser::default()
        .parse(raw.as_bytes())
        .ok_or(EmailError::Unparsable)?;

    let metadata = EmailMetadata {
        // mail-parser strips the angle brackets; restore them so the value
        // stays a syntactically complete RFC 5322 msg-id.
        message_id: message
            .message_id()
            .map(|id| MessageId(normalize_message_id(id))),
        subject: message.subject().map(|s| Subject(s.to_string())),
        sender: sender(&message),
        to: recipient(&message),
        date: message.date().and_then(normalize_date),
    };

    let attachments = message
        .attachments()
        .map(|part| {
            Attachment::new(
                part.attachment_name().map(|name| FileName(name.to_string())),
                content_type(part.content_type()),
                AttachmentContents::new(part.contents().to_vec()),
            )
        })
        .collect();

    Ok(ParsedEmail::new(metadata, attachments))
}

fn normalize_message_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.starts_with('<') {
        trimmed.to_string()
    } else {
        format!("<{trimmed}>")
    }
}

fn sender(message: &mail_parser::Message<'_>) -> Option<Sender> {
    first_field_address(message.from()).map(Sender)
}

/// Primary recipient: the first address of the `To` header.
fn recipient(message: &mail_parser::Message<'_>) -> Option<Recipient> {
    first_field_address(message.to()).map(Recipient)
}

/// First address of a header field: the bare address if present, otherwise
/// the display name. Group syntax (`Undisclosed recipients:;) is flattened.
fn first_field_address(field: Option<&Address<'_>>) -> Option<String> {
    let addresses = match field? {
        Address::List(list) => list.iter().collect::<Vec<_>>(),
        Address::Group(groups) => groups
            .iter()
            .flat_map(|group| group.addresses.iter())
            .collect(),
    };
    addresses.into_iter().find_map(first_address)
}

fn first_address(addr: &Addr<'_>) -> Option<String> {
    addr.address
        .as_deref()
        .map(str::to_string)
        .or_else(|| addr.name.as_deref().map(str::to_string))
}

fn content_type(ct: Option<&mail_parser::ContentType<'_>>) -> Option<MimeType> {
    ct.map(|ct| match ct.c_subtype.as_deref() {
        Some(subtype) => MimeType(format!("{}/{subtype}", ct.c_type)),
        None => MimeType(ct.c_type.to_string()),
    })
}

/// Converts an RFC 5322 date into UTC ISO 8601 without pulling in a full
/// calendar crate: `to_timestamp()` gives seconds since the epoch and
/// `civil_from_days` converts days back to a Y-M-D date.
fn normalize_date(dt: &DateTime) -> Option<IsoDateTime> {
    let timestamp = dt.to_timestamp();
    let days = timestamp.div_euclid(86_400);
    let secs_of_day = timestamp.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if year < 0 || year > 9999 {
        return None;
    }
    let (hour, minute, second) = (
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60,
    );
    Some(IsoDateTime(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    )))
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::{EmailError, RawMessage};

    fn parse_str(raw: &str) -> Result<ParsedEmail, EmailError> {
        parse(RawMessage::new(raw.as_bytes()))
    }

    #[test]
    fn parses_basic_headers() {
        let parsed = parse_str(
            "Message-ID: <simple@example.com>\r\n\
             From: Alice <alice@example.com>\r\n\
             To: Bob <bob@example.com>\r\n\
             Subject: Hello\r\n\
             Date: Thu, 01 Jan 2026 12:00:00 +0000\r\n\
             \r\n\
             body",
        )
        .unwrap();
        let metadata = &parsed.metadata;
        assert_eq!(metadata.message_id.as_ref().unwrap().0, "<simple@example.com>");
        assert_eq!(metadata.subject.as_ref().unwrap().0, "Hello");
        assert_eq!(metadata.sender.as_ref().unwrap().0, "alice@example.com");
        assert_eq!(metadata.to.as_ref().unwrap().0, "bob@example.com");
        assert_eq!(metadata.date.as_ref().unwrap().0, "2026-01-01T12:00:00Z");
    }

    #[test]
    fn picks_first_recipient_of_multi_address_to_header() {
        let parsed = parse_str("To: bob@example.com, Carol <carol@example.com>\r\n\r\n").unwrap();
        assert_eq!(parsed.metadata.to.as_ref().unwrap().0, "bob@example.com");
    }

    #[test]
    fn normalizes_date_to_utc() {
        let parsed = parse_str("Date: Fri, 21 Aug 2026 19:30:00 +0300\r\n\r\n").unwrap();
        assert_eq!(parsed.metadata.date.as_ref().unwrap().0, "2026-08-21T16:30:00Z");
    }

    #[test]
    fn keeps_epoch_boundary_date_correct() {
        let parsed = parse_str("Date: Wed, 31 Dec 1969 23:59:59 -0100\r\n\r\n").unwrap();
        assert_eq!(parsed.metadata.date.as_ref().unwrap().0, "1970-01-01T00:59:59Z");
    }

    #[test]
    fn decodes_rfc2047_utf8_subject() {
        let parsed =
            parse_str("Subject: =?utf-8?B?0J4g0L3QsNC/0YDQsNCy0LvQtdC90LjQuCDQtNC+0LrRg9C80LXQvdGC0L7Qsg==?=\r\n\r\n")
                .unwrap();
        assert_eq!(
            parsed.metadata.subject.as_ref().unwrap().0,
            "О направлении документов"
        );
    }

    #[test]
    fn decodes_rfc2047_koi8_r_subject() {
        let parsed = parse_str("Subject: =?koi8-r?Q?=D0=D2=C9=D7=C5=D4?=\r\n\r\n").unwrap();
        assert_eq!(parsed.metadata.subject.as_ref().unwrap().0, "привет");
    }

    #[test]
    fn wraps_bare_message_id_in_angle_brackets() {
        let parsed = parse_str("Message-ID: bare-id@example.com\r\n\r\n").unwrap();
        assert_eq!(parsed.metadata.message_id.as_ref().unwrap().0, "<bare-id@example.com>");
    }

    #[test]
    fn missing_headers_are_none() {
        let parsed = parse_str("Subject: only subject\r\n\r\n").unwrap();
        assert!(parsed.metadata.message_id.is_none());
        assert!(parsed.metadata.sender.is_none());
        assert!(parsed.metadata.to.is_none());
        assert!(parsed.metadata.date.is_none());
    }

    #[test]
    fn plain_message_has_no_attachments() {
        let parsed = parse_str("Subject: plain\r\n\r\njust text\r\n").unwrap();
        assert!(parsed.attachments().is_empty());
    }

    #[test]
    fn extracts_multiple_attachments() {
        let raw = "\
Message-ID: <multi@example.com>\r\n\
Subject: attachments\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"B\"\r\n\
\r\n\
--B\r\n\
Content-Type: text/plain\r\n\
\r\n\
body text\r\n\
--B\r\n\
Content-Type: application/octet-stream; name=\"a.docx\"\r\n\
Content-Disposition: attachment; filename=\"a.docx\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
AAECAwQF\r\n\
--B\r\n\
Content-Disposition: attachment; filename=\"b.txt\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
aGVsbG8=\r\n\
--B--\r\n";
        let parsed = parse_str(raw).unwrap();
        let attachments = parsed.attachments();

        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].filename().unwrap().as_str(), "a.docx");
        assert_eq!(
            attachments[0].content_type().unwrap().as_str(),
            "application/octet-stream"
        );
        assert_eq!(attachments[0].contents().as_bytes(), &[0u8, 1, 2, 3, 4, 5]);
        assert_eq!(attachments[1].filename().unwrap().as_str(), "b.txt");
        assert_eq!(attachments[1].contents().as_bytes(), b"hello");
    }

    #[test]
    fn attachment_without_filename_has_none() {
        let raw = "\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"B\"\r\n\
\r\n\
--B\r\n\
Content-Disposition: attachment\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
AA==\r\n\
--B--\r\n";
        let parsed = parse_str(raw).unwrap();
        let attachments = parsed.attachments();
        assert_eq!(attachments.len(), 1);
        assert!(attachments[0].filename().is_none());
        assert_eq!(attachments[0].contents().as_bytes(), &[0u8]);
    }

    #[test]
    fn empty_input_is_unparsable() {
        assert!(matches!(
            parse(RawMessage::new(b"")),
            Err(EmailError::Unparsable)
        ));
    }
}
