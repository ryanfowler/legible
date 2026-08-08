#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use support::input;

fuzz_target!(|data: &[u8]| {
    let Some(html) = input(data) else { return };

    // Document::new and the readability check must accept arbitrary, malformed
    // HTML without panicking. parse may return a documented extraction error.
    let document = legible::Document::new(html.as_ref());
    let _ = document.is_probably_readerable(None);
    let _ = document.parse(Some("https://example.com/"), None);
});
