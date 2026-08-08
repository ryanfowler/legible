# Fuzz targets

Install cargo-fuzz and the nightly Rust toolchain, then run a target from the
repository root:

```text
rustup toolchain install nightly
cargo install cargo-fuzz
cargo +nightly fuzz run document_parse
```

Available targets:

- `document_parse` checks `Document::new`, the readerable check, and `parse`.
- `dom_mutation_serialization` exercises extraction mutations and reparses HTML output.
- `render_markdown_text` exercises Markdown and normalized text rendering.
- `json_ld_metadata` exercises valid and malformed JSON-LD metadata.
- `url_rewriting` exercises link, media, and `srcset` URL rewriting.
- `nested_malformed_html` exercises deep nesting and HTML parser repair.

Each target skips inputs larger than 256 KiB. Cargo-fuzz currently uses nightly
Rust because it enables unstable sanitizer compiler options. The targets treat extraction errors as
valid results. A panic, stack overflow, or failure to reparse serialized article HTML
is a fuzzing failure.
