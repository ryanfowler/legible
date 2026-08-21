//! Case-insensitive whitespace-separated token matching.

/// True when any whitespace-separated token equals `expected` (ASCII case-insensitive).
#[inline]
pub(crate) fn has_token(value: &str, expected: &str) -> bool {
    if value.is_ascii() {
        value
            .split_ascii_whitespace()
            .any(|token| token.eq_ignore_ascii_case(expected))
    } else {
        value
            .split_whitespace()
            .any(|token| token.eq_ignore_ascii_case(expected))
    }
}

/// True when any token equals any entry in `expected`.
#[inline]
pub(crate) fn has_any_token(value: &str, expected: &[&str]) -> bool {
    if value.is_ascii() {
        value.split_ascii_whitespace().any(|token| {
            expected
                .iter()
                .any(|candidate| token.eq_ignore_ascii_case(candidate))
        })
    } else {
        value.split_whitespace().any(|token| {
            expected
                .iter()
                .any(|candidate| token.eq_ignore_ascii_case(candidate))
        })
    }
}

/// True when any token contains `needle` (ASCII case-insensitive).
#[inline]
pub(crate) fn any_token_contains(value: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return value.split_whitespace().next().is_some();
    }
    if value.is_ascii() && needle.is_ascii() {
        value
            .split_ascii_whitespace()
            .any(|token| contains_ascii_case_insensitive(token, needle))
    } else {
        value
            .split_whitespace()
            .any(|token| contains_ascii_case_insensitive(token, needle))
    }
}

#[inline]
fn contains_ascii_case_insensitive(value: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    // Short values scan directly; memchr setup does not pay off yet.
    if value.len() <= 24 {
        return value
            .as_bytes()
            .windows(needle.len())
            .any(|candidate| candidate.eq_ignore_ascii_case(needle.as_bytes()));
    }
    // memchr finds candidate positions for the first needle byte quickly;
    // each candidate then needs one short case-insensitive compare.
    let value_bytes = value.as_bytes();
    let needle_bytes = needle.as_bytes();
    let first = needle_bytes[0];
    let mut start = 0;
    while let Some(position) = memchr::memchr2(
        first.to_ascii_lowercase(),
        first.to_ascii_uppercase(),
        &value_bytes[start..],
    ) {
        let candidate = start + position;
        if candidate + needle_bytes.len() <= value_bytes.len()
            && value_bytes[candidate..candidate + needle_bytes.len()]
                .eq_ignore_ascii_case(needle_bytes)
        {
            return true;
        }
        start = candidate + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ascii_tokens_without_allocating() {
        assert!(has_token("  MAIN\tnavigation ", "main"));
        assert!(has_any_token(
            "  MAIN\tnavigation ",
            &["dialog", "navigation"]
        ));
        assert!(!has_token("main-content", "main"));
    }

    #[test]
    fn matches_unicode_whitespace_and_ascii_case() {
        assert!(has_token("préface\u{2003}MAIN", "main"));
        assert!(has_any_token("préface\u{2003}MAIN", &["main"]));
    }

    #[test]
    fn matches_case_insensitive_substrings_in_tokens() {
        assert!(any_token_contains("header gh-Header-title", "header"));
        assert!(!any_token_contains("header gh-title", "nav"));
        assert!(any_token_contains("header", ""));
        assert!(!any_token_contains("   ", ""));
    }
}
