#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use legible::{ContentHint, ContentTag, Extractor, ParseBudget};
use support::MAX_INPUT_SIZE;

// The first bytes select the extractor and renderer options. The remaining
// bytes are the document. Extraction must not panic for any option set, and
// two extractions with the same options must produce the same output.
fuzz_target!(|data: &[u8]| {
    if data.len() < 4 || data.len() > MAX_INPUT_SIZE {
        return;
    }
    let (options, html) = data.split_at(4);
    let flags = options[0];
    let hint = options[1];
    let width = usize::from(options[2]);
    let budget = options[3];

    let mut builder = Extractor::builder()
        .structured_data(flags & 1 != 0)
        .diagnostics(flags & 2 != 0)
        .metadata_diagnostics(flags & 4 != 0)
        .retain_structured_data(flags & 8 != 0);
    let selector = match hint % 6 {
        0 => None,
        1 => Some(ContentHint::Id("main".into())),
        2 => Some(ContentHint::Class("content".into())),
        3 => Some(ContentHint::Tag(ContentTag::Article)),
        4 => Some(ContentHint::Tag(ContentTag::Main)),
        _ => Some(ContentHint::Tag(ContentTag::Div)),
    };
    if let Some(selector) = selector {
        builder = if flags & 16 != 0 {
            builder.content_root(selector)
        } else {
            builder.content_hint(selector)
        };
    }
    if budget & 1 != 0 {
        builder = builder.parse_budget(ParseBudget::untrusted());
    } else if budget & 2 != 0 {
        // Small limits exercise the resource-limit paths.
        builder = builder.parse_budget(ParseBudget {
            max_nodes: usize::from(budget) * 8,
            max_depth: usize::from(budget >> 2),
            max_json_ld_items: usize::from(budget >> 4),
            ..ParseBudget::default()
        });
    }
    let extractor = builder.build();

    let content_type = (flags & 32 != 0).then_some("text/html; charset=windows-1252");
    let first = extractor.extract_bytes(html, Some("https://example.com/a/b"), content_type);
    let second = extractor.extract_bytes(html, Some("https://example.com/a/b"), content_type);
    match (first, second) {
        (Ok(first), Ok(second)) => {
            let render = |page: &legible::ExtractedPage| {
                let mut builder = page
                    .markdown_builder()
                    .links(flags & 64 != 0)
                    .images(flags & 128 != 0);
                if width > 0 {
                    builder = builder.max_line_width(width);
                }
                builder.render()
            };
            assert_eq!(render(&first), render(&second));
            assert_eq!(first.html(), second.html());
            assert_eq!(first.content().metrics(), second.content().metrics());
            assert_eq!(first.diagnostics().is_some(), flags & 2 != 0);
            assert_eq!(first.structured_data().is_some(), flags & 8 != 0);
            assert!(first.validate_document());
        }
        (Err(first), Err(second)) => assert_eq!(first.to_string(), second.to_string()),
        _ => panic!("extraction is not deterministic"),
    }
});
