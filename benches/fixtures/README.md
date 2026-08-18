# Benchmark fixtures

These HTML snapshots come from Mozilla Readability's test-page corpus. This
repository also uses the same snapshots in
`tests/readability-js/test/test-pages/`.

The `readability-js/` directory contains the source snapshots used by the
real-world benchmark target:

- `medium-2`
- `ars-1`
- `heise`
- `nytimes-5`
- `wikipedia-2`
- `yahoo-2`
- `buzzfeed-1`
- `engadget`
- `guardian-1`

Keep these copies under `benches/` because Cargo packages benchmark sources but excludes the test suite. Do not change a benchmark copy without changing its source attribution and reviewing the Criterion baseline.
