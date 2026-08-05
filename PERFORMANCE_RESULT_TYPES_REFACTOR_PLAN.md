# Single-format extraction refactor plan

## Goal

Make single-format extraction the primary API. Render the selected format from the cleaned extraction DOM, then drop the DOM before returning. Do not freeze a public `ArticleTree` for the normal extraction path.

This is a breaking public API change. Keep the deprecated `parse` compatibility adapter, but replace the tree-backed `Article` API and its `to_*` methods.

## Target public API

Add these public result types in `src/article.rs` and re-export them from `src/lib.rs`:

```rust
pub struct HtmlArticle {
    metadata: ArticleMetadata,
    content: String,
    text_char_count: usize,
}

pub struct MarkdownArticle {
    metadata: ArticleMetadata,
    content: String,
    text_char_count: usize,
}

pub struct TextArticle {
    metadata: ArticleMetadata,
    content: String,
    text_char_count: usize,
}
```

Each type must implement `Debug` and these methods:

```rust
pub fn metadata(&self) -> &ArticleMetadata;
pub fn content(&self) -> &str;
pub fn into_content(self) -> String;
pub fn text_char_count(&self) -> usize;
```

The types must remain `Send + Sync`. Do not expose the extraction DOM or `NodeId`.

Add this base method set to `Extractor`:

```rust
pub fn extract_html(&self, html: &str) -> Result<HtmlArticle>;
pub fn extract_markdown(&self, html: &str) -> Result<MarkdownArticle>;
pub fn extract_markdown_with(
    &self,
    html: &str,
    options: &MarkdownOptions,
) -> Result<MarkdownArticle>;
pub fn extract_text(&self, html: &str) -> Result<TextArticle>;
pub fn extract_text_with(
    &self,
    html: &str,
    options: &TextOptions,
) -> Result<TextArticle>;
```

Provide equivalent input variants with consistent names:

- `extract_{format}_with_url(html, url)`
- `extract_document_{format}(document)`
- `extract_document_{format}_with_url(document, url)`

For Markdown and text options, also provide:

- `extract_{format}_with_url_and_options(html, url, options)`
- `extract_document_{format}_with(document, options)`
- `extract_document_{format}_with_url_and_options(document, url, options)`

Keep the argument order as input, URL when present, then format options. Route all variants through a small set of private helpers. Do not duplicate size checks, parsing, or readability setup.

Replace the current free `extract` and `extract_with_url` functions with `extract_html` and `extract_html_with_url`. Do not add free functions for every format and option combination. Update all current first-class examples to use `Extractor` when they need Markdown or text.

Remove public `Article` and its on-demand render methods. Do not keep a deprecated tree-backed path because it would preserve the allocation and retained-memory cost that this refactor removes. Keep the already deprecated `parse` and `legacy::Article` API through the next compatibility period.

## Internal design

### 1. Return the cleaned source from Readability

In `src/readability.rs`, add a private-to-the-crate result:

```rust
pub(crate) struct ExtractedArticle {
    dom: Dom,
    root: NodeId,
    metadata: ArticleMetadata,
    text_char_count: usize,
}
```

Use private fields plus narrow `pub(crate)` rendering/access methods, or add one `into_parts` method. Avoid broad access to Readability internals.

Rename/refactor `Readability::extract` to `extract_source`. Preserve all current behavior before the final `ArticleTree::freeze` call:

1. Validate the base URL and element limit.
2. Collect all metadata sources.
3. prepare, score, retry, and clean the document.
4. Preserve `Error::NoBody`, `Error::NoContent`, and `Error::TooManyElements` behavior.
5. Return the final DOM, output-fragment root, merged metadata, and normalized character count.

The successful normal path should move `self.dom` into `ExtractedArticle`. The below-threshold retry path must continue to retain only the best compact copied subtree. Do not retain all retry DOMs. Its returned root must belong to the moved best-attempt DOM.

Pay close attention to fragment semantics: serializers render the **children** of `root`, not `root` itself. This must match current `ArticleTree::freeze` output on both the normal article root and the retry fragment root.

### 2. Render directly from the DOM

Add crate-private rendering functions with one clear interface each:

```rust
fn render_html(dom: &Dom, root: NodeId, capacity: usize) -> String;
fn render_markdown(
    dom: &Dom,
    root: NodeId,
    capacity: usize,
    options: &MarkdownOptions,
) -> String;
fn render_text(
    dom: &Dom,
    root: NodeId,
    capacity: usize,
    options: &TextOptions,
) -> String;
```

