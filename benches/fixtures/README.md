# Benchmark fixtures

These HTML snapshots come from Mozilla Readability's test-page corpus. This repository already uses the same snapshots in `tests/readability-js/test/test-pages/`.

- `guardian-article/source.html` comes from `guardian-1`.
- `wikipedia-reference/source.html` comes from `wikipedia-2`.

Keep these copies under `benches/` because Cargo packages benchmark sources but excludes the test suite. Do not change a benchmark copy without changing its source attribution and reviewing the Criterion baseline.
