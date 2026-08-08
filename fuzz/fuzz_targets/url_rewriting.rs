#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use support::{escape_attribute, input, parse_article, reparse_serialized};

fuzz_target!(|data: &[u8]| {
    let Some(value) = input(data) else { return };
    let value = escape_attribute(value.as_ref());
    let html = format!(
        "<html><head><base href=\"https://cdn.example/assets/\"></head>\
         <body><article><p>URL rewriting has enough text for extraction. \
         <a href=\"{value}\">link</a> <img src=\"{value}\" alt=\"image\">\
         <source srcset=\"{value} 1x, fallback.png 2x\">\
         More article text follows here.</p></article></body></html>"
    );

    // The extraction post-processing resolves links, media URLs, and srcset
    // entries against the base URL. Invalid URLs must remain harmless inputs.
    let Some(article) = parse_article(&html) else {
        return;
    };
    reparse_serialized(&article.content);
});