The `capacity` value is `text_char_count`. Keep it as an output capacity hint. Do not treat it as a byte length or strict output size.

#### HTML

Move `src/dom/serialize.rs` out of its current test-only module configuration. Add an infallible crate-private children serializer for output to an in-memory buffer. Preallocate its byte buffer with the capacity hint. Keep the existing iterative serializer and `html5ever` escaping behavior. Convert impossible in-memory write failures with an explicit `expect`, as the current tree serializer does.

Do not serialize and parse again. `HtmlArticle::content` must remain an HTML fragment and must not include the output root wrapper itself.

#### Markdown

Continue to use `markdown::tree_to_markdown_filtered` directly with `Dom`, `NodeId`, `include_links`, and `include_images`. Keep the iterative task stack and all CommonMark escaping and URI filtering behavior.

Apply `MarkdownOptions` heading and bullet transformations after direct DOM serialization. Move or expose only the minimum crate-private option logic required. Do not make option fields public.

#### Text

Move the normalized text output logic from `src/article_tree.rs` into a focused module such as `src/text.rs`. Implement direct iterative DOM traversal for:

- default normalized text;
- block separators;
- preserved `<br>` line breaks;
- skipped `<template>` contents.

Preserve the byte-wise ASCII fast path in `NormalizedOutput`. Preserve punctuation and inline-node boundaries. Use an explicit work stack so untrusted nesting cannot overflow the Rust call stack. Do not use serialized HTML or a temporary `ArticleTree`.

Keep the invariant for default text:

```rust
result.content().chars().count() == result.text_char_count()
```

Options can add line-break structure, but must not change the stored normalized source character count.

### 3. Centralize result construction

In `src/extractor.rs`, each public method should:

1. enforce `max_input_bytes` before parsing when it receives `&str`;
2. parse only once;
3. call one private source-extraction helper;
4. render exactly one requested format;
5. move metadata and count into the explicit result;
6. let the DOM drop before returning.

Use private `render_*_article` helpers to avoid repeating extraction setup across HTML, Markdown, and text. Do not introduce a public generic format enum or a shallow public wrapper.

Ensure `Document` entry points consume `Document<'_>` and still apply the input-size limit from `document.html`. URL variants must continue to use typed `&Url` values.

### 4. Preserve the legacy adapter without `ArticleTree`

Refactor `Readability::parse` in `src/readability.rs` to call `extract_source` once. Render HTML, Markdown, and text directly from that one cleaned DOM before dropping it. Then build `LegacyArticle` from the rendered strings and `ArticleMetadata`.

Do not implement `parse` by calling three public single-format methods. That would parse and extract three times. Preserve every legacy field and current URL/error behavior.

After this works, delete `src/article_tree.rs` and remove `mod article_tree` from `src/lib.rs`. Remove all `ArticleTree::freeze`, retained-node-count, and render-only tree code.

## Implementation sequence

### Phase 0: Record the baseline

Before changing code:

1. Run the full validation commands listed below.
2. Save a Criterion baseline for the existing tree-backed implementation:

   ```bash
   cargo bench --bench readability -- --save-baseline tree-backed
   ```

3. Record results for `medium-2` and at least one large retained article fixture.
4. If allocation and peak-memory claims are part of release notes, collect heap profiles for the old implementation now. Criterion latency alone does not prove allocation or peak-memory improvements.

### Phase 1: Add direct renderer parity tests

Before removing `ArticleTree`, extend unit tests so the same cleaned DOM is rendered through both implementations. Cover:

- exact HTML fragment output;
- default and filtered Markdown;
- default text and every `TextOptions` combination;
- comments and `<template>` elements;
- inline punctuation and adjacent spans;
- Unicode and ASCII whitespace;
- deeply nested markup;
- retry output where `root` is the copied fragment root.

Use these tests as a temporary migration oracle. Delete only tests that require `ArticleTree` after equivalent expected-output tests exist for the direct DOM renderers.

### Phase 2: Introduce `ExtractedArticle` and direct result types

