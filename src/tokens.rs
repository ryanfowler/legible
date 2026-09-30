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

/// True when `value` contains any of `needles` (case-sensitive).
pub(crate) fn contains_any(value: &str, needles: &[&str]) -> bool {
    // Most values are short class and ID names. One pass over their bytes is
    // cheaper than a substring search setup for each needle. Long text uses
    // the vectorized search.
    if value.len() > 64 {
        return needles.iter().any(|needle| value.contains(needle));
    }
    let mut first_bytes = [false; 256];
    for needle in needles {
        match needle.as_bytes().first() {
            Some(&byte) => first_bytes[usize::from(byte)] = true,
            None => return true,
        }
    }
    let bytes = value.as_bytes();
    bytes.iter().enumerate().any(|(start, &byte)| {
        first_bytes[usize::from(byte)]
            && needles
                .iter()
                .any(|needle| bytes[start..].starts_with(needle.as_bytes()))
    })
}

/// True when `value` contains `needle` (ASCII case-insensitive).
#[inline]
pub(crate) fn contains_ascii_case_insensitive(value: &str, needle: &str) -> bool {
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
    fn finds_any_substring_in_short_and_long_values() {
        let needles = ["navbar", "footer"];
        assert!(contains_any("site-footer", &needles));
        assert!(contains_any("navbar", &needles));
        assert!(!contains_any("site-foote", &needles));
        assert!(!contains_any("", &needles));
        assert!(contains_any("anything", &[""]));

        let long = format!("{} footer", "x".repeat(80));
        assert!(contains_any(&long, &needles));
        assert!(!contains_any(&"x".repeat(80), &needles));
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
