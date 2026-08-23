# Semantic IR migration baseline

This note records the historical behavior baseline and the current status of the semantic document migration.

## Revision

The migration baseline is `main` at `a97e49c460a0ef0e8e4ed271ef2b3f417c7b6184`.

## Historical pre-IR output contracts

At the recorded pre-IR baseline, `ExtractedPage` retained a compact cleaned DOM fragment and rendered each output on demand. This describes historical behavior only; the current page retains a private semantic representation.

- `markdown()` returned CommonMark and GFM output. It did not emit raw HTML. It filtered unsupported link and image URI schemes.
- `text()` returned normalized plain text. `text_length()` counted its Unicode scalar values. `word_count()` used the same normalized DOM walk.
- `html()` returned extracted markup from the retained DOM. It was not sanitized.
- `safe_html()` copied and sanitized the retained fragment before it rendered HTML.
- Repeated render calls were deterministic.

The raw `html()` shape was compatibility-sensitive. Unit tests required source attributes to remain in `html()` while `safe_html()` removed active content. The semantic renderer migration deliberately replaced this contract.

## Current migration status

`ExtractedPage` now retains only a private semantic representation. The final normalized compact DOM fragment is compiled once and then dropped. Markdown, normalized text, and canonical semantic HTML render lazily from that representation. `safe_html()` is a compatibility alias for canonical `html()` output.

## Quality baseline

Run the corpus from the repository root:

```bash
npm --prefix tools/extractor-eval ci
cargo fetch
node tools/extractor-eval/index.mjs --all
```

The 125-fixture report for the baseline revision is in `target/quality-comparison/report.json` after a local run. Keep the report as a CI or review artifact. Do not commit `target/`.

Rounded Legible results are:

| Metric | Result |
| --- | ---: |
| Content recall | 0.95 |
| Noise rejection | 0.91 |
| Structural fidelity | 0.91 |
| Metadata accuracy | 0.70 |
| Reference F1 | 0.98 |
| Reliability | 0.98 |

The checked-in historical comparison is `evals/quality/BASELINE.md`.

## Performance baseline

Use the same machine, Rust toolchain, power mode, and background workload for both runs.

```bash
cargo bench --bench pipeline -- --save-baseline pre-ir
cargo bench --bench pipeline -- --baseline pre-ir
```

Criterion stores the complete local baseline under `target/criterion`. Keep that directory as a local or CI artifact. The benchmark procedure and regression limits are in `benches/README.md`.

The recorded run used an Apple M2 Pro (`arm64`), macOS 26.6 build 25G72, and `rustc 1.97.1 (8bab26f4f 2026-07-14)`. Criterion reported these median estimates:

| Benchmark | Median |
| --- | ---: |
| `extract/small/prose` | 222.08 µs |
| `extract/medium/prose` | 2.4033 ms |
| `extract/large/prose` | 24.074 ms |
| `extract/large/reference` | 62.018 ms |
| `extract_markdown/small/prose` | 233.40 µs |
| `extract_markdown/medium/prose` | 2.5134 ms |
| `extract_markdown/large/prose` | 25.136 ms |
| `extract_markdown/large/malformed` | 125.19 ms |
| `lazy_output/short/markdown` | 9.3905 µs |
| `lazy_output/short/text` | 5.0989 µs |
| `lazy_output/short/html` | 4.6162 µs |
| `lazy_output/long/markdown` | 574.23 µs |
| `lazy_output/long/text` | 316.83 µs |
| `lazy_output/long/html` | 273.79 µs |
| `complex_pages/documentation` | 31.250 ms |
| `complex_pages/highlighted-code` | 26.829 ms |
| `complex_pages/math` | 26.092 ms |
| `complex_pages/table-heavy` | 26.163 ms |
| `large_compatibility_fixtures/guardian-article` | 10.906 ms |
| `large_compatibility_fixtures/wikipedia-reference` | 59.179 ms |
| `deeply_nested_document/1000` | 4.4753 ms |
| `deeply_nested_document/2000` | 17.139 ms |
| `deeply_nested_document/4000` | 67.025 ms |
| `deeply_nested_document/8000` | 264.90 ms |

Use the named Criterion baseline for the complete set and this table for a durable review reference.

## Initial semantic vocabulary

The initial IR vocabulary comes from existing normalization and rendering behavior.

| Semantic area | Existing implementation evidence | Fixture evidence |
| --- | --- | --- |
| Paragraphs, headings, quotes, emphasis, strong text, links, breaks, and thematic breaks | `src/render/markdown.rs`, `src/render/text.rs`, `src/normalize/headings.rs` | General, Defuddle, Web, and Readability suites |
| Ordered and unordered lists, including normalized ARIA lists | `src/normalize/lists.rs` | `tests/fixtures/snapshots/general/article-listing`, `tests/fixtures/capabilities/listings`, and specialized Hacker News fixtures |
| Code blocks and inline code | `src/document/code.rs`, `src/normalize.rs` | `tests/fixtures/snapshots/general/code-heavy-docs`, `tests/fixtures/snapshots/general/standalone-code-breaks`, and `tests/fixtures/snapshots/compatibility-defuddle/code-blocks` |
| Data tables and normalized layout or listing tables | `src/document/tables.rs`, `src/normalize.rs` | `tests/fixtures/snapshots/general/recommended-data-table`, `tests/fixtures/snapshots/general/table-heavy-reference`, `tests/fixtures/snapshots/general/old-table-layout`, and `tests/fixtures/snapshots/compatibility-defuddle/table-layout` |
| Figures, captions, and images | `src/document/figures.rs`, `src/document/images.rs`, `src/normalize/images.rs` | `tests/fixtures/snapshots/general/figure-heavy`, `tests/fixtures/snapshots/general/figure-caption`, `tests/fixtures/snapshots/general/lazy-images`, `tests/fixtures/snapshots/compatibility-defuddle/images`, and `tests/fixtures/capabilities/images` |
| Footnote references and definitions | `src/document/footnotes.rs`, `src/document/compiler.rs` | `tests/fixtures/snapshots/general/footnotes`, `tests/fixtures/snapshots/compatibility-defuddle/footnotes`, and `tests/fixtures/capabilities/footnotes` |
| Inline and display math | `src/document/math.rs`, `src/normalize.rs` | `tests/fixtures/snapshots/compatibility-defuddle/math`, `tests/fixtures/capabilities/math`, and Readability MathJax fixtures |
| Callouts | `src/document/callouts.rs`, `src/document/compiler.rs` | `tests/fixtures/capabilities/docs/callout` and quality fixtures such as `docs-callout-types` |
| Details and summaries | `src/document/compiler.rs`, `src/render/markdown.rs`, and `src/render/text.rs` | Readability `toc-missing` and focused IR tests. This distinction has limited exact-output coverage and remains provisional. |
| Definition lists | `src/document/compiler.rs`, `src/render/markdown.rs`, and `src/normalize/lists.rs` | Quality fixture `docs-definition-list` and focused IR tests |
| Meaningful media | `src/normalize/media.rs` | `tests/fixtures/capabilities/media` and the quality corpus media categories |
| Discussions | Shared specialized discussion builder and adapters | `tests/fixtures/snapshots/general/barrier-discussion` and all 12 fixtures under `tests/fixtures/snapshots/specialized` |

The semantic IR also includes table spans and alignment because the previous DOM renderers retained that source meaning. It includes typed figures, footnotes, math, and callouts because normalization already identifies those concepts. It does not include arbitrary HTML elements, classes, IDs, styles, or attribute bags.

## Fixture coverage

The current first-party extraction suites contain:

- 79 general fixture directories;
- 11 Defuddle fixture categories;
- 12 Web capability categories;
- 12 specialized extraction fixtures;
- Mozilla Readability compatibility fixtures under `tests/fixtures/compatibility/readability/test/test-pages`.

A unit test extracts every `source.html` below `tests/`. Every successful extraction compiles to the IR and passes IR validation. Fixtures with `expected.error` remain expected extraction failures.
