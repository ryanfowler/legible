//! Character-encoding detection for HTML bytes.
//!
//! The order follows the HTML encoding sniffing algorithm in a simplified
//! form: a byte order mark, then the transport `Content-Type` charset, then
//! valid UTF-8, then a `meta` charset in the start of the document, then
//! Windows-1252.

use std::borrow::Cow;

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};

/// Decodes HTML bytes to UTF-8 text.
///
/// `content_type` is the complete transport `Content-Type` value, such as
/// `text/html; charset=shift_jis`. Valid UTF-8 input without a transport
/// charset is borrowed, not copied. Malformed sequences become U+FFFD.
pub(crate) fn decode_html<'a>(bytes: &'a [u8], content_type: Option<&str>) -> Cow<'a, str> {
    if let Some((encoding, _)) = Encoding::for_bom(bytes) {
        return encoding.decode_with_bom_removal(bytes).0;
    }

    let declared = content_type
        .and_then(content_type_charset)
        .and_then(|label| Encoding::for_label(label.as_bytes()));
    if declared.is_none()
        && let Ok(text) = std::str::from_utf8(bytes)
    {
        return Cow::Borrowed(text);
    }

    let encoding = declared
        .or_else(|| {
            html_charset(bytes)
                .and_then(|label| Encoding::for_label(label.as_bytes()))
                .map(meta_encoding)
        })
        .unwrap_or(WINDOWS_1252);
    encoding.decode_without_bom_handling(bytes).0
}

/// A `meta` element cannot declare UTF-16: the prescan reads ASCII bytes, so
/// the document is not UTF-16. The HTML standard uses UTF-8 instead.
fn meta_encoding(encoding: &'static Encoding) -> &'static Encoding {
    if encoding == UTF_16LE || encoding == UTF_16BE {
        UTF_8
    } else {
        encoding
    }
}

fn content_type_charset(value: &str) -> Option<&str> {
    value.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.trim().split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let value = value.trim().trim_matches(['"', '\'']);
        (!value.is_empty()).then_some(value)
    })
}

/// Finds a `charset` value in the initial part of an HTML document. HTML's
/// encoding sniffing algorithm only needs to inspect the beginning of a page.
/// Restrict the scan to meta elements so text, comments, and scripts cannot
/// change the response encoding.
fn html_charset(bytes: &[u8]) -> Option<String> {
    let sample = &bytes[..bytes.len().min(4096)];
    let mut cursor = 0;
    while cursor + 5 <= sample.len() {
        if sample[cursor..].starts_with(b"<!--") {
            let Some(comment_end) = find_bytes(sample, cursor + 4, b"-->") else {
                break;
            };
            cursor = comment_end + 3;
            continue;
        }
        if starts_with_tag(sample, cursor, b"script") || starts_with_tag(sample, cursor, b"style") {
            let Some(open_end) = find_tag_end(sample, cursor + 1) else {
                break;
            };
            let tag_name = if starts_with_tag(sample, cursor, b"script") {
                b"script".as_slice()
            } else {
                b"style".as_slice()
            };
            let closing = [b"</", tag_name].concat();
            let Some(close_start) = find_bytes(sample, open_end + 1, &closing) else {
                break;
            };
            cursor = close_start + closing.len();
            continue;
        }
        if sample[cursor] != b'<'
            || !sample[cursor + 1..cursor + 5].eq_ignore_ascii_case(b"meta")
            || !sample
                .get(cursor + 5)
                .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
        {
            cursor += 1;
            continue;
        }

        let tag_start = cursor + 5;
        let Some(tag_end) = find_tag_end(sample, tag_start) else {
            break;
        };
        if let Some(charset) = meta_charset(&sample[tag_start..tag_end]) {
            return Some(charset);
        }
        cursor = tag_end + 1;
    }
    None
}

fn starts_with_tag(bytes: &[u8], start: usize, name: &[u8]) -> bool {
    bytes.get(start) == Some(&b'<')
        && bytes
            .get(start + 1..start + 1 + name.len())
            .is_some_and(|tag| tag.eq_ignore_ascii_case(name))
        && bytes
            .get(start + 1 + name.len())
            .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
}

fn find_bytes(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || start > bytes.len() || bytes.len() - start < needle.len() {
        return None;
    }
    (start..=bytes.len() - needle.len())
        .find(|&index| bytes[index..index + needle.len()].eq_ignore_ascii_case(needle))
}

