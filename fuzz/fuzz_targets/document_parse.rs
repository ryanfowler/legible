#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use support::input;

fuzz_target!(|data: &[u8]| {
    let Some(html) = input(data) else { return };

    // Extraction must accept arbitrary, malformed HTML without panicking. It can
    // return a documented extraction error.
    let _ = legible::extract(html.as_ref(), Some("https://example.com/"));

    // Charset detection must accept arbitrary bytes and transport values.
    let content_type = data
        .split(|&byte| byte == b'\n')
        .next()
        .and_then(|line| std::str::from_utf8(line).ok());
    let _ = legible::Extractor::default().extract_bytes(data, None, content_type);
});
