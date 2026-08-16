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
cargo bench --bench smoke      # Run the quick performance smoke benchmarks
cargo bench --bench extraction # Run the full compatibility performance suite
cargo +nightly fuzz run <target> # Run a fuzz target (requires nightly + cargo-fuzz)
prettier -w .          # Format other files
node scripts/compare-extractors/performance.mjs  # Compare extractors
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
4. **Specialized Recognition** (`specialized/`) - High-confidence listing and discussion extraction
5. **Content Extraction** (`extraction.rs`) - Strategy retries, candidate selection, and content consolidation
6. **Content Cleaning** (`cleaning.rs`) - Hard cleanup, contextual boilerplate cleanup, and multi-signal heuristic cleanup
7. **Semantic Compilation** (`document/`) - Direct source recognition and compilation into the semantic document

### Key Modules

| Module | Role |
|---|---|
| `extractor.rs` | Public builder, extraction config |
| `page.rs` | `ExtractedPage` with lazy HTML/MD/text serialization |
| `candidate.rs` | Internal candidate model and balanced structural root-boundary selection |
| `extraction.rs` | Strategy retries, candidate selection, content consolidation |
| `scoring.rs` | General candidate features, ranking, and cached text statistics |
| `cleaning.rs` | Pre-extraction preparation and conservative structural and textual relevance cleanup |
| `normalize.rs` / `normalize/` | Source preparation and relevance cleanup for SVG charts, media, duplicate images, and heading artifacts |
| `normalize/svg.rs` | Namespace-aware SVG implementation cleanup and accessible chart conversion |
| `document/lists.rs` / `document/tables.rs` | Direct semantic list recognition, table classification, listing conversion, and layout-table flattening |
| `document/footnotes.rs` / `document/math.rs` / `document/callouts.rs` | Direct semantic footnote, math, and callout recognition for the compiler |
| `document/facts.rs` | Shared complex-source feature inventory, cleanup-collected semantic gates, sparse source evidence, propagated semantic facts, and cleanup-safe source facts |
| `document/ordinary.rs` | Conservative feature routing and stack-safe streaming compilation for ordinary HTML semantics |
| `quality.rs` | Source-relative DOM metrics, Document-native result metrics, diagnostics-only semantic coverage, access-barrier and short-result checks, and best-attempt scoring |
| `diagnostics.rs` | Opt-in strategy, cleanup, normalization, semantic coverage, and specialized extractor diagnostics |
| `document/` | Private compact semantic event tape plus internal retained-source compiler, direct semantic recognition, validation, and stable test debug output; production pages retain this representation instead of a DOM |
| `metadata.rs` | Structured-data parsing and multi-source metadata resolution |
| `page_kind.rs` | Internal page categories that control cleanup policy, including job-profile boundaries |
| `specialized/` | Internal registry and extractors for non-article page structures |
| `specialized/discussion.rs` | Shared canonical HTML builder for primary posts, reply metadata, and nested discussions |
| `specialized/ai_conversation.rs` | Static shared AI conversation adapter |
| `specialized/discourse.rs` / `specialized/reddit.rs` | Static Discourse and old-Reddit discussion adapters |
| `render/markdown.rs` / `render/text.rs` / `render/html.rs` | Stack-safe format renderers from the semantic document |
| `benches/support/compact_ir.rs` | Benchmark-only adapters for compact preorder and event-tape representation experiments |
| `constants.rs` | Regex patterns, config flags, matching helpers |
| `dom/` | Arena storage, typed tags/attributes, traversal, mutation |
| `dom/state.rs` | Dense scoring state indexed by `NodeId` |

### Design Rules

These invariants are costly to violate:

