# Extractor quality and performance runners

The quality runner compares Legible against a pinned Defuddle release using the independent fixtures in `benchmarks/quality/`. It scores both extractors against each fixture manifest. Defuddle output is a comparison. It is not the expected answer.

The performance runner compares Legible, Defuddle, and Firecrawl HTML Extractor using the Mozilla Readability page corpus:

```bash
node scripts/compare-extractors/performance.mjs
```

It reports median extraction time for three timed rounds after two warmup rounds. It measures Defuddle with linkedom, and measures both synchronous and asynchronous Firecrawl NAPI calls. The runner reads the corpus before timing and does not fetch pages.

The performance helper accepts another corpus root and round count:

```bash
node scripts/compare-extractors/performance.mjs tests/readability-js/test/test-pages 3
```

## Install

Install the exact versions from `package-lock.json`:

```bash
npm --prefix scripts/compare-extractors ci
cargo fetch
```

The quality runner pins `defuddle` to version 0.19.2. The performance runner pins `@firecrawl/html-extractor` to version 0.1.2. It disables Defuddle asynchronous enrichment and invokes Cargo with `--offline`. After the Node and Rust dependencies are present, a benchmark run does not fetch pages or packages.

## Run

Run all fixtures:

```bash
node scripts/compare-extractors/index.mjs --all
```

Run one fixture:

```bash
node scripts/compare-extractors/index.mjs --fixture technical-doc
```

Write a machine-readable report to another path:

```bash
node scripts/compare-extractors/index.mjs --all --json target/quality-report.json
node scripts/compare-extractors/index.mjs --all --summary benchmarks/quality/BASELINE.md
```

The runner always writes per-fixture Markdown, one `result.json` record per fixture, and an aggregate `report.json` under `target/quality-comparison/`. Use `--output` to select another artifact directory. Use `--fixture-root` for a compatible local corpus.

The JSON report records the Legible commit, dirty-worktree state, dirty diff hash, and pinned Defuddle version. It reports content recall, noise rejection, structure, metadata, reference token scores, reliability, panic and tool-failure counts, and diagnostic counts. It keeps these dimensions separate. Phrase and reference-token checks use visible Markdown text and ignore link destinations. Structural checks can cover heading levels, figures, and list-encoded reply depth.

See `benchmarks/quality/README.md` for the fixture schema, tokenization rules, quality definitions, and instructions for adding a case.

## External comparator

Set `DEFUDDLE_COMMAND` to replace the pinned wrapper with another local command. `DEFUDDLE_ARGS` must be a JSON array. The command receives the source path as its last argument and the source URL in `LEGIBLE_SOURCE_URL`. It can write plain Markdown or JSON with `markdown`, `word_count`, `tables`, and `metadata` fields.

```bash
DEFUDDLE_COMMAND="node" \
  DEFUDDLE_ARGS='["/path/to/defuddle-wrapper.mjs"]' \
  node scripts/compare-extractors/index.mjs --all
```
