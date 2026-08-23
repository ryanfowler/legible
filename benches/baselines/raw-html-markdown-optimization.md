# Raw HTML to Markdown optimization

## Measurement

- Machine: Linux `dev`, x86_64, 4 vCPUs
- Rust: `rustc 1.97.1`
- Command: `cargo bench --bench corpus -- --noplot --sample-size 10`
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
- Command: `cargo bench --bench corpus -- --noplot --sample-size 10`
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

## Image-role token matching follow-up

- Machine: Linux `dev`, x86_64
- Rust: `rustc 1.97.1`
- Command: `cargo bench --bench corpus -- --noplot --sample-size 10`
- Profile: `perf record -F 999 -g` on the Guardian fixture

The profile showed that image-role matching rescanned the same context once for
each role pattern. Tokenizing the context once reduced that function from about
11% to about 1.3% of sampled CPU cycles. The fresh raw HTML to Markdown medians
improved as follows. Values are machine-specific.

| Fixture | Markdown before | Markdown after | Change |
|---|---:|---:|---:|
| `medium-2` | 2.323 | 2.295 | -1.2% |
| `ars-1` | 3.763 | 3.544 | -5.8% |
| `heise` | 3.292 | 3.246 | -2.0% |
| `nytimes-5` | 18.710 | 18.040 | -4.1% |
| `wikipedia-2` | 125.020 | 121.010 | -3.2% |
| `yahoo-2` | 27.410 | 26.680 | -2.7% |
| `buzzfeed-1` | 12.484 | 12.226 | -2.1% |
| `engadget` | 33.734 | 31.684 | -6.1% |
| `guardian-1` | 16.409 | 14.920 | -9.1% |

The sum of fixture medians improved by **3.9%**. In-place context lowercasing
and reusable cleanup name buffers reduced transient allocation events. The
instrumented report showed total allocated bytes down about **0.1%** and peak
live bytes unchanged on this run.

## Continued profile-guided pass

- Machine: Linux `dev`, x86_64
- Rust: `rustc 1.97.1`
- Command: `cargo bench --bench corpus -- --noplot --sample-size 10 markdown`
- Baseline: the fresh measurement before the combined optimization series
- Values: Criterion median, milliseconds

| Fixture | Markdown after continuation |
|---|---:|
| `medium-2` | 2.119 |
| `ars-1` | 3.333 |
| `heise` | 3.085 |
| `nytimes-5` | 17.259 |
| `wikipedia-2` | 114.840 |
| `yahoo-2` | 26.443 |
| `buzzfeed-1` | 12.019 |
| `engadget` | 31.070 |
| `guardian-1` | 14.126 |

The sum of these medians is **224.295 ms**. The fresh baseline sum was
**243.145 ms**, which is a **7.8%** improvement in this run. Relative to the
immediate pre-continuation sum of **233.644 ms**, this continuation contributes
about **3.8%**. Repeated runs on the shared machine ranged from about 221 ms to
225 ms. The result is therefore below a defensible double-digit corpus-wide
claim against the fresh baseline.

The continuation changes were guided by profiles. They add a leaf and
single-text-child statistics fast path, preserve valid subtree statistics
across targeted cleanup detaches, dispatch common semantic class tokens by
first byte, skip SVG-only normalization when no SVG exists, and normalize
access-barrier text in place for ASCII input. Footnote analysis now stores its
per-node flags compactly.

The final instrumented report measured **98,680,112 allocated bytes** and
**384,946 allocation events** across the nine fixtures. The earlier report
measured 100,573,277 bytes and 425,731 events. This is **1.9% fewer allocated
bytes** and **9.6% fewer allocation events**. The largest final fixture sample
used 20,819,120 live bytes.

## Final continued profile-guided pass

- Machine: Linux `dev`, x86_64
- Rust: `rustc 1.97.1`
- Command: `cargo bench --bench corpus -- 'mozilla_readability_markdown' --noplot`
- Measurement: 30 Criterion samples per fixture; the repository benchmark was restored to 10 samples after measurement
- Values: Criterion median, milliseconds

| Fixture | Markdown median |
|---|---:|
| `medium-2` | 2.1053 |
| `ars-1` | 3.3820 |
| `heise` | 3.1719 |
| `nytimes-5` | 17.444 |
| `wikipedia-2` | 117.870 |
| `yahoo-2` | 25.357 |
| `buzzfeed-1` | 12.135 |
| `engadget` | 31.307 |
| `guardian-1` | 14.049 |

The summed median is **226.821 ms**, or **6.7% faster** than the fresh
243.145 ms reference. This is below a defensible double-digit corpus-wide
claim. The focused Wikipedia and Guardian runs improved after the normalized
text rewrite, but shared-machine variance remained significant across the
full corpus.