- **No CSS matcher.** Use `Dom`'s direct `NodeId` traversal and typed query helpers.
- **No `RefCell` after parse.** Parser-only interior mutability stays in `dom/parse.rs`.
- **Snapshot before mutation.** Collect preorder snapshots when tree order matters. Arena allocation order can differ from DOM order after HTML tree repair. Use element-only snapshots when a pass skips text nodes and removed subtrees.
- **Keep extraction structural.** Do not serialize the DOM for internal inspection. Render only the final requested format.
- **Lazy rendering.** `ExtractedPage` owns only the private semantic representation, not a retained DOM. Render HTML, Markdown, and text lazily from that representation. Derive result metrics from cached internal stats; defer that measurement until a text or metric method needs it. The public `extract` function must not eagerly render output.
- **Iterative traversal** for untrusted HTML depth.
- **Preparation order:** collect metadata first. Then reveal noscript images, remove non-math scripts and styles, normalise body BR runs, and rename font elements. One linear traversal per stage. Keep math source until semantic compilation.
- **Non-destructive discovery.** Candidate discovery and scoring must not mutate the source DOM. Defer candidate removals until scoring is complete.
- **Copy before cleanup.** Copy the selected region into a compact fragment. Run content cleanup only on that fragment.
- **Consume the winning fragment.** Move an accepted or deferred winning fragment into semantic compilation. Do not deep-copy the final cleaned DOM before compilation.
- **Keep final DOM mutation focused on relevance.** Cleanup decides what to remove. The semantic compiler resolves URLs, drops source attributes, ignores comments, collapses transparent wrappers, and emits output semantics.
- **Use static image evidence together.** Small dimensions are a signal. Protect described images, math, responsive sources, and captioned figures.
- **Preserve table content models.** Synthetic extraction boundaries must keep valid table, section, row, and cell ancestry. Normalize conservative rank-based listing tables into lists, but keep real data tables.
- **Use multiple clutter signals.** Do not remove substantial content from one weak class, ID, role, length, or link-density signal. Breadcrumb, subscription, related-content, and document-chrome cleanup must also use structure and document position. Preserve article-contained regions, pricing content, meaningful media, and identity text.
- **Compile code directly.** The semantic compiler recognizes source code, language hints, line wrappers, and explicit syntax-highlighter gutters without renderer-oriented DOM rewrites. Preserve numeric source code and source-line wrappers.
- **Compile lists and tables directly.** Keep scoring-time table analysis and ARIA list preparation separate. The semantic compiler emits ordered-list metadata, converts rank-based listings, flattens layout tables, and preserves data-table cells without renderer-oriented DOM rewrites.
- **Borrow, don't clone.** Borrow `ExtractorConfig` during extraction. Borrow a JSON-LD script's single text child and allocate a fallback only when the subtree is complex.
- **Canonicalize discussions once.** Specialized discussion extractors must use the shared builder for primary posts, reply metadata, rich reply bodies, and retained nesting.
- **Compile output semantics directly.** Footnote, math, and callout source recognition belongs in `document/`. Keep only source protection and external footnote adoption before cleanup.
- **Route ordinary semantics conservatively.** Use the streaming compiler for supported native HTML only when a cheap source gate finds no inferred semantic dialect. The gate may count source nodes for builder capacity but must not propagate semantic visibility facts. The ordinary compiler validates structural details while it lowers and returns to the complex compiler when required. Do not add a separate ordinary inventory pass.
- **Share complex semantic facts.** Build feature worklists once. Keep broadly useful node facts compact and dense. Keep feature-specific analysis sparse. Reuse cleanup-safe source facts in final cleanup and semantic compilation. Update derived facts after detach operations.
- **Cache semantic source evidence.** Collect the tiny source gate during an existing cleanup traversal. Keep feature-specific callout, footnote, math, accessible-math, and data-table evidence as sparse node sets only when the gate finds that feature. Retain sparse feature candidates so complex lowering does not repeat broad gate classification. Resolve fragment references against only their target IDs. Pass the cached evidence through hard cleanup, heuristic cleanup, final cleanup, and semantic compilation. Use cheap tag and attribute gates before rich recognizers. Do not add a standalone whole-fragment pass solely to build the gate.
- **Defer rejected-attempt compilation.** Normal extraction uses cheap DOM quality metrics until a candidate can win. Diagnostics may compile every attempt to report semantic metrics.
- **Use dense semantic indexes.** Renderers should use node-indexed storage for per-document state instead of hash maps when semantic node IDs are dense.
- **Keep the semantic representation compact and private.** Production pages retain an immutable event tape with 8-byte operation headers and type-specific payload tables. Do not add general-purpose tree links to the retained representation. The benchmark prototype remains useful for later renderer comparisons.
- **Use one canonical text arena.** Store semantic prose and inline-code text in one document-owned UTF-8 buffer with `TextRef` ranges. Do not add one owned heap string per semantic text leaf. Keep raw block-code payloads separate until measurements justify moving them.
- **Reuse across retries.** Restore the prepared source DOM without parsing HTML again. Reuse source-only candidate, visibility, and title indexes across extraction retries. Keep the cleaning node snapshot and text buffers alive across retries and sequential mutation passes.

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
- State that `ExtractedPage::html()` is canonical semantic HTML. `safe_html()` is a compatibility alias for the same output.
- Write all documentation and explanatory text in ASD-STE100 Simplified Technical English. Use short sentences, active voice, and consistent terms.

