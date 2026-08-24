#![no_main]

use libfuzzer_sys::fuzz_target;

#[allow(dead_code, unused_imports)]
#[path = "../../src/dom/mod.rs"]
mod dom;
mod support;

use support::{escape_attribute, input, parse_article, reparse_serialized};

fuzz_target!(|data: &[u8]| {
    let Some(payload) = input(data) else { return };
    let attribute = escape_attribute(payload.as_ref());
    let variant = data.first().copied().unwrap_or(0) % 8;
    let body = match variant {
        0 => format!(r#"<img src="fallback.jpg" srcset="{attribute}" alt="Responsive image">"#),
        1 => format!(
            r#"<ol><li><ul><li><ol><li>{}</ol></ul></ol>"#,
            payload.as_ref()
        ),
        2 => format!(
            r##"<a href="#note-{attribute}" role="doc-noteref">1</a><aside id="note-{attribute}" role="doc-footnote">{}</aside>"##,
            payload.as_ref()
        ),
        3 => format!(
            r#"<span class="katex" data-latex="{attribute}"><math><semantics><mrow>{}</mrow><annotation encoding="application/x-tex">{}</annotation></semantics></math></span>"#,
            payload.as_ref(),
            payload.as_ref()
        ),
        4 => format!(
            r#"<a href="{attribute}">Strange link</a><img src="{attribute}" alt="Strange image">"#
        ),
        5 => format!(
            r#"<div onclick="{attribute}" style="{attribute}" data-value="{attribute}">Sanitized attributes</div>"#
        ),
        6 => format!(
            r#"<figure><img src="same.jpg" alt="Diagram"><span><img src="same.jpg" srcset="same.jpg 1x, {attribute} 2x" alt="Diagram"></span><figcaption>{}</figcaption></figure>"#,
            payload.as_ref()
        ),
        _ => format!(
            r#"<table><tr><td>1.</td><td><a href="/{attribute}">One</a></td></tr><tr><td></td><td>{}</td></tr><tr><td>2.</td><td><a href="/two">Two</a></td></tr><tr><td></td><td>Details</td></tr><tr><td>3.</td><td><a href="/three">Three</a></td></tr><tr><td></td><td>Details</td></tr></table>"#,
            payload.as_ref()
        ),
    };
    let html = format!(
        "<html><body><article><h1>Normalization fuzz</h1><p>This article contains enough stable text for extraction and semantic normalization.</p>{body}<p>Trailing content keeps the selected region coherent.</p></article></body></html>"
    );

    let Some(page) = parse_article(&html) else {
        return;
    };
    let markdown = page.markdown();
    let text = page.text();
    let html = page.html();
    assert!(page.validate_document());
    assert_eq!(page.text_length(), text.chars().count());
    assert!(page.word_count() <= page.text_length());
    assert_eq!(page.word_count() == 0, text.is_empty());
    let _ = markdown.len();
    reparse_serialized(&html);
    validate_serialized_dom(&html);
});

fn validate_serialized_dom(content: &str) {
    let parsed = dom::Dom::parse_document(content).expect("serialized HTML must parse");
    let root = parsed.root();
    assert!(parsed.parent(root).is_none());
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(parent) = stack.pop() {
        assert!(seen.insert(parent), "serialized DOM contains a cycle");
        let mut previous = None;
        for child in parsed.children(parent) {
            assert_eq!(parsed.parent(child), Some(parent));
            assert_eq!(parsed.prev_sibling(child), previous);
            if let Some(previous) = previous {
                assert_eq!(parsed.next_sibling(previous), Some(child));
            }
            previous = Some(child);
            stack.push(child);
        }
        assert_eq!(previous, parsed.last_child(parent));
    }
}
