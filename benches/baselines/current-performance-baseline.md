# Current performance baseline

This file records the baseline used for the extraction measurement work.

- Machine: MacBook Pro Mac14,10, Apple M2 Pro, 12 cores, 32 GB RAM
- OS: Darwin 25.6.0, arm64
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- Implementation-start revision: `87f604d2fe0b094bfdc3fec6c24fda27a13586a3`
- Benchmark command: `cargo bench --bench pipeline -- --save-baseline m01-current`
- Criterion baseline: `target/criterion/**/m01-current`

Criterion medians are in nanoseconds. The values are point estimates from the
named Criterion baseline on this machine. Re-run the command on the same
machine and toolchain before comparing a change.

| Benchmark | Median (ns) |
| --- | ---: |
| `extract/small/prose` | 250173 |
| `extract/small/reference` | 588447 |
| `extract/medium/prose` | 2700101 |
| `extract/medium/reference` | 6793089 |
| `extract/medium/listing` | 4121816 |
| `extract/medium/ordinary-inline` | 4546210 |
| `extract/large/prose` | 26758500 |
| `extract/large/reference` | 67107333 |
| `extract/large/listing` | 40161406 |
| `extract/large/ordinary-inline` | 44909386 |
| `extract_markdown/small/prose` | 252746 |
| `extract_markdown/medium/prose` | 2810898 |
| `extract_markdown/medium/reference` | 7002941 |
| `extract_markdown/medium/ordinary-inline` | 4779742 |
| `extract_markdown/large/prose` | 28095302 |
| `extract_markdown/large/ordinary-inline` | 46438573 |
| `extract_markdown/large/malformed` | 85485480 |
| `lazy_output/short/markdown` | 8447 |
| `lazy_output/short/text` | 4762 |
| `lazy_output/short/html` | 5398 |
| `lazy_output/medium/ordinary-inline/markdown` | 119827 |
| `lazy_output/medium/ordinary-inline/text` | 56447 |
| `lazy_output/medium/ordinary-inline/html` | 73192 |
| `lazy_output/long/markdown` | 514894 |
| `lazy_output/long/text` | 291945 |
| `lazy_output/long/html` | 319168 |
| `lazy_output/large/ordinary-inline/markdown` | 1173964 |
| `lazy_output/large/ordinary-inline/text` | 555878 |
| `lazy_output/large/ordinary-inline/html` | 719994 |
| `lazy_output/reference/markdown` | 120242 |
| `lazy_output/reference/text` | 54399 |
| `lazy_output/reference/html` | 68111 |
| `complex_pages/prose` | 13684517 |
| `complex_pages/documentation` | 33337778 |
| `complex_pages/footnotes-reference` | 24515302 |
| `complex_pages/highlighted-code` | 30845688 |
| `complex_pages/math` | 29416192 |
| `complex_pages/media-heavy` | 17848756 |
| `complex_pages/table-heavy` | 28897107 |
| `complex_pages/listing` | 20245555 |
| `complex_pages/malformed` | 84472382 |
| `complex_pages/metadata-heavy` | 14709694 |
| `complex_pages/json-ld-heavy` | 12568561 |
| `large_compatibility_fixtures/guardian-article` | 12127611 |
| `large_compatibility_fixtures/wikipedia-reference` | 86219211 |
| `deeply_nested_document/1000` | 4433729 |
| `deeply_nested_document/2000` | 17049188 |
| `deeply_nested_document/4000` | 67224271 |
| `deeply_nested_document/8000` | 263475230 |
| `lower_retained_fragment/simple-prose/dom-171-semantic-168` | 21917 |
| `lower_retained_fragment/long-prose/dom-10048-semantic-10045` | 1269589 |
| `lower_retained_fragment/ordinary-inline/dom-2801-semantic-2733` | 404617 |
| `lower_retained_fragment/ordinary-inline-large/dom-27913-semantic-27261` | 4032595 |
| `lower_retained_fragment/highlighted-code/dom-5613-semantic-1496` | 522684 |
| `lower_retained_fragment/math/dom-8529-semantic-4060` | 1501012 |
| `lower_retained_fragment/table-heavy/dom-10965-semantic-10150` | 2490564 |
| `lower_retained_fragment/documentation/dom-7283-semantic-6188` | 1465671 |
| `lower_retained_fragment/footnotes/dom-4249-semantic-3474` | 1475607 |
| `lower_retained_fragment/listing/dom-4365-semantic-4362` | 794028 |
| `owned_lowering_comparison/prose/borrowed` | 506884 |
| `owned_lowering_comparison/prose/owned` | 545687 |
| `owned_lowering_comparison/highlighted-code/borrowed` | 518628 |
| `owned_lowering_comparison/highlighted-code/owned` | 568680 |
| `owned_lowering_comparison/large-raw-code/borrowed` | 4360 |
| `owned_lowering_comparison/large-raw-code/owned` | 2585 |

