#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use support::{article_document, input, parse_article, reparse_serialized};

fuzz_target!(|data: &[u8]| {
    let Some(body) = input(data) else { return };
    let html = article_document(body.as_ref());

    // parse renders Markdown and normalized text from the final cleaned DOM.
    // Reading both fields here keeps this target focused on both serializers.
    let Some(article) = parse_article(&html) else {
        return;
    };
    let _markdown = &article.markdown_content;
    let _text = &article.text_content;
    reparse_serialized(&article.content);
});
