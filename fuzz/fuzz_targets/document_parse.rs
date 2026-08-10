#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use support::input;

fuzz_target!(|data: &[u8]| {
    let Some(html) = input(data) else { return };

    // Extraction must accept arbitrary, malformed HTML without panicking. It can
    // return a documented extraction error.
    let _ = legible::extract(html.as_ref(), Some("https://example.com/"));
});
