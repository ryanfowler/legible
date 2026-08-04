# AGENTS.md

This file provides guidance to AI agents when working with code in this repository.
Keep this file updated after making changes.

## Project Overview

Legible is a Rust port of Mozilla's Readability.js - an algorithm for extracting clean, readable article content from web pages by removing navigation, ads, footers, and other non-content elements.

## Build & Test Commands

```bash
cargo build            # Build the library
cargo test             # Run all tests (includes Mozilla's readability test suite)
cargo test test_name   # Run a specific test (test names are sanitized from test-pages directory names)
cargo fmt              # Format code - run after making changes
cargo clippy           # Run linter - address all warnings after making changes
cargo doc --open       # Generate and view documentation
prettier -w .          # Format other files
```

## Architecture

The extraction pipeline flows through these stages:

1. **Document Parsing** - HTML parsed by `html5ever` into the internal stable-ID arena DOM
2. **Preparation** (`cleaning.rs`) - Script removal, BR/font normalization, lazy image fixing
3. **Metadata Extraction** (`metadata.rs`) - Title, byline, excerpt from JSON-LD, OpenGraph, meta tags
4. **Content Extraction** (`readability.rs`) - Main algorithm in `grab_article()`
5. **Content Cleaning** (`cleaning.rs`) - Conditional removal of low-scoring elements

### Key Modules

- **`document.rs`** - Public fallible `Document<'a>` parser for checking readability before extraction
- **`extractor.rs`** - Reusable extraction configuration and the primary extraction entry points
- **`article.rs` / `article_tree.rs`** - Private-field public result API and compact immutable `Send + Sync` output tree with direct HTML, text, and Markdown rendering
- **`readability.rs`** - Core algorithm: candidate selection, scoring, content consolidation
- **`readerable.rs`** - Quick heuristic check for whether a document is likely parseable; exposes `pub(crate) is_probably_readerable_doc` for use by `Document`
- **`scoring.rs`** - Node scoring by tag type, class/id weight, link density, and bottom-up cached text statistics
- **`cleaning.rs`** - DOM preparation and cleanup functions
- **`metadata.rs`** - Multi-source metadata extraction (JSON-LD, meta tags, heuristics)
- **`markdown.rs`** - Iterative, direct DOM-to-CommonMark serialization of cleaned article content
- **`constants.rs`** - Static regex patterns (via `once_cell::Lazy`) and configuration flags
- **`src/dom/`** - Compact arena storage, typed tags and attributes, iterative traversal, centralized mutation, fragment parsing, and `html5ever` serialization
- **`dom/state.rs`** - Dense Readability state indexed by stable `NodeId` values

### Performance Notes

- Use `Dom`'s direct `NodeId` traversal and typed query helpers. Do not add a general CSS matcher.
- Keep post-parse DOM access free of `RefCell`; parser-only interior mutability belongs in `dom/parse.rs`.
- Use borrowed attribute values for hot reads and `Tag`/`AttrName` for common predicates. Keep parser tag and attribute classification allocation-free for html5ever's normalized lowercase names.
- Collect attached preorder snapshots before mutation when tree order matters. Arena allocation order can differ from DOM order after HTML tree repair. Use element-only snapshots with depth when a pass processes only elements and can skip removed subtrees.
- Use `SmallVec` for hot, short-lived traversal stacks, scoring candidates, metadata tables, and small child snapshots. Keep full-document snapshots in `Vec`.
- Keep structural mutation in `dom/mutation.rs` and validate links in debug builds.
- Preserve the O(1) leaf fast path in DOM cycle checks. The parser appends new leaf nodes, so do not add another depth-dependent scan to this path.
- Use the `deeply_nested_document` Criterion benchmark for parser-scaling changes. `html5ever` currently scans its open-element stack for each nested `<div>`, so this adversarial case is quadratic upstream.
- Use the `dom_parse` Criterion group to isolate custom DOM construction from readability extraction.
- Keep the byte-wise ASCII fast path in text-statistics scans. Use the Unicode path for non-ASCII text.
- Use the dense `NodeStateStore` for scores, score-scan deduplication, table state, and cached text statistics.
- Use iterative traversal for untrusted HTML depth.
- Use the Criterion fixtures in `benches/readability.rs` for changes to parsing or extraction. Use `parse_retries/medium-2` for retry-storage changes, and preserve output compatibility with the Mozilla fixture suite.
- Keep extraction structural. Do not serialize DOM content for internal inspection or mutation. Serialize only the final selected article for `Article::content`.
- Generate legacy Markdown directly from the final cleaned DOM. Render the immutable `Article` directly from `ArticleTree` through the shared read-only Markdown traversal interface. Do not rebuild a temporary DOM. Keep Markdown traversal iterative and escape text, link destinations, and code fences for CommonMark.
- Preserve Markdown's byte-wise ASCII text path, compact task fields, and output capacity hint from normalized article text. These avoid per-character work, excess task-stack traffic, and repeated output growth.
- Keep only the best below-threshold retry as a compact frozen DOM subtree. Compare attempts with allocation-free normalized character counts.

### Scoring System

Initial scores by tag: DIV +5, PRE/TD/BLOCKQUOTE +3, H1-H6/TH -5, ADDRESS/OL/UL/DL/FORM -3. Class/ID patterns matching positive/negative regexes add ±25.

### Algorithm Flags

- `FLAG_STRIP_UNLIKELYS` (0x1) - Remove non-content-like elements
- `FLAG_WEIGHT_CLASSES` (0x2) - Score based on class/id patterns
- `FLAG_CLEAN_CONDITIONALLY` (0x4) - Conditional cleanup pass

The algorithm retries with progressively fewer flags if initial extraction fails.

## Documentation

- Keep `README.md` and the public Rust API docs consistent.
- Write user documentation in ASD-STE100 Simplified Technical English. Use short sentences, active voice, and consistent terms.
- State that `Article::content` is not sanitized. Do not describe cleaned HTML as safe HTML.
- State that `char_threshold` causes less-filtered retries and is not a strict output minimum.
- State that the quick readability check is a heuristic and can return false positives or false negatives.

## Testing

The custom DOM uses safe Rust only. Run `cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets --all-features -- -D warnings` after DOM changes.

Tests run against Mozilla's official Readability.js test suite (git submodule at `tests/readability-js/`). The `build.rs` script auto-generates test functions from `tests/readability-js/test/test-pages/` directories. Each test directory contains `source.html`, `expected.html`, and `expected-metadata.json`.

Extraction with default options must return `Error::NoContent` when the best retry has no text. This includes empty, head-only, and image-only documents.

## Public API

```rust
use legible::{extract, Document, Extractor};

let article = extract(html)?;
let markdown = article.to_markdown();

let document = Document::parse(html)?;
if document.is_probably_readable() {
    let article = Extractor::default().extract_document(document)?;
}
```

The deprecated `parse` adapter returns `legacy::Article` with the 0.4 public string fields.