fn find_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, &byte) in bytes[start..].iter().enumerate() {
        match (quote, byte) {
            (Some(delimiter), byte) if byte == delimiter => quote = None,
            (None, b'\'' | b'"') => quote = Some(byte),
            (None, b'>') => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn meta_charset(attributes: &[u8]) -> Option<String> {
    let mut cursor = 0;
    let mut http_equiv = None;
    let mut content = None;
    while cursor < attributes.len() {
        while attributes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'/')
        {
            cursor += 1;
        }
        let name_start = cursor;
        while attributes
            .get(cursor)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'/'))
        {
            cursor += 1;
        }
        if cursor == name_start {
            cursor += 1;
            continue;
        }
        let name = &attributes[name_start..cursor];
        while attributes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let Some(&b'=') = attributes.get(cursor) else {
            continue;
        };
        cursor += 1;
        while attributes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let (value, next) = attribute_value(attributes, cursor);
        cursor = next;
        if name.eq_ignore_ascii_case(b"charset") {
            return nonempty_ascii_value(value);
        }
        if name.eq_ignore_ascii_case(b"http-equiv") {
            http_equiv = Some(value);
        } else if name.eq_ignore_ascii_case(b"content") {
            content = Some(value);
        }
    }

    if http_equiv.is_some_and(|value| value.eq_ignore_ascii_case(b"content-type")) {
        return content.and_then(content_charset);
    }
    None
}

fn attribute_value(bytes: &[u8], start: usize) -> (&[u8], usize) {
    let Some(&first) = bytes.get(start) else {
        return (&[], start);
    };
    if matches!(first, b'\'' | b'"') {
        let value_start = start + 1;
        let end = bytes[value_start..]
            .iter()
            .position(|&byte| byte == first)
            .map_or(bytes.len(), |offset| value_start + offset);
        return (&bytes[value_start..end], end.saturating_add(1));
    }
    let end = bytes[start..]
        .iter()
        .position(u8::is_ascii_whitespace)
        .map_or(bytes.len(), |offset| start + offset);
    (&bytes[start..end], end)
}

fn nonempty_ascii_value(value: &[u8]) -> Option<String> {
    let value = value.strip_suffix(b"/").unwrap_or(value);
    (!value.is_empty()).then(|| String::from_utf8_lossy(value).into_owned())
}

fn content_charset(value: &[u8]) -> Option<String> {
    let marker = b"charset";
    if value.len() < marker.len() {
        return None;
    }
    let end = value.len() - marker.len();
    for index in 0..=end {
        if !value[index..index + marker.len()].eq_ignore_ascii_case(marker) {
            continue;
        }
        let mut cursor = index + marker.len();
        while value.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if value.get(cursor) != Some(&b'=') {
            continue;
        }
        cursor += 1;
        while value.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let (label, _) = attribute_value(value, cursor);
        if !label.is_empty() {
            return nonempty_ascii_value(label);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_transport_charset() {
        assert_eq!(
            decode_html(
                b"<p>caf\xe9</p>",
                Some("text/html; charset=\"windows-1252\"")
            ),
            "<p>café</p>"
        );
    }

    #[test]
    fn borrows_valid_utf8_without_a_transport_charset() {
        let html = "<p>café</p>".as_bytes();
        assert!(matches!(decode_html(html, None), Cow::Borrowed(_)));
        assert!(matches!(
            decode_html(html, Some("text/html")),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn a_byte_order_mark_wins_over_other_declarations() {
        let html = b"\xEF\xBB\xBF<meta charset=windows-1252><p>caf\xC3\xA9</p>";
        assert_eq!(
            decode_html(html, Some("text/html; charset=windows-1252")),
            "<meta charset=windows-1252><p>café</p>"
        );
    }

    #[test]
    fn uses_a_meta_charset_for_invalid_utf8() {
        let html = b"<meta charset=windows-1252><p>caf\xe9</p>";
        assert_eq!(
            decode_html(html, None),
            "<meta charset=windows-1252><p>café</p>"
        );
    }

    #[test]
    fn treats_a_meta_utf16_declaration_as_utf8() {
        let html = b"<meta charset=utf-16><p>caf\xC3\xA9 \xff</p>";
        assert_eq!(
            decode_html(html, None),
            "<meta charset=utf-16><p>café \u{FFFD}</p>"
        );
    }

    #[test]
    fn falls_back_to_windows_1252() {
        assert_eq!(decode_html(b"<p>caf\xe9</p>", None), "<p>café</p>");
    }

    #[test]
    fn detects_html_charset() {
        assert_eq!(
            html_charset(br#"<meta charset = 'windows-1252'>"#).as_deref(),
            Some("windows-1252")
        );
    }

    #[test]
    fn ignores_short_invalid_html() {
        assert_eq!(html_charset(&[0xff]), None);
    }

    #[test]
    fn ignores_charset_in_scripts() {
        let html =
            br#"<script>const value = "<meta charset=utf-8>";</script><meta charset=windows-1252>"#;
        assert_eq!(html_charset(html).as_deref(), Some("windows-1252"));
    }

    #[test]
    fn ignores_meta_in_comments() {
        let html = br#"<!-- <meta charset=utf-8> --><meta charset=windows-1252>"#;
        assert_eq!(html_charset(html).as_deref(), Some("windows-1252"));
    }

    #[test]
    fn detects_http_equiv_charset() {
        let html = br#"<meta http-equiv=content-type content="text/html; charset=windows-1252">"#;
        assert_eq!(html_charset(html).as_deref(), Some("windows-1252"));
    }

    #[test]
    fn detects_unquoted_content_charset() {
        let html = br#"<meta http-equiv=content-type content=text/html;charset=shift_jis>"#;
        assert_eq!(html_charset(html).as_deref(), Some("shift_jis"));
    }
}
