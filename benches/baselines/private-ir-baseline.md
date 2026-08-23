# Private semantic representation baseline

This note freezes the baseline before private semantic representation work.
The source revision is `499e09f3bf2e53164321e991254b9ff124cccb59`.

## Environment

- Machine: `DO-Premium-Intel`, 4 vCPUs, x86_64 Linux 7.0.0-27-generic
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- Initial benchmark command: `cargo bench --bench pipeline -- --save-baseline before-task`
- Current benchmark command: `cargo bench --bench pipeline -- lower_retained_fragment --noplot`
- Criterion baseline: `target/criterion/**/before-task`

Absolute times are specific to this machine. Use the Criterion baseline for
statistical comparisons. The initial values below are median point estimates
from the starting benchmark harness.

## Initial pipeline timings

| Benchmark | Median |
|---|---:|
| `extract/small/prose` | 597.52 us |
| `extract/medium/prose` | 8.846 ms |
| `extract/large/prose` | 69.463 ms |
| `extract/large/reference` | 218.396 ms |
| `extract/medium/ordinary-inline` | 12.291 ms |
| `extract/large/ordinary-inline` | 165.794 ms |
| `document_compile/simple-prose` | 45.26 us |
| `document_compile/ordinary-inline` | 1.147 ms |
| `document_compile/ordinary-inline-large` | 14.576 ms |
| `document_compile/highlighted-code` | 1.934 ms |
| `document_compile/table-heavy` | 5.791 ms |
| `document_compile/documentation` | 3.143 ms |
| `document_compile/listing` | 1.747 ms |
| `lazy_output/short/markdown` | 20.26 us |
| `lazy_output/long/markdown` | 908.30 us |
| `lazy_output/long/text` | 521.52 us |
| `lazy_output/long/html` | 616.79 us |
| `extract_markdown/small/prose` | 585.71 us |
| `extract_markdown/medium/prose` | 6.337 ms |
| `extract_markdown/large/prose` | 57.652 ms |
| `extract_markdown/large/ordinary-inline` | 195.382 ms |
| `extract_markdown/large/malformed` | 196.333 ms |
| `complex_pages/documentation` | 82.302 ms |
| `complex_pages/highlighted-code` | 70.584 ms |
| `complex_pages/math` | 67.962 ms |
| `complex_pages/table-heavy` | 73.592 ms |
| `large_compatibility_fixtures/guardian-article` | 49.046 ms |
| `large_compatibility_fixtures/wikipedia-reference` | 330.704 ms |

The baseline harness renames the compiler group to `lower_retained_fragment`
and expands ordinary and complex fixture coverage. The current lowering
measurements below are the frozen reference for those new benchmark IDs.

## Lowering measurements

| Fixture | Median |
|---|---:|
| `simple-prose` | 42.77 us |
| `long-prose` | 3.001 ms |
| `ordinary-inline` | 833.88 us |
| `ordinary-inline-large` | 13.559 ms |
| `highlighted-code` | 1.345 ms |
| `math` | 2.893 ms |
| `table-heavy` | 4.343 ms |
| `documentation` | 2.564 ms |
| `footnotes` | 3.167 ms |
| `listing` | 1.712 ms |
| **average semantic nodes** | **6,993.7** |

## Representation snapshot

This section records the linked-arena layout before the compact event-tape
migration. The benchmark reports the historical layout sizes below:

| Measurement | Value |
|---|---:|
| `size_of::<ArenaNode>()` | 80 bytes |
| `size_of::<EventOp>()` | 8 bytes |
| `size_of::<TextValue>()` | 24 bytes |

The lowering benchmark records these values for every retained-fragment fixture:

| Fixture | DOM nodes | Semantic nodes | Roots | Retained bytes | Semantic string bytes | String values* |
|---|---:|---:|---:|---:|---:|---:|
| `simple-prose` | 171 | 168 | 24 | 16,934 | 3,134 | 72 |
| `long-prose` | 10,048 | 10,045 | 1,435 | 1,001,769 | 189,745 | 4,305 |
| `ordinary-inline` | 2,801 | 2,733 | 1 | 256,867 | 32,779 | 1,431 |
| `ordinary-inline-large` | 27,913 | 27,261 | 1 | 2,560,712 | 327,664 | 14,279 |
| `highlighted-code` | 5,613 | 1,496 | 374 | 144,818 | 22,858 | 1,122 |
| `math` | 8,529 | 4,060 | 406 | 354,358 | 27,278 | 3,248 |
| `table-heavy` | 10,965 | 10,150 | 406 | 905,974 | 26,654 | 4,060 |
| `documentation` | 7,283 | 6,188 | 364 | 624,106 | 39,346 | 2,912 |
| `footnotes` | 4,249 | 3,474 | 386 | 403,406 | 49,078 | 1,930 |
| `listing` | 4,365 | 4,362 | 727 | 430,857 | 77,569 | 2,181 |

