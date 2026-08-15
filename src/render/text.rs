//! Normalized text rendering from the semantic document.

use crate::document::Document;

#[derive(Clone, Debug, Default)]
pub(crate) struct TextOptions {
    pub(crate) block_newlines: bool,
    pub(crate) preserve_line_breaks: bool,
}

pub(crate) fn render_text(document: &Document, capacity: usize, options: &TextOptions) -> String {
    crate::document::stats::walk_text(
        document,
        options.block_newlines,
        options.preserve_line_breaks,
        Some(capacity),
        false,
    )
    .0
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{CodeBlock, DocumentBuilder, NodeKind};

    #[test]
    fn deeply_nested_text_rendering_is_stack_safe() {
        const DEPTH: usize = 10_000;
        let mut builder = DocumentBuilder::with_capacity(DEPTH + 1);
        let mut parent = None;
        for _ in 0..DEPTH {
            parent = Some(builder.append(parent, NodeKind::BlockGroup).unwrap());
        }
        builder.append_prose(parent, "deep").unwrap();
        let document = builder.finish();
        assert_eq!(render_text(&document, 0, &TextOptions::default()), "deep");
        assert_eq!(document.text_length(), 4);
    }

    #[test]
    fn semantic_boundaries_drive_text_and_metrics() {
        let mut builder = DocumentBuilder::with_capacity(8);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder
            .append_prose(Some(paragraph), "Hello world")
            .unwrap();
        builder
            .append(
                None,
                NodeKind::CodeBlock(CodeBlock {
                    language: None,
                    text: "a  b\n".into(),
                }),
            )
            .unwrap();
        let final_paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(final_paragraph), "x").unwrap();
        builder
            .append(Some(final_paragraph), NodeKind::HardBreak)
            .unwrap();
        builder.append_prose(Some(final_paragraph), "y").unwrap();
        let document = builder.finish();
        let text = render_text(&document, 0, &TextOptions::default());
        let stats = document.stats();
        assert_eq!(text, "Hello world a b x y");
        assert_eq!(stats.text_length, text.chars().count());
        assert_eq!(stats.word_count, 6);
    }
}