The retained changes include stack-based semantic fact propagation, a smaller
source-quality statistics merger, ASCII fast paths for normalized text and
fragment targets, cached `aria-modal` source evidence, and allocation-free
URI and footnote label trimming. The current instrumented real-world report
measured **95,201,798 allocated bytes** and **384,892 allocation events**.
That is **5.3% fewer allocated bytes** and **9.6% fewer allocation events**
than the 100,573,277-byte and 425,731-event reference report.

## Double-digit profile-guided pass

- Machine: Linux `dev`, x86_64
- Rust: `rustc 1.97.1`
- Command: `cargo bench --bench corpus -- 'mozilla_readability_markdown' --noplot`
- Measurement: 30 Criterion samples per fixture; the repository benchmark was restored to 10 samples after measurement
- Values: Criterion median, milliseconds

| Fixture | Markdown median |
|---|---:|
| `medium-2` | 2.0581 |
| `ars-1` | 3.3352 |
| `heise` | 3.1072 |
| `nytimes-5` | 16.6166 |
| `wikipedia-2` | 111.6468 |
| `yahoo-2` | 24.2400 |
| `buzzfeed-1` | 12.0841 |
| `engadget` | 29.4418 |
| `guardian-1` | 13.3809 |

The summed median is **215.911 ms**, or **11.2% faster** than the fresh
243.145 ms reference. The final profile-guided changes add direct equality
fast paths for common attribute names, ASCII tokenization for specialized
class checks and callout prefixes, and ASCII-aware trimming in scoring and
table text paths. These paths retain Unicode fallbacks and all fixture output
remains unchanged.

The fresh instrumented report continues to show allocation reductions from
the earlier pass: the real-world report measured **95,201,798 allocated
bytes** and **384,892 allocation events**, versus **100,573,277 bytes** and
**425,731 events** in the reference report. That is **5.3% fewer allocated
bytes** and **9.6% fewer allocation events**. The synthetic large-prose
report recorded **13,857,591 peak live bytes** for the large-prose workload
and **18,695,952** for the large ordinary-inline workload.

## Scanner and token fast-path pass

- Machine: macOS arm64 (Apple M2 Pro, 12 logical CPUs), AC power
- Rust: `rustc 1.98.0`
- Command: `cargo bench --bench corpus -- 'mozilla_readability_markdown' --noplot --baseline ac-start`
- Baseline: the parent revision on the same machine, saved as `ac-start`
- Values: Criterion median, milliseconds. Sample size is the repository default of 10.

| Fixture | Markdown before | Markdown after | Change |
|---|---:|---:|---:|
| `medium-2` | 1.249 | 1.172 | -6.2% |
| `ars-1` | 1.712 | 1.708 | -0.3% |
| `heise` | 1.614 | 1.527 | -5.4% |
| `nytimes-5` | 10.294 | 9.733 | -5.4% |
| `wikipedia-2` | 64.453 | 60.976 | -5.4% |
| `yahoo-2` | 13.632 | 13.488 | -1.1% |
| `buzzfeed-1` | 7.286 | 6.922 | -5.0% |
| `engadget` | 19.400 | 17.641 | -9.1% |
| `guardian-1` | 6.788 | 6.572 | -3.2% |
| `go-net-http` | 39.788 | 40.336 | +1.4% |

The summed median improved by about **3.7%** for raw HTML to Markdown.
Repeated runs on this machine varied by roughly two percentage points per
fixture, and background load skewed whole-corpus runs, so the recorded run
was taken on a quiet machine.

The changes came from profile-guided analysis:

- New `scan.rs` word-at-a-time ASCII whitespace scanners with a scalar fast
  path below 32 bytes. Normalized-text appending and normalized character
  counting now iterate whitespace-separated tokens through these scanners,
  which also removed a redundant UTF-8 validation pass per token.
- Mixed-content text statistics classify ASCII bytes through the class table
  and decode only real non-ASCII characters, removing Unicode property table
  lookups from common text.
- Token matchers no longer pre-scan values for whitespace before matching.
  Case-insensitive substring search uses memchr candidates on longer values.
- Pricing-content detection checks its currency-symbol gate first and skips
  all substring scans when no currency mark exists near digits. Document TOC
  detection makes one traversal instead of two plus a link vector.
- Footnote ID convention checks compare prefixes without allocating.
- The dynamic attribute fast gate covers the frequently queried footnote and
  callout data attributes, skipping the full attribute-name matcher.

All fixture outputs remain unchanged; the general, Defuddle, web, and Mozilla
Readability suites pass without snapshot updates.