## Instrumented report sample

The command below produced `target/extraction-report.txt` during baseline
collection:

```bash
cargo run --release --bin extraction-report --features bench-instrumentation \
  > target/extraction-report.txt
```

The report includes phase durations, rendering output sizes, retry winners and
attempts, allocation totals, peak live bytes, retained document estimates, DOM
clone bytes, source scan counts, semantic operation counts, builder capacities,
and separate JSON-LD source, parsed, and retained byte estimates.

Representative extraction workloads from that report:

| Workload | Winner | Attempts | Allocated bytes | Peak live bytes | DOM clone bytes | Full scans | Element snapshots | Retained bytes | JSON-LD parsed | JSON-LD retained |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `small-prose` | normal | 1 | 311789 | 86800 | 24000 | 21 | 19 | 7736 | 0 | 0 |
| `medium-prose` | normal | 1 | 3722823 | 950446 | 223680 | 21 | 19 | 103688 | 0 | 0 |
| `large-prose` | normal | 1 | 37005096 | 9513205 | 2166720 | 21 | 19 | 909136 | 0 | 0 |
| `large-ordinary-inline` | normal | 1 | 55956955 | 13249772 | 2699320 | 21 | 19 | 1282127 | 0 | 0 |
| `metadata-heavy` | normal | 1 | 17716712 | 4567707 | 1077440 | 21 | 19 | 438320 | 0 | 0 |
| `json-ld-heavy` | normal | 1 | 17253453 | 4478753 | 974080 | 21 | 19 | 432608 | 53612 | 0 |
| `json-ld-retained` | normal | 1 | 17407833 | 4478753 | 974080 | 21 | 19 | 432608 | 53612 | 53580 |

Phase durations are in nanoseconds. Columns use the `Phase` enum order:
parse, metadata, preparation, candidate discovery, scoring, root selection,
fragment copy, cleanup, semantic compilation, rendering.

| Workload | Phase durations (ns) |
| --- | --- |
| `normal` | 979708, 1018709, 139584, 208792, 39750, 35708, 875, 460708, 28708, 15250 |
| `relaxed-cleanup` | 10792, 6625, 6375, 23626, 9542, 3126, 2375, 56833, 14666, 16958 |
| `broad-content` | 9125, 4833, 5708, 25375, 1200999, 5333, 1166, 76916, 12124, 5167 |
| `structured-data-hint` | 21291, 20667, 11334, 203917, 93833, 254876, 5332, 262291, 46041, 5749 |
| `relaxed-visibility` | 6167, 3541, 4167, 16208, 11127, 9709, 874, 40876, 8250, 2583 |
| `body-fallback` | 13208, 4792, 7000, 22792, 30709, 4460, 2751, 111000, 45583, 6042 |
| `large-prose` | 4132458, 598917, 1552875, 6344042, 2708125, 177375, 317083, 16073875, 2105542, 2699500 |
| `metadata-heavy` | 2169417, 608458, 783708, 2937582, 2581250, 61667, 141250, 7126000, 956125, 1058251 |
| `json-ld-retained` | 1863500, 563291, 727833, 2770291, 1021209, 160875, 133083, 6772417, 901707, 1028083 |

Retry fixtures selected `relaxed-cleanup`, `broad-content`, `relaxed-visibility`,
and `body-fallback`. The structured-data fixture evaluated the structured-data
and body-fallback strategies after the broad-content attempt. The report also
confirmed the explicit content-root fixture and the `reddit` specialized
extractor identity.
