use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DestinationKind {
    Link,
    Resource,
}

/// Trims the characters rejected at the edges of a URI attribute.
///
/// HTML URLs are almost always ASCII. Keep that path byte-oriented and use
/// the Unicode predicate only for the uncommon non-ASCII case.
#[inline]
pub(crate) fn trim_destination(value: &str) -> &str {
    if value.is_ascii() {
        let bytes = value.as_bytes();
        let mut start = 0;
        while start < bytes.len() && (bytes[start] <= b' ' || bytes[start] == 0x7f) {
            start += 1;
        }
        let mut end = bytes.len();
        while end > start && (bytes[end - 1] <= b' ' || bytes[end - 1] == 0x7f) {
            end -= 1;
        }
        &value[start..end]
    } else {
        value.trim_matches(|character: char| {
            character.is_ascii_whitespace() || character.is_control()
        })
    }
}

/// Resolves a destination and applies the semantic URI policy.
///
/// Relative destinations stay relative when no base URL is available.
pub(crate) fn safe_destination(
    value: &str,
    base_url: Option<&Url>,
    kind: DestinationKind,
) -> Option<Box<str>> {
    let value = trim_destination(value);
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }

    if let Some(end) = scheme_end(value) {
        let scheme = &value[..end];
        if !valid_scheme(scheme) || !allowed_scheme(scheme, kind) {
            return None;
        }
    }

    if value.starts_with('#') {
        return Some(value.into());
    }

    if let Some(base_url) = base_url {
        let resolved = base_url.join(value).ok()?;
        if !allowed_scheme(resolved.scheme(), kind) {
            return None;
        }
        return Some(resolved.to_string().into());
    }
    Some(value.into())
}

fn valid_scheme(scheme: &str) -> bool {
    !scheme.is_empty()
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn allowed_scheme(scheme: &str, kind: DestinationKind) -> bool {
    match kind {
        DestinationKind::Link => ["http", "https", "mailto", "tel"]
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed)),
        DestinationKind::Resource => ["http", "https"]
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed)),
    }
}

fn scheme_end(value: &str) -> Option<usize> {
    let mut offset = 0;
    while let Some(character) = value[offset..].chars().next() {
        match character {
            ':' => return Some(offset),
            '/' | '?' | '#' => return None,
            '&' if is_colon_entity(&value[offset..]) => return Some(offset),
            _ => offset += character.len_utf8(),
        }
    }
    None
}

fn is_colon_entity(value: &str) -> bool {
    if value
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("&colon;"))
    {
        return true;
    }
    let Some(mut value) = value.strip_prefix("&#") else {
        return false;
    };
    let (radix, remaining) = value
        .strip_prefix('x')
        .or_else(|| value.strip_prefix('X'))
        .map_or((10, value), |remaining| (16, remaining));
    value = remaining;
    let digits = value
        .bytes()
        .take_while(|byte| match radix {
            10 => byte.is_ascii_digit(),
            16 => byte.is_ascii_hexdigit(),
            _ => false,
        })
        .count();
    digits > 0 && u32::from_str_radix(&value[..digits], radix) == Ok(u32::from(':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_and_fragment_destinations() {
        let base = Url::parse("https://example.test/docs/page").unwrap();
        assert_eq!(
            safe_destination("../guide", Some(&base), DestinationKind::Link).as_deref(),
            Some("https://example.test/guide")
        );
        assert_eq!(
            safe_destination("#part", Some(&base), DestinationKind::Link).as_deref(),
            Some("#part")
        );
        assert_eq!(
            safe_destination("mailto:a@example.test", None, DestinationKind::Link).as_deref(),
            Some("mailto:a@example.test")
        );
        assert_eq!(
            safe_destination("tel:123", None, DestinationKind::Link).as_deref(),
            Some("tel:123")
        );
    }

    #[test]
    fn rejects_unsafe_obfuscated_and_empty_destinations() {
        for value in [
            "",
            "  ",
            "java\nscript:alert(1)",
            "javascript&colon;alert(1)",
            "javascript&#58;alert(1)",
            "data:text/html,x",
        ] {
            assert_eq!(safe_destination(value, None, DestinationKind::Link), None);
        }
        assert_eq!(
            safe_destination("data:image/svg+xml,x", None, DestinationKind::Resource),
            None
        );
        assert_eq!(
            safe_destination("mailto:a@example.test", None, DestinationKind::Resource),
            None
        );
        let ftp = Url::parse("ftp://example.test/base/").unwrap();
        assert_eq!(
            safe_destination("image.png", Some(&ftp), DestinationKind::Resource),
            None
        );
    }
}
