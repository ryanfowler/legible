# Test suites

Choose a suite by the result that it verifies.

## Unit tests

Unit tests are next to the Rust modules under `src/`. Use them for one internal
algorithm or invariant.

```bash
cargo test --lib
```

## Repository fixture tests

`fixture_tests` runs two fixture types:

- `fixtures/snapshots/` requires exact Markdown or an exact public error.
- `fixtures/capabilities/` checks required behavior without fixing all output.

A snapshot fixture contains `source.html` and either `expected.md` or
`expected.error`. It can also contain `metadata.json` and `url.txt`.
`LEGIBLE_UPDATE_FIXTURES=1` updates snapshot Markdown. The
`snapshots/sites/` fixtures are minimized, original documents that model markup
from Wikipedia, GitHub, Stack Overflow, MDN, Medium, Reddit, npm, and arXiv.
They compare exact Markdown and metadata without copied third-party page text.

A capability fixture contains `source.html` and `expected.json`. Use capability
fixtures for extraction rules that permit more than one correct rendering. A
`url.txt` file sets the source URL used for relative-link resolution; the test
never fetches it. Use capability assertions for behavior that permits more
than one correct rendering.

```bash
cargo test --test fixture_tests
cargo test --test fixture_tests fixture-name
```

The snapshot directories record fixture provenance:

- `general/` contains repository-owned extraction regressions.
- `specialized/` contains recognized listing and discussion pages.
- `compatibility-defuddle/` contains minimized Defuddle compatibility cases.
- `sites/` contains minimized website-shape extraction cases with exact output.

## Readability compatibility

`fixtures/compatibility/readability/` is the vendored Mozilla Readability
corpus. Its runner checks retained words, text order, broad semantic structure,
and metadata. It does not require source-shaped HTML.

```bash
cargo test --test readability_compat_tests
```

## Public API tests

The other Rust files directly under `tests/` contain focused public API and
regression tests. `cargo test` runs them with all fixture suites.

## Quality evaluation

`evals/quality/` is an independent scored corpus. It is not a snapshot suite or
a timing benchmark. The evaluator compares Legible and a pinned Defuddle
version against the same manifests.

```bash
npm --prefix tools/extractor-eval ci
cargo fetch
node tools/extractor-eval/index.mjs --all
```

## Performance and fuzzing

See `benches/README.md` for Criterion timing suites. See `fuzz/README.md` for
robustness targets.