1. Split source extraction from rendering in `src/readability.rs`.
2. Add the three result types and shared method behavior in `src/article.rs`.
3. Add direct HTML and text renderers.
4. Reuse the existing direct DOM Markdown renderer.
5. Add the base `Extractor` methods first.
6. Add URL, options, and `Document` variants through private helpers.
7. Refactor legacy `parse` to render all formats from one source result.

Run focused tests after each renderer is connected.

### Phase 3: Remove the tree-backed API

1. Migrate internal tests, benchmarks, crate docs, and README examples.
2. Replace free `extract` functions with explicit HTML free functions.
3. Remove `Article`, `ArticleTree`, and `src/article_tree.rs`.
4. Remove dead renderer traits or methods only if the direct DOM path no longer uses them. Keep `MarkdownTree` if it remains a useful deep internal interface; otherwise simplify it to DOM-only code without changing output.
5. Run `cargo clippy` and remove all dead code.

### Phase 4: Update documentation and project guidance

Update:

- `README.md`
- crate-level docs in `src/lib.rs`
- `Document` examples and links in `src/document.rs`
- public API docs in `src/article.rs` and `src/extractor.rs`
- `AGENTS.md`

Documentation must state:

- `HtmlArticle::content` is not sanitized;
- each extraction method creates only its requested format;
- callers need separate extraction runs for multiple public formats;
- `text_char_count` is the normalized text count;
- the retry threshold causes less-filtered retries and is not a strict minimum;
- the readability check remains a heuristic.

Update the architecture and performance notes in `AGENTS.md` to remove `ArticleTree` guidance and to require direct final-DOM rendering.

## Benchmark plan

Replace `bench_output_formats` and remove `bench_render_only`, because returned articles no longer retain a renderable tree.

Add a Criterion group named `output_formats` with these cases for `medium-2`, `wikipedia-2`, and `large-retained-article`:

- `extract_html`
- `extract_markdown`
- `extract_text`
- `extract_all_formats_separately` (three public extraction calls)
- `legacy_extract_all_formats_once` (deprecated `parse`, one extraction and three direct renders)

Use `black_box` on input and returned content. Set byte throughput per fixture. Keep `parse_retries/medium-2` unchanged to detect retry-storage regressions.

Compare with the saved baseline:

```bash
cargo bench --bench readability -- --baseline tree-backed
```

Interpret results correctly:

- compare old extract-plus-one-render cases with the matching new single-format methods;
- compare old one-extraction/all-render case with the legacy one-pass direct-render case;
- report the separate three-extraction public case as the explicit cost of requesting all formats through the single-format API;
- do not claim lower allocations or peak memory without allocator or heap-profile measurements.

## Test plan

Add public API tests for:

1. all result accessors and `into_content`;
2. metadata equality across HTML, Markdown, and text extraction;
3. URL resolution in HTML and Markdown URL variants;
4. `MarkdownOptions` links, images, heading style, and bullet marker;
5. `TextOptions` block separators and preserved line breaks;
6. `Document` variants after `is_probably_readable` borrows the document;
7. input-byte and element limits through every input path;
8. invalid legacy string URLs and typed URL behavior;
9. empty, head-only, and image-only documents returning `Error::NoContent`;
10. `HtmlArticle`, `MarkdownArticle`, and `TextArticle` being `Send + Sync`;
11. exact compatibility of deprecated `parse` output;
12. deeply nested content rendering without stack overflow.

Keep Mozilla fixture tests on deprecated `parse` until they verify all legacy fields. Add representative fixture tests for each new format API so the primary API does not rely only on legacy coverage.

## Validation commands

Run all commands after the refactor:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps
cargo bench --bench readability -- --baseline tree-backed
```

Also run `prettier -w .` if the repository formatter includes the changed Markdown files, then run `cargo fmt --check` again.

## Acceptance criteria

- Normal HTML, Markdown, and text extraction never calls `ArticleTree::freeze` and retains no DOM in the public result.
- A single-format method parses, extracts, and renders exactly once.
- The DOM drops before the public result is returned.
- Default outputs and option behavior match the current implementation.
- Deprecated `parse` performs one parse and one extraction, then renders all three formats directly.
- Empty-content and retry behavior remain unchanged.
- All traversal of untrusted document depth remains iterative.
- The full Mozilla suite, formatting, Clippy, and docs pass.
- Criterion results are recorded against the tree-backed baseline.
- `README.md`, Rust API docs, and `AGENTS.md` describe the new explicit result API and direct-DOM architecture.
