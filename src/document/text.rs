use std::borrow::Cow;

/// Canonicalizes one prose fragment without changing code payloads.
///
/// A leading or trailing HTML whitespace run becomes one ASCII space. This
/// preserves a boundary across inline semantic nodes. Adjacent fragments are
/// merged by the builder.
pub(super) fn normalize_prose_fragment(value: &str) -> Cow<'_, str> {
    // HTML prose is usually already ASCII-normalized. Keep it borrowed so the
    // builder can append it without creating a short-lived copy.
    if value.is_empty() || is_ascii_normalized(value) {
        return Cow::Borrowed(value);
    }

    let mut output = String::with_capacity(value.len().saturating_add(1));
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space {
                output.push(' ');
            }
            pending_space = false;
            output.push(character);
        }
    }
    if pending_space {
        output.push(' ');
    }
    Cow::Owned(output)
}

fn is_ascii_normalized(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.is_ascii()
        && !bytes.is_empty()
        && bytes
            .iter()
            .all(|&byte| !byte.is_ascii_whitespace() || byte == b' ')
        && !bytes.windows(2).any(|window| window == b"  ")
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
        assert!(matches!(
            normalize_prose_fragment(value),
            Cow::Borrowed(fragment) if fragment == value
        ));
    }

    #[test]
    fn normalized_ascii_boundary_spaces_are_borrowed() {
        for value in [" leading", "trailing ", " leading ", " "] {
            assert!(matches!(
                normalize_prose_fragment(value),
                Cow::Borrowed(fragment) if fragment == value
            ));
        }
    }

    #[test]
    fn preformatted_text_is_unchanged() {
        assert_eq!(preformatted_text("  a\n\tb\n").as_ref(), "  a\n\tb\n");
    }
}
