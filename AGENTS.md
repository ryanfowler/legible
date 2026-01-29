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
```

## Architecture

The extraction pipeline flows through these stages:

1. **Document Parsing** - HTML parsed via `dom_query` crate
2. **Preparation** (`cleaning.rs`) - Script removal, BR/font normalization, lazy image fixing
3. **Metadata Extraction** (`metadata.rs`) - Title, byline, excerpt from JSON-LD, OpenGraph, meta tags
4. **Content Extraction** (`readability.rs`) - Main algorithm in `grab_article()`
5. **Content Cleaning** (`cleaning.rs`) - Conditional removal of low-scoring elements

### Key Modules

- **`readability.rs`** - Core algorithm: candidate selection, scoring, content consolidation
- **`scoring.rs`** - Node scoring by tag type, class/id weight, link density, text density
- **`cleaning.rs`** - DOM preparation and cleanup functions
- **`metadata.rs`** - Multi-source metadata extraction (JSON-LD, meta tags, heuristics)
- **`constants.rs`** - Static regex patterns (via `once_cell::Lazy`) and configuration flags
- **`dom/node.rs`** - `NodeDataStore` pattern for attaching score data to nodes (workaround for Rust's lack of arbitrary node data attachment like JS)

### Scoring System

Initial scores by tag: DIV +5, PRE/TD/BLOCKQUOTE +3, H1-H6/TH -5, ADDRESS/OL/UL/DL/FORM -3. Class/ID patterns matching positive/negative regexes add ±25.

### Algorithm Flags

- `FLAG_STRIP_UNLIKELYS` (0x1) - Remove non-content-like elements
- `FLAG_WEIGHT_CLASSES` (0x2) - Score based on class/id patterns
- `FLAG_CLEAN_CONDITIONALLY` (0x4) - Conditional cleanup pass

The algorithm retries with progressively fewer flags if initial extraction fails.

## Testing

Tests run against Mozilla's official Readability.js test suite (git submodule at `tests/readability-js/`). The `build.rs` script auto-generates test functions from `tests/readability-js/test/test-pages/` directories. Each test directory contains `source.html`, `expected.html`, and `expected-metadata.json`.

## Public API

```rust
use legible::{parse, Options, is_probably_readerable};

// Full extraction
let article = parse(html, Some("https://example.com"), None)?;  // Returns Article with title, content, text_content, byline, excerpt, etc.

// Quick check without full parsing
if is_probably_readerable(html, None) { /* ... */ }
```