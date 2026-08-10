#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use support::{input, parse_article, reparse_serialized};

fuzz_target!(|data: &[u8]| {
    let Some(payload) = input(data) else { return };
    let depth = 512 + data.first().copied().unwrap_or(0) as usize * 6;

    let mut html = String::with_capacity(payload.len().saturating_add(depth * 12));
    html.push_str("<html><body><article><p>Deep content ");
    for _ in 0..depth {
        html.push_str("<div><span>");
    }
    html.push_str(payload.as_ref());
    // Deliberately close only one of each pair. html5ever must repair this
    // malformed tree, and all Legible traversals must remain stack-safe.
    for _ in 0..depth {
        html.push_str("</div>");
    }
    html.push_str("</p><p>Trailing article text.</p></article></body></html>");

    let Some(page) = parse_article(&html) else {
        return;
    };
    let _ = page.markdown();
    let _ = page.text();
    reparse_serialized(&page.html());
});
