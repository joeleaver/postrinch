//! Gmail's snippets are HTML-escaped text ("I&#39;ll be there"): decode the
//! character references so a snippet is plain text like everything else in
//! a message.

use serde::{Deserialize, Deserializer};

/// For `#[serde(deserialize_with)]` on a snippet field.
pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(unescape(&String::deserialize(d)?))
}

/// Decode HTML character references: the named ones Gmail uses and every
/// numeric one. Anything else that starts with `&` is left as it is.
pub(crate) fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest
            .find(';')
            .filter(|&semi| semi <= 10)
            .and_then(|semi| reference(&rest[1..semi]).map(|c| (c, semi)));
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn reference(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::unescape;

    #[test]
    fn decodes_what_gmail_sends() {
        assert_eq!(unescape("I&#39;ve sent &quot;it&quot;"), "I've sent \"it\"");
        assert_eq!(unescape("AT&amp;T &lt;3 &#x2014; ok"), "AT&T <3 — ok");
    }

    #[test]
    fn leaves_what_is_not_a_reference() {
        assert_eq!(unescape("fish & chips"), "fish & chips");
        assert_eq!(unescape("a &bogus; b &#zz; c &"), "a &bogus; b &#zz; c &");
        assert_eq!(unescape("&amp;amp;"), "&amp;");
    }
}
