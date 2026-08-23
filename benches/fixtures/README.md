# Benchmark fixtures

Most HTML snapshots come from Mozilla Readability's test-page corpus. This
repository also uses those snapshots in `tests/fixtures/compatibility/readability/test/test-pages/`.

The `readability-js/` directory contains the source snapshots used by the
fixed corpus benchmark target:

- `medium-2`
- `ars-1`
- `heise`
- `nytimes-5`
- `wikipedia-2`
- `yahoo-2`
- `buzzfeed-1`
- `engadget`
- `guardian-1`

The `go-net-http` directory contains a snapshot of the Go `net/http` package
page from `https://pkg.go.dev/net/http`. The snapshot was captured on
2026-08-20. The page identifies the package content with the BSD-3-Clause
license.

Keep these copies under `benches/` because Cargo packages benchmark sources but excludes the test suite. Do not change a benchmark copy without changing its source attribution and reviewing the Criterion baseline.
