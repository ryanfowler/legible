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
cargo +nightly fuzz run <target> # Run a fuzz target (requires nightly + cargo-fuzz)
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

- **`document.rs`** - Public `Document<'a>` wrapper for checking readability before extraction
- **`readability.rs`** - Core algorithm: candidate selection, scoring, content consolidation
- **`readerable.rs`** - Quick heuristic check for whether a document is likely parseable; exposes `pub(crate) is_probably_readerable_doc` for use by `Document`
- **`scoring.rs`** - Node scoring by tag type, class/id weight, link density, and bottom-up cached text statistics
- **`cleaning.rs`** - DOM preparation and cleanup functions
- **`metadata.rs`** - Multi-source metadata extraction (JSON-LD, meta tags, heuristics)
- **`markdown.rs` / `text.rs`** - Iterative direct rendering of cleaned article content
- **`constants.rs`** - Static regex patterns, specialized matching helpers, and configuration flags
- **`src/dom/`** - Compact arena storage, typed tags and attributes, iterative traversal, centralized mutation, fragment parsing, and `html5ever` serialization
- **`dom/state.rs`** - Dense Readability state indexed by stable `NodeId` values

### Performance Notes

- Use `Dom`'s direct `NodeId` traversal and typed query helpers. Do not add a general CSS matcher.
- Keep post-parse DOM access free of `RefCell`; parser-only interior mutability belongs in `dom/parse.rs`.
- Retain html5ever's attribute vectors when the parser creates elements. Do not rebuild them only to cache attribute classifications.
- Use borrowed attribute values for hot reads and `Tag`/`AttrName` for common predicates. Keep parser tag and attribute classification allocation-free for html5ever's normalized lowercase names.
- Collect attached preorder snapshots before mutation when tree order matters. Arena allocation order can differ from DOM order after HTML tree repair. Use element-only snapshots with depth when a pass processes only elements and can skip removed subtrees.
- Reuse the cleaning node snapshot and text buffers across extraction retries and sequential mutation passes. Keep URI repair, class cleanup, and comment removal in one post-processing snapshot.
- Preserve preparation order: remove scripts and styles, normalize body BR runs, then rename font elements. Use one linear traversal for each stage. Do not add per-target ancestor scans. Keep unusable-image and noscript-image discovery in one traversal.
- Borrow a JSON-LD script's single text child. Allocate a fallback buffer only when the script has a more complex subtree.
- Use `SmallVec` for hot, short-lived traversal stacks, scoring candidates, metadata tables, and small child snapshots. Keep full-document snapshots in `Vec`.
- Keep structural mutation in `dom/mutation.rs` and validate links in debug builds.
- Preserve the O(1) leaf fast path in DOM cycle checks. The parser appends new leaf nodes, so do not add another depth-dependent scan to this path.
- Keep the bounded, markup-density-aware node capacity hint. Count markup with `memchr` so preallocation does not add a full scalar scan or overallocate for dense adversarial input.
- Preallocate the element-and-depth mutation snapshot from half of the arena length. This avoids repeated growth on normal mixed element/text trees and limits over-allocation on markup-only trees.
- Use the `deeply_nested_document` Criterion benchmark for parser-scaling changes. `html5ever` currently scans its open-element stack for each nested `<div>`, so this adversarial case is quadratic upstream.
- Use the `dom_parse` Criterion group to isolate custom DOM construction.
- Keep the byte-wise ASCII fast path in text-statistics scans. Use the Unicode path for non-ASCII text.
- Keep byte-wise ASCII paths in normalized character counts and readerable text-length scans. These paths avoid UTF-8 decoding on common article text while preserving Unicode behavior.
- Keep quick readerability state dense by NodeId. Compute trimmed subtree text lengths bottom-up and propagate list-item exclusions once. Keep adversarial no-early-return Criterion cases for nested candidates and repeated BR parents.
- Keep weighted descendant link length in cached text statistics. Candidate link-density reads must stay O(1).
- Keep cached text and comma counts as saturating `u32` values. Scan each text node with native `usize` counters, then clamp it before storage. This keeps the dense cache compact without adding overflow checks to the byte-wise hot loop.
- Use the dense `NodeStateStore` for scores, score-scan deduplication, table state, and cached text statistics.
- Use iterative traversal for untrusted HTML depth.
- Use the Criterion fixtures in `benches/readability.rs` for changes to parsing or extraction. Use `parse_retries/medium-2` for retry-storage changes, and preserve output compatibility with the Mozilla fixture suite.
- Keep extraction structural. Do not serialize DOM content for internal inspection or mutation. Render only the requested final format from the cleaned DOM.
- Render HTML, Markdown, and text directly from the final cleaned DOM. Do not freeze an intermediate output tree or rebuild a temporary DOM. Drop the DOM before returning the public result.
- Keep final HTML rendering on the direct iterative serializer. Escape text and attributes in byte runs. Do not route final output through html5ever's character-at-a-time serializer.
- The public `parse` function must render all formats from one cleaned DOM. Keep final rendering iterative. Match html5ever's HTML escaping and namespace rules. Escape Markdown text, link destinations, and code fences for CommonMark.
- Preserve the byte-wise ASCII paths in Markdown and normalized article text, compact task fields, the preallocated heap-backed Markdown task stack, and output capacity hints from normalized article text. Keep code span and code block rendering free of temporary text and fence allocations. These avoid per-character work, excess task-stack traffic, stack-resident task buffers on complex articles, and repeated output growth.
- Use typed `AttrName` lookups for hot Markdown link and image attributes. Keep local-name lookups only for attributes without a known enum variant.
- Keep only the best below-threshold retry as a compact frozen DOM subtree. Compare attempts with allocation-free normalized character counts.
- Borrow `Options` during extraction. Keep the owned options alive at the public API boundary.

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

Tests run against Mozilla's official Readability.js test suite (git submodule at `tests/readability-js/`). The integration harness compares canonical HTML structure and ordered text, every metadata field, and the fixture's readerable value. Each test directory contains `source.html`, `expected.html`, and `expected-metadata.json`.

Extraction with default options must return `Error::NoContent` when the best retry has no text. This includes empty, head-only, and image-only documents.

## Public API

```rust
use legible::{Document, parse};

let article = parse(html, None, None)?;

let document = Document::new(html);
if document.is_probably_readerable(None) {
    let article = document.parse(None, None)?;
}
```

`Article` contains public HTML, Markdown, text, and metadata fields.

## Fuzzing

Cargo-fuzz targets are in `fuzz/fuzz_targets/`. They cover public document parsing,
DOM mutation and serialization, Markdown and text rendering, JSON-LD metadata, URL
rewriting, and deeply nested malformed HTML. Run them with `cargo +nightly fuzz run <target>`.
