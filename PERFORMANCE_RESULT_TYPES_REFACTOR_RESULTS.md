# Result type refactor benchmark results

Benchmark date: 2026-08-05

Criterion settings:

```text
warm-up: 1 second
measurement: 3 seconds
sample size: 30
```

The benchmarks used the same machine and fixtures. Times below are Criterion midpoint estimates.

## Markdown extraction comparison

The `main` API creates HTML, Markdown, and text in one `parse` call. It has no Markdown-only extraction path. The current branch creates only Markdown with `Extractor::extract_markdown`. Therefore, this comparison shows the cost to request Markdown from each public API. It does not isolate Markdown rendering.

| Fixture                  | `main` `parse` | Current `extract_markdown` | Change |
| ------------------------ | -------------: | -------------------------: | -----: |
| `medium-2`               |      614.10 µs |                  556.51 µs |  -9.4% |
| `wikipedia-2`            |      25.967 ms |                  22.040 ms | -15.1% |
| `large-retained-article` |      2.2559 ms |                  1.8001 ms | -20.2% |

## Current branch output formats

| Fixture                  |      HTML |  Markdown |      Text | Three separate calls | Legacy one-pass all formats |
| ------------------------ | --------: | --------: | --------: | -------------------: | --------------------------: |
| `medium-2`               | 596.28 µs | 556.51 µs | 543.94 µs |            1.6963 ms |                   631.36 µs |
| `wikipedia-2`            | 24.811 ms | 22.040 ms | 21.515 ms |            68.403 ms |                   27.008 ms |
| `large-retained-article` | 2.1014 ms | 1.8001 ms | 1.7122 ms |            5.6178 ms |                   2.3940 ms |

## Legacy one-pass comparison

This comparison measures one extraction that renders all three formats on both revisions.

| Fixture                  |    `main` | Current branch | Change |
| ------------------------ | --------: | -------------: | -----: |
| `medium-2`               | 614.10 µs |      631.36 µs |  +2.8% |
| `wikipedia-2`            | 25.967 ms |      27.008 ms |  +4.0% |
| `large-retained-article` | 2.2559 ms |      2.3940 ms |  +6.1% |

The unchanged `parse_retries/medium-2` case measured 2.0966 ms. Criterion reported a +1.0% change, within its noise threshold.

No allocator or heap profile was collected. These results do not support claims about allocation counts or peak memory.
