# Real-world HTML rendering optimization

## Measurement

- Machine: Linux `dev`, x86_64
- Rust: `rustc 1.98.0 (88d9e12ae 2026-08-18)`
- Benchmark: `cargo bench --bench corpus -- 'mozilla_readability_html' --noplot --sample-size 10`
- Values: Criterion median
- Baseline: `html-before`, measured at the parent revision before this optimization

The benchmark extracts each Mozilla Readability fixture once, then measures
lazy canonical HTML rendering. The sum of the medians fell from **3.602 ms**
to **1.577 ms** (**56.2% faster**).

| Fixture | Before | After | Change |
|---|---:|---:|---:|
| `medium-2` | 30.093 us | 8.698 us | -70.9% |
| `ars-1` | 28.895 us | 6.535 us | -77.2% |
| `heise` | 14.181 us | 2.393 us | -83.1% |
| `nytimes-5` | 157.310 us | 59.186 us | -62.6% |
| `wikipedia-2` | 1.978 ms | 875.920 us | -55.8% |
| `yahoo-2` | 21.012 us | 3.703 us | -82.7% |
| `buzzfeed-1` | 26.494 us | 6.760 us | -74.4% |
| `engadget` | 117.980 us | 51.959 us | -55.8% |
| `guardian-1` | 80.873 us | 31.479 us | -61.0% |
| `go-net-http` | 1.147 ms | 530.530 us | -53.7% |

## Profile findings

The profile showed that HTML escaping sent ordinary characters through
`fmt::Write::write_char` one character at a time. The renderer now writes
unescaped runs as one string and only decodes characters when an escape is
needed. Output and error behavior remain unchanged.
