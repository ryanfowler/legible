/// Canonicalizes one prose fragment without changing code payloads.
///
/// A leading or trailing HTML whitespace run becomes one ASCII space. This
/// preserves a boundary across inline semantic nodes. Adjacent fragments are
/// merged by the builder.
pub(super) fn normalize_prose_fragment(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
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
    output
}

pub(super) fn merge_prose(existing: &mut String, next: &str) {
    if existing.ends_with(' ') && next.starts_with(' ') {
        existing.push_str(&next[1..]);
    } else {
        existing.push_str(next);
    }
}

/// Accumulates prose fragments when a caller needs one canonical text value.
#[derive(Default)]
pub(crate) struct ProseTextAccumulator {
    value: String,
}

impl ProseTextAccumulator {
    pub(crate) fn push(&mut self, fragment: &str) {
        let fragment = normalize_prose_fragment(fragment);
        merge_prose(&mut self.value, &fragment);
    }

    pub(crate) fn finish(self) -> String {
        self.value.trim().to_owned()
    }
}

/// Returns preformatted content unchanged.
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
    fn preformatted_text_is_unchanged() {
        assert_eq!(preformatted_text("  a\n\tb\n").as_ref(), "  a\n\tb\n");
    }
}
