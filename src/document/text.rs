use std::borrow::Cow;

use super::stats::is_visible_inline_character;

/// Classification returned with one normalized prose fragment.
///
/// The builder uses these facts when it appends or extends a text operation.
/// They describe only the new fragment, so extending an operation does not
/// need to inspect the text already in the arena.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct NormalizedAppend {
    pub(super) bytes_added: usize,
    pub(super) has_visible_text: bool,
    pub(super) starts_with_space: bool,
    pub(super) ends_with_space: bool,
    pub(super) first_non_space: Option<char>,
    pub(super) last_non_space: Option<char>,
}

pub(super) struct NormalizedFragment<'a> {
    value: Cow<'a, str>,
    pub(super) append: NormalizedAppend,
}

impl AsRef<str> for NormalizedFragment<'_> {
    fn as_ref(&self) -> &str {
        &self.value
    }
}

impl NormalizedFragment<'_> {
    pub(super) fn is_empty(&self) -> bool {
        self.append.bytes_added == 0
    }
}

/// Canonicalizes one prose fragment without changing code payloads.
///
/// A leading or trailing HTML whitespace run becomes one ASCII space. This
/// preserves a boundary across inline semantic nodes. Adjacent fragments are
/// merged by the builder.
pub(super) fn normalize_prose_fragment(value: &str) -> NormalizedFragment<'_> {
    if value.is_empty() {
        return NormalizedFragment {
            value: Cow::Borrowed(value),
            append: NormalizedAppend::default(),
        };
    }

    // The common path is already normalized ASCII. Classify it while checking
    // the normalization invariant so no second character scan is needed.
    if let Some(append) = classify_ascii_normalized(value) {
        return NormalizedFragment {
            value: Cow::Borrowed(value),
            append,
        };
    }

    let mut output = String::with_capacity(value.len().saturating_add(1));
    let mut pending_space = false;
    let mut append = NormalizedAppend::default();
    for character in value.chars() {
        if character.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space {
                output.push(' ');
            }
            pending_space = false;
            output.push(character);
            record_non_space(&mut append, character);
        }
    }
    if pending_space {
        output.push(' ');
    }
    append.bytes_added = output.len();
    append.starts_with_space = output.starts_with(' ');
    append.ends_with_space = output.ends_with(' ');
    NormalizedFragment {
        value: Cow::Owned(output),
        append,
    }
}

fn classify_ascii_normalized(value: &str) -> Option<NormalizedAppend> {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut append = NormalizedAppend::default();
    let mut previous_space = false;
    for (index, &byte) in bytes.iter().enumerate() {
        if byte >= 0x80 {
            return None;
        }
        if index == 0 {
            append.starts_with_space = byte == b' ';
        }
        append.ends_with_space = byte == b' ';
        if byte.is_ascii_whitespace() {
            if byte != b' ' || previous_space {
                return None;
            }
            previous_space = true;
        } else {
            previous_space = false;
            record_non_space(&mut append, byte as char);
        }
    }
    append.bytes_added = bytes.len();
    Some(append)
}

fn record_non_space(append: &mut NormalizedAppend, character: char) {
    append.first_non_space.get_or_insert(character);
    append.last_non_space = Some(character);
    append.has_visible_text |= is_visible_inline_character(character);
}

#[cfg(test)]
pub(super) fn merge_prose(existing: &mut String, next: &str) {
    if existing.ends_with(' ') && next.starts_with(' ') {
        existing.push_str(&next[1..]);
    } else {
        existing.push_str(next);
    }
}

/// Accumulates prose fragments when a caller needs one canonical text value.
#[derive(Default)]
#[cfg(test)]
pub(crate) struct ProseTextAccumulator {
    value: String,
}

#[cfg(test)]
impl ProseTextAccumulator {
    pub(crate) fn push(&mut self, fragment: &str) {
        let fragment = normalize_prose_fragment(fragment);
        merge_prose(&mut self.value, fragment.as_ref());
    }

    pub(crate) fn finish(self) -> String {
        self.value.trim().to_owned()
    }
}

/// Returns preformatted content unchanged.
#[cfg(test)]
pub(crate) fn preformatted_text(value: &str) -> Box<str> {
    value.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_collapses_whitespace_across_fragments() {
        let mut text = ProseTextAccumulator::default();
        text.push(" Hello\t");
        text.push(" \u{2003}world ");
        assert_eq!(text.finish(), "Hello world");
    }

    #[test]
    fn normalized_ascii_prose_is_borrowed() {
        let value = "already normalized";
        assert_eq!(normalize_prose_fragment(value).as_ref(), value);
    }

    #[test]
    fn normalized_ascii_boundary_spaces_are_borrowed() {
        for value in [" leading", "trailing ", " leading ", " "] {
            assert_eq!(normalize_prose_fragment(value).as_ref(), value);
        }
    }

    #[test]
    fn normalized_fragment_reports_boundaries_and_visibility() {
        let fragment = normalize_prose_fragment("  Hello\tworld  ");
        assert_eq!(fragment.as_ref(), " Hello world ");
        assert_eq!(
            fragment.append,
            NormalizedAppend {
                bytes_added: 13,
                has_visible_text: true,
                starts_with_space: true,
                ends_with_space: true,
                first_non_space: Some('H'),
                last_non_space: Some('d'),
            }
        );
    }

    #[test]
    fn split_fragments_match_whole_fragment_metadata() {
        let whole = "  Alpha\u{2003}beta\u{200b} gamma  ";
        let expected = normalize_prose_fragment(whole);
        let mut combined = String::new();
        for split in whole
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(whole.len()))
        {
            let (left, right) = whole.split_at(split);
            let left = normalize_prose_fragment(left);
            let right = normalize_prose_fragment(right);
            combined.clear();
            merge_prose(&mut combined, left.as_ref());
            merge_prose(&mut combined, right.as_ref());
            assert_eq!(combined, expected.as_ref());
        }
    }

    #[test]
    fn preformatted_text_is_unchanged() {
        assert_eq!(preformatted_text("  a\n\tb\n").as_ref(), "  a\n\tb\n");
    }
}