## Testing

`tests/general/` is the authoritative Markdown fixture suite. Each fixture has `source.html` and `expected.md`. It can also have `metadata.json`. Use `expected.error` for a fixture that must fail with a named public error variant. Set `LEGIBLE_UPDATE_FIXTURES=1` when you intentionally update snapshots. `tests/defuddle/` contains exact Markdown compatibility fixtures. Each fixture has `source.html` and `expected.md`. A fixture can also have `metadata.json`. The top-level fixture directory identifies the likely owning pipeline layer. `tests/web/` contains capability fixtures with semantic assertions in `expected.json`. Add positive and negative capability cases without making every output an exact snapshot. Mozilla's Readability.js suite in `tests/readability-js/` remains an article regression suite.

`benchmarks/quality/` contains independent extraction-quality fixtures. Install dependencies with `npm --prefix scripts/compare-extractors ci` and `cargo fetch`. Run all quality fixtures with `node scripts/compare-extractors/index.mjs --all`, or use `--fixture <id>` for one case. The runner invokes Cargo offline. Third-party extractor output is comparison data, not ground truth.

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
let markdown = page.markdown();
```

`ExtractedPage` owns a private semantic representation. It provides lazy HTML, Markdown, and text methods, scalar metrics, and page metadata. Extraction diagnostics are opt-in through `ExtractorBuilder::diagnostics` and are not retained by default.

## Fuzzing

Cargo-fuzz targets are in `fuzz/fuzz_targets/`. They cover public extraction, DOM mutation and serialization, Markdown and text rendering, JSON-LD metadata, URL rewriting, and deeply nested malformed HTML. Run them with `cargo +nightly fuzz run <target>`.

The internal `fuzzing` feature exposes semantic document validation only to fuzz targets. Standalone DOM fuzz targets validate their own DOM values. Do not use this feature in normal applications.

## Performance

Use `cargo bench --bench smoke` for the normal development performance check. It
covers medium and large article extraction, end-to-end Markdown extraction, and
lazy Markdown rendering. It uses short Criterion measurement windows. Use it to
catch large regressions, not to make final performance claims.

Do not run the full benchmark suite for every change. Run
`cargo bench --bench extraction` for performance-sensitive work, baseline
updates, or when a change affects a workload that the smoke set does not cover.
Use the full suite's focused Criterion filter when possible. The extraction suite
separates extraction, retained-fragment lowering, end-to-end Markdown, and lazy
Markdown/HTML/text rendering. See `benches/README.md` for workload coverage,
baseline commands, and regression guardrails. The baseline at the private-IR
migration starting revision is recorded in `benches/private-ir-baseline.md`.

`benches/extraction.rs` covers generated compatibility workloads, large real fixtures, lazy renderers, and deeply nested parser input.

Keep malformed nested-table handling linear. Use bounded text scans for heading and clutter classifiers. Do not repeat full subtree scans for nested tables or protected-content checks. Keep ASCII normalized-text, cached candidate statistics, and Markdown ordinary-text paths bulk-oriented, with Unicode and syntax-sensitive fallbacks. The semantic compiler skips multiline and media separator passes when their source evidence is absent. Use the end-to-end `extract_markdown` benchmark when changing the raw-HTML-to-Markdown path.
