# Semantic IR migration baseline

This note records the behavior baseline for the semantic document migration. Stage A does not change a production output path.

## Revision

The migration baseline is `main` at `a97e49c460a0ef0e8e4ed271ef2b3f417c7b6184`.

## Current output contracts

`ExtractedPage` retains a compact cleaned DOM fragment. It renders each output on demand.

- `markdown()` returns CommonMark and GFM output. It does not emit raw HTML. It filters unsupported link and image URI schemes.
- `text()` returns normalized plain text. `text_length()` counts its Unicode scalar values. `word_count()` uses the same normalized DOM walk.
- `html()` returns extracted markup from the retained DOM. It is not sanitized.
- `safe_html()` copies and sanitizes the retained fragment before it renders HTML.
- Repeated render calls are deterministic.

The raw `html()` shape is compatibility-sensitive. Unit tests require source attributes to remain in `html()` while `safe_html()` removes active content. A later switch to canonical semantic HTML must be a documented contract change.

## Quality baseline

Run the corpus from the repository root:

```bash
npm --prefix scripts/compare-extractors ci
cargo fetch
node scripts/compare-extractors/index.mjs --all
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

The checked-in historical comparison is `benchmarks/quality/BASELINE.md`.

## Performance baseline

Use the same machine, Rust toolchain, power mode, and background workload for both runs.

```bash
cargo bench --bench extraction -- --save-baseline pre-ir
cargo bench --bench extraction -- --baseline pre-ir
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
| Paragraphs, headings, quotes, emphasis, strong text, links, breaks, and thematic breaks | `src/markdown.rs`, `src/text.rs`, `src/normalize/headings.rs` | General, Defuddle, Web, and Readability suites |
| Ordered and unordered lists, including normalized ARIA lists | `src/normalize/lists.rs` | `tests/general/article-listing`, `tests/web/listings`, and specialized Hacker News fixtures |
| Code blocks and inline code | `src/normalize/code.rs` | `tests/general/code-heavy-docs`, `tests/general/standalone-code-breaks`, and `tests/defuddle/code-blocks` |
| Data tables and normalized layout or listing tables | `src/normalize/tables.rs`, repeated-listing normalization | `tests/general/recommended-data-table`, `tests/general/table-heavy-reference`, `tests/general/old-table-layout`, and `tests/defuddle/table-layout` |
| Figures, captions, and images | `src/normalize/images.rs`, figure normalization | `tests/general/figure-heavy`, `tests/general/figure-caption`, `tests/general/lazy-images`, `tests/defuddle/images`, and `tests/web/images` |
| Footnote references and definitions | `src/normalize/footnotes.rs` | `tests/general/footnotes`, `tests/defuddle/footnotes`, and `tests/web/footnotes` |
| Inline and display math | `src/normalize/math.rs` | `tests/defuddle/math`, `tests/web/math`, and Readability MathJax fixtures |
| Callouts | `src/normalize/callouts.rs` | `tests/web/docs/callout` and quality fixtures such as `docs-callout-types` |
| Details and summaries | DOM Markdown and text block handling | Readability `toc-missing` and focused IR tests. This distinction has limited exact-output coverage and remains provisional. |
| Definition lists | DOM Markdown and text handling, list normalization | Quality fixture `docs-definition-list` and focused IR tests |
| Meaningful media | `src/normalize/media.rs` | `tests/web/media` and the quality corpus media categories |
| Discussions | Shared specialized discussion builder and adapters | `tests/general/barrier-discussion` and all 12 fixtures under `tests/specialized` |

The crate-private IR also includes table spans and alignment because the current DOM Markdown and HTML paths retain that source meaning. It includes typed figures, footnotes, math, and callouts because normalization already identifies those concepts. It does not include arbitrary HTML elements, classes, IDs, styles, or attribute bags.

## Fixture coverage

The current first-party extraction suites contain:

- 79 general fixture directories;
- 11 Defuddle fixture categories;
- 12 Web capability categories;
- 12 specialized extraction fixtures;
- Mozilla Readability compatibility fixtures under `tests/readability-js/test/test-pages`.

A Stage A unit test extracts every `source.html` below `tests/`. Every successful extraction compiles to the IR and passes IR validation. Fixtures with `expected.error` remain expected extraction failures.
