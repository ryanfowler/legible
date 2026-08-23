# Evaluations

Evaluations measure broad extraction quality. They do not define exact output
and they do not measure execution time.

`quality/` contains independent fixture manifests. Run them with the evaluator:

```bash
npm --prefix tools/extractor-eval ci
cargo fetch
node tools/extractor-eval/index.mjs --all
```

See `quality/README.md` for the fixture format and scoring rules. See
`../tools/extractor-eval/README.md` for runner options.
