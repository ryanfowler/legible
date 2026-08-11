# Differential fixture comparison

This maintainer tool runs Legible against `tests/web/` and writes Markdown and basic metrics to `target/defuddle-comparison/`.

```bash
node scripts/compare-defuddle/index.mjs
```

Set `DEFUDDLE_COMMAND` to also run a local Defuddle wrapper. The command must accept the source file path as its final argument. It can write Markdown to standard output. It can instead write JSON with `markdown` and `metadata` fields to include metadata agreement.

```bash
DEFUDDLE_COMMAND='node' \
  DEFUDDLE_ARGS='["/path/to/defuddle-wrapper.mjs"]' \
  node scripts/compare-defuddle/index.mjs
```

The script does not install Node packages. The Rust crate has no Node or Defuddle dependency.