\* String values count owned string-bearing fields. This is a practical
allocation proxy. It does not instrument the global allocator, so it is not an
exact allocator-call count.

The extraction benchmark also prints source DOM nodes, final cleaned DOM nodes,
semantic nodes, and retained document bytes for the accepted attempt. These
measurements allow later work to compare the selected pre-IR representation
with the retained semantic representation.

## Current full-suite medians

The complete current harness was saved with:

```text
cargo bench --bench pipeline -- --save-baseline task-0
```

The Criterion artifact is `target/criterion/**/task-0`. The following medians
record all output and extraction groups so later tasks can compare each phase.

### Extraction

| Benchmark | Median |
|---|---:|
| `extract/small/prose` | 593.47 us |
| `extract/small/reference` | 1.571 ms |
| `extract/medium/prose` | 9.575 ms |
| `extract/medium/reference` | 17.410 ms |
| `extract/medium/listing` | 10.311 ms |
| `extract/medium/ordinary-inline` | 10.788 ms |
| `extract/large/prose` | 68.854 ms |
| `extract/large/reference` | 229.273 ms |
| `extract/large/listing` | 103.603 ms |
| `extract/large/ordinary-inline` | 149.260 ms |

### Retained-fragment lowering

| Benchmark | Median |
|---|---:|
| `lower_retained_fragment/simple-prose` | 42.77 us |
| `lower_retained_fragment/long-prose` | 3.001 ms |
| `lower_retained_fragment/ordinary-inline` | 833.88 us |
| `lower_retained_fragment/ordinary-inline-large` | 13.559 ms |
| `lower_retained_fragment/highlighted-code` | 1.345 ms |
| `lower_retained_fragment/math` | 2.893 ms |
| `lower_retained_fragment/table-heavy` | 4.343 ms |
| `lower_retained_fragment/documentation` | 2.564 ms |
| `lower_retained_fragment/footnotes` | 3.167 ms |
| `lower_retained_fragment/listing` | 1.712 ms |

### End-to-end Markdown

| Benchmark | Median |
|---|---:|
| `extract_markdown/small/prose` | 560.20 us |
| `extract_markdown/medium/prose` | 6.034 ms |
| `extract_markdown/medium/reference` | 16.573 ms |
| `extract_markdown/medium/ordinary-inline` | 14.017 ms |
| `extract_markdown/large/prose` | 66.690 ms |
| `extract_markdown/large/ordinary-inline` | 118.813 ms |
| `extract_markdown/large/malformed` | 186.895 ms |

### Lazy output

| Benchmark | Median |
|---|---:|
| `lazy_output/short/markdown` | 15.85 us |
| `lazy_output/short/text` | 8.00 us |
| `lazy_output/short/html` | 9.61 us |
| `lazy_output/long/markdown` | 932.75 us |
| `lazy_output/long/text` | 795.61 us |
| `lazy_output/long/html` | 596.15 us |
| `lazy_output/reference/markdown` | 293.10 us |
| `lazy_output/reference/text` | 105.82 us |
| `lazy_output/reference/html` | 96.08 us |
| `lazy_output/medium/ordinary-inline/markdown` | 430.07 us |
| `lazy_output/medium/ordinary-inline/text` | 138.06 us |
| `lazy_output/medium/ordinary-inline/html` | 147.14 us |
| `lazy_output/large/ordinary-inline/markdown` | 3.111 ms |
| `lazy_output/large/ordinary-inline/text` | 1.044 ms |
| `lazy_output/large/ordinary-inline/html` | 1.829 ms |

### Complex and compatibility coverage

| Benchmark | Median |
|---|---:|
| `complex_pages/prose` | 31.531 ms |
| `complex_pages/documentation` | 73.495 ms |
| `complex_pages/footnotes-reference` | 55.840 ms |
| `complex_pages/highlighted-code` | 72.790 ms |
| `complex_pages/math` | 72.499 ms |
| `complex_pages/table-heavy` | 70.052 ms |
| `complex_pages/listing` | 44.184 ms |
| `complex_pages/malformed` | 183.334 ms |
| `complex_pages/metadata-heavy` | 33.339 ms |
| `complex_pages/json-ld-heavy` | 27.817 ms |
| `large_compatibility_fixtures/guardian-article` | 26.580 ms |
| `large_compatibility_fixtures/wikipedia-reference` | 251.681 ms |
