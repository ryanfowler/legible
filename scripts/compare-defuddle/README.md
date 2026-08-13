# Differential quality runner

This maintainer tool runs Legible and a pinned Defuddle release against the independent fixtures in `benchmarks/quality/`. It scores both extractors against each fixture manifest. Defuddle output is a comparison. It is not the expected answer.

## Install

Install the exact versions from `package-lock.json`:

```bash
npm --prefix scripts/compare-defuddle ci
cargo fetch
```

The runner pins `defuddle` to version 0.19.2. It disables Defuddle asynchronous enrichment and invokes Cargo with `--offline`. After the Node and Rust dependencies are present, a benchmark run does not fetch pages or packages.

## Run

Run all fixtures:

```bash
node scripts/compare-defuddle/index.mjs --all
```

Run one fixture:

```bash
node scripts/compare-defuddle/index.mjs --fixture technical-doc
```

Write a machine-readable report to another path:

```bash
node scripts/compare-defuddle/index.mjs --all --json target/quality-report.json
```

The runner always writes per-fixture Markdown, one `result.json` record per fixture, and an aggregate `report.json` under `target/quality-comparison/`. Use `--output` to select another artifact directory. Use `--fixture-root` for a compatible local corpus.

The JSON report records the Legible commit, dirty-worktree state, dirty diff hash, and pinned Defuddle version. It reports content recall, noise rejection, structure, metadata, reference token scores, reliability, panic and tool-failure counts, and diagnostic counts. It keeps these dimensions separate. Phrase and reference-token checks use visible Markdown text and ignore link destinations. Structural checks can cover heading levels, figures, and list-encoded reply depth.

See `benchmarks/quality/README.md` for the fixture schema, tokenization rules, quality definitions, and instructions for adding a case.

## External comparator

Set `DEFUDDLE_COMMAND` to replace the pinned wrapper with another local command. `DEFUDDLE_ARGS` must be a JSON array. The command receives the source path as its last argument and the source URL in `LEGIBLE_SOURCE_URL`. It can write plain Markdown or JSON with `markdown`, `word_count`, `tables`, and `metadata` fields.

```bash
DEFUDDLE_COMMAND="node" \
  DEFUDDLE_ARGS='["/path/to/defuddle-wrapper.mjs"]' \
  node scripts/compare-defuddle/index.mjs --all
```
