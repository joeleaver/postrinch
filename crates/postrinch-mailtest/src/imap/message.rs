//! Reading the parts of a raw RFC 5322 message the server needs: its
//! header and text, and individual header fields.

/// Splits a message into its header (through the blank line that ends
/// it) and its text. A message with no blank line is all header.
pub(crate) fn split_message(body: &[u8]) -> (&[u8], &[u8]) {
    let crlf = find(body, b"\r\n\r\n").map(|i| i + 4);
    let lf = find(body, b"\n\n").map(|i| i + 2);
    let end = match (crlf, lf) {
        (Some(a), Some(b)) => a.min(b),
        (a, b) => a.or(b).unwrap_or(body.len()),
    };
    body.split_at(end)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The header's fields, each as `(name, raw lines with their line
/// endings)`; continuation lines belong to the field before them.
fn fields(header: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut fields: Vec<(String, Vec<u8>)> = Vec::new();
    for line in header.split_inclusive(|&b| b == b'\n') {
        let blank = line.iter().all(|b| b.is_ascii_whitespace());
        if blank {
            continue;
        }
        if line[0] == b' ' || line[0] == b'\t' {
            if let Some(last) = fields.last_mut() {
                last.1.extend_from_slice(line);
            }
            continue;
        }
        let name = match line.iter().position(|&b| b == b':') {
            Some(colon) => String::from_utf8_lossy(&line[..colon]).trim().to_string(),
            None => continue,
        };
        fields.push((name, line.to_vec()));
    }
    fields
}

/// `BODY[HEADER.FIELDS (...)]` and `HEADER.FIELDS.NOT`: the chosen fields
/// as they appear, followed by the blank line.
pub(crate) fn header_fields(header: &[u8], names: &[String], include: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, raw) in fields(header) {
        let listed = names.iter().any(|n| n.eq_ignore_ascii_case(&name));
        if listed == include {
            out.extend_from_slice(&raw);
        }
    }
    out.extend_from_slice(b"\r\n");
    out
}

/// The unfolded values of every field called `name`.
pub(crate) fn header_values(body: &[u8], name: &str) -> Vec<String> {
    let (header, _) = split_message(body);
    fields(header)
        .into_iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, raw)| {
            let text = String::from_utf8_lossy(&raw);
            let value = text.split_once(':').map_or("", |(_, v)| v);
            value
                .split(['\r', '\n'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Case-insensitive (ASCII) substring search, as SEARCH does.
pub(crate) fn contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIL: &[u8] =
        b"From: a@example.com\r\nSubject: Hello\r\n  world\r\nTo: b@example.com\r\n\r\nBody text\r\n";

    #[test]
    fn split() {
        let (h, t) = split_message(MAIL);
        assert!(h.ends_with(b"b@example.com\r\n\r\n"));
        assert_eq!(t, b"Body text\r\n");
        assert_eq!(split_message(b"X: y"), (&b"X: y"[..], &b""[..]));
    }

    #[test]
    fn fields_and_values() {
        let (h, _) = split_message(MAIL);
        assert_eq!(
            header_fields(h, &["SUBJECT".into()], true),
            b"Subject: Hello\r\n  world\r\n\r\n"
        );
        assert_eq!(
            header_fields(h, &["subject".into(), "to".into()], false),
            b"From: a@example.com\r\n\r\n"
        );
        assert_eq!(header_values(MAIL, "subject"), vec!["Hello world"]);
        assert!(contains_ignore_case(MAIL, b"BODY TEXT"));
    }
}
