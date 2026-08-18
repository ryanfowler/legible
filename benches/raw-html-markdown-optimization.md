# Raw HTML to Markdown optimization

## Measurement

- Machine: Linux `dev`, x86_64, 4 vCPUs
- Rust: `rustc 1.97.1`
- Command: `cargo bench --bench real_world -- --noplot --sample-size 10`
- Baseline: the same command before this change
- Values: Criterion median, milliseconds

| Fixture | Extraction before | Extraction after | Markdown before | Markdown after |
|---|---:|---:|---:|---:|
| `medium-2` | 3.001 | 2.470 | 3.096 | 2.533 |
| `ars-1` | 3.793 | 3.501 | 3.879 | 3.627 |
| `heise` | 3.627 | 3.243 | 3.643 | 3.382 |
| `nytimes-5` | 20.771 | 19.961 | 20.969 | 20.588 |
| `wikipedia-2` | 140.990 | 125.180 | 142.980 | 127.010 |
| `yahoo-2` | 32.985 | 27.505 | 32.841 | 28.511 |
| `buzzfeed-1` | 16.432 | 13.527 | 16.494 | 13.845 |
| `engadget` | 37.680 | 34.977 | 37.518 | 35.118 |
| `guardian-1` | 21.482 | 21.064 | 21.463 | 21.173 |

The sum of fixture medians improved by **10.4% for extraction** and **9.6%
for raw HTML to Markdown**. These values are machine-specific.

## Instrumented memory sample

The command below uses the benchmark instrumentation report. It measures the
normal extraction path without diagnostics. Use rows named
`counters/real-world/<fixture>`. The report resets allocator counters before
each fixture. The before run used the parent revision with the same real-world
report fixture list. The after run used this revision.

```bash
cargo run --release --bin extraction-report --features bench-instrumentation
```

| Fixture | Allocated bytes before | Allocated bytes after | Peak live bytes before | Peak live bytes after |
|---|---:|---:|---:|---:|
| `medium-2` | 965,894 | 884,795 | 422,134 | 422,586 |
| `nytimes-5` | 8,937,068 | 8,075,904 | 4,433,172 | 4,443,538 |
| `wikipedia-2` | 65,845,589 | 54,340,681 | 21,700,160 | 20,819,120 |
| `yahoo-2` | 14,057,280 | 11,925,052 | 6,516,852 | 6,516,822 |
| `buzzfeed-1` | 6,550,414 | 5,626,270 | 2,845,733 | 2,849,121 |

Wikipedia external footnote handling changed from 380 subtree copies to one
lazy import source. This reduced its allocated bytes by 17.5% and peak live
bytes by 4.1%.

## Follow-up source and renderer optimization

- Machine: Linux `dev`, x86_64, 16 logical CPUs
- Rust: `rustc 1.97.1`
- Command: `cargo bench --bench real_world -- --noplot --sample-size 10`
- Baseline: `aff80a3`, measured before the follow-up changes
- Values: Criterion median, milliseconds

| Fixture | Markdown before | Markdown after | Change |
|---|---:|---:|---:|
| `medium-2` | 2.613 | 2.340 | -10.4% |
| `ars-1` | 3.634 | 3.604 | -0.8% |
| `heise` | 3.320 | 3.304 | -0.5% |
| `nytimes-5` | 20.771 | 19.077 | -8.2% |
| `wikipedia-2` | 127.429 | 126.556 | -0.7% |
| `yahoo-2` | 27.482 | 27.684 | +0.7% |
| `buzzfeed-1` | 13.753 | 12.869 | -6.4% |
| `engadget` | 35.416 | 33.615 | -5.1% |
| `guardian-1` | 21.463 | 16.702 | -22.2% |

The sum of fixture medians improved by **4.0%** for raw HTML to Markdown in
this final run. Extraction-only medians improved by **5.3%**. Repeated runs
varied by about two percentage points on this shared machine. The largest
gains came from allocation-free image-role token matching and shared
source-order scans for footnote analysis. Markdown lookahead now scans forward
only when trailing punctuation can form a cross-node Markdown construct.

The instrumented report showed total allocated bytes falling from 102,787,493
to 100,573,277 (**-2.2%**) across the nine fixtures. Peak live bytes stayed
within measurement noise. The retained semantic representation did not grow.
