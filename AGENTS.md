# AGENTS.md

Guidance for AI agents editing this repository. Keep this file updated after changes.

## Build & Test Commands

```bash
cargo build            # Build the library
cargo test             # Run all tests (includes Mozilla's readability test suite)
cargo test test_name   # Run a specific test (test names are sanitized from test-pages directory names)
cargo fmt              # Format code - run after making changes
cargo clippy           # Run linter - address all warnings after making changes
cargo doc --open       # Generate and view documentation
cargo +nightly fuzz run <target> # Run a fuzz target (requires nightly + cargo-fuzz)
prettier -w .          # Format other files
```

## Design Philosophy

Apply the principles from *A Philosophy of Software Design* by John K. Ousterhout:

- **Minimize complexity.** Prefer deep modules with simple interfaces.
- **Hide implementation details.** Information hiding reduces the cost of change.
- **Avoid shallow abstractions.** A module whose interface is not much simpler than its implementation is a net negative.
- **Avoid pass-through layers.** If a method does little more than delegate to another, eliminate it.
- **Keep related knowledge together.** Do not split a design concern across unrelated modules.
- **Define errors out of existence.** Design APIs that make invalid states unrepresentable.

## Architecture

The extraction pipeline flows through these stages:

1. **Document Parsing** - HTML parsed by `html5ever` into the internal stable-ID arena DOM
2. **Preparation** (`cleaning.rs`) - Script removal, BR/font normalization, lazy image fixing
3. **Metadata Extraction** (`metadata.rs`) - Candidate resolution from JSON-LD, OpenGraph, meta tags, and HTML elements
4. **Content Extraction** (`extraction.rs`) - Strategy retries, candidate selection, and content consolidation
5. **Content Cleaning** (`cleaning.rs`) - Hard cleanup and multi-signal heuristic cleanup
6. **Semantic Normalization** (`normalize.rs`) - Image, code, figure, footnote, table, and wrapper normalization

### Key Modules

| Module | Role |
|---|---|
| `extractor.rs` | Public builder, extraction config |
| `page.rs` | `ExtractedPage` with lazy HTML/MD/text serialization |
| `candidate.rs` | Internal candidate model and balanced structural root-boundary selection |
| `extraction.rs` | Strategy retries, candidate selection, content consolidation |
| `scoring.rs` | General candidate features, ranking, and cached text statistics |
| `cleaning.rs` | Pre-extraction preparation and conservative relevance cleanup |
| `normalize.rs` / `normalize/` | Ordered semantic passes for images, headings, lists, code, figures, footnotes, tables, and wrappers |
| `quality.rs` | Source-relative quality, access-barrier and short-result checks, and best-attempt scoring |
| `metadata.rs` | Structured-data parsing and multi-source metadata resolution |
| `markdown.rs` / `text.rs` | Format renderers from cleaned DOM |
| `constants.rs` | Regex patterns, config flags, matching helpers |
| `dom/` | Arena storage, typed tags/attributes, traversal, mutation |
| `dom/state.rs` | Dense scoring state indexed by `NodeId` |

### Design Rules

These invariants are costly to violate:

- **No CSS matcher.** Use `Dom`'s direct `NodeId` traversal and typed query helpers.
- **No `RefCell` after parse.** Parser-only interior mutability stays in `dom/parse.rs`.
- **Snapshot before mutation.** Collect preorder snapshots when tree order matters. Arena allocation order can differ from DOM order after HTML tree repair. Use element-only snapshots when a pass skips text nodes and removed subtrees.
- **Keep extraction structural.** Do not serialize the DOM for internal inspection. Render only the final requested format.
- **Lazy rendering.** `ExtractedPage` owns the cleaned DOM. Render HTML, Markdown, and text lazily. The public `extract` function must not eagerly render output.
- **Iterative traversal** for untrusted HTML depth.
- **Preparation order:** collect metadata first. Then reveal noscript images, remove scripts and styles, normalise body BR runs, and rename font elements. One linear traversal per stage.
- **Non-destructive discovery.** Candidate discovery and scoring must not mutate the source DOM. Defer candidate removals until scoring is complete.
- **Copy before cleanup.** Copy the selected region into a compact fragment. Run content cleanup only on that fragment.
- **Separate cleanup and normalization.** Cleanup decides what to remove. Normalization makes retained markup predictable for serializers.
- **Preserve table content models.** Synthetic extraction boundaries must keep valid table, section, row, and cell ancestry. Normalize conservative rank-based listing tables into lists, but keep real data tables.
- **Use multiple clutter signals.** Do not remove substantial content from one weak class, ID, role, length, or link-density signal.
- **Borrow, don't clone.** Borrow `ExtractorConfig` during extraction. Borrow a JSON-LD script's single text child and allocate a fallback only when the subtree is complex.
- **Reuse across retries.** Restore the prepared source DOM without parsing HTML again. Keep the cleaning node snapshot and text buffers alive across extraction retries and sequential mutation passes.

### Common Pitfalls

- Do not add dependencies on `ego-tree`, `scraper`, or other DOM crates. The custom arena DOM is intentional.
- Mutation belongs in `dom/mutation.rs`. External modules must use the public traversal and query APIs.
- `scoring.rs` owns all text statistics. Do not duplicate text scanning in other modules.
- Use the `Error` enum from `error.rs` for fallible paths. Do not panic.

### Scoring System

Candidate ranking combines Readability propagation with text, structure, link-density, and class/ID features. It gives extra weight to code, data tables, and meaningful link lists. A separate structural pass selects a precise child, a common semantic parent, or a close schema-text match. Readability initial scores remain: DIV +5, PRE/TD/BLOCKQUOTE +3, H1-H6/TH -5, and ADDRESS/OL/UL/DL/FORM -3. Class/ID patterns matching positive/negative regexes add ±25 to the Readability feature.

Unlikely class, ID, and role values are negative ranking evidence. They do not remove candidates during discovery. The algorithm retries with normal, relaxed-cleanup, broad-content, structured-data, and body-fallback strategies when extraction quality is weak.

## Documentation

- Keep `README.md` and the public Rust API docs consistent.
- State that `ExtractedPage::html()` is not sanitized. Do not describe cleaned HTML as safe HTML.
- Write all documentation and explanatory text in ASD-STE100 Simplified Technical English. Use short sentences, active voice, and consistent terms.

## Testing

`tests/general/` is the authoritative Markdown fixture suite. Each fixture has `source.html` and `expected.md`. It can also have `metadata.json`. Use `expected.error` for a fixture that must fail with a named public error variant. Set `LEGIBLE_UPDATE_FIXTURES=1` when you intentionally update snapshots. `tests/web/` contains capability fixtures with semantic assertions in `expected.json`. Add positive and negative capability cases without making every output an exact snapshot. Mozilla's Readability.js suite in `tests/readability-js/` remains an article regression suite. Use `node scripts/compare-defuddle/index.mjs` for optional local differential artifacts.

Default extraction must return `Error::NoContent` for empty, head-only, and image-only documents.

After logic changes:
1. `cargo fmt --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test`
4. Verify output compatibility with the Mozilla fixture suite.

The custom DOM uses safe Rust only. Run the full suite above after DOM changes.

## Public API

```rust
use legible::{Extractor, extract};

let page = extract(html, None)?;

let extractor = Extractor::builder().structured_data(true).build();
let page = extractor.extract(html, None)?;
```

`ExtractedPage` owns the cleaned extraction DOM. It provides lazy HTML, Markdown, and text methods plus page metadata. Extraction diagnostics are opt-in through `ExtractorBuilder::diagnostics` and are not retained by default.

## Fuzzing

Cargo-fuzz targets are in `fuzz/fuzz_targets/`. They cover public extraction, DOM mutation and serialization, Markdown and text rendering, JSON-LD metadata, URL rewriting, and deeply nested malformed HTML. Run them with `cargo +nightly fuzz run <target>`.
