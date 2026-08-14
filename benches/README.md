# Performance benchmarks

Run the full suite with:

```bash
cargo bench --bench extraction
```

The suite measures these workloads:

- small, medium, and large prose extraction
- large documentation pages
- syntax-highlighted code pages
- KaTeX and MathML pages
- table-heavy pages
- listing and malformed pages
- metadata-heavy pages
- large Guardian and Wikipedia compatibility fixtures
- end-to-end raw HTML to Markdown extraction
- lazy Markdown, text, and HTML rendering
- deeply nested parser input

Criterion stores local baselines in `target/criterion`. Use a named baseline before a substantial pipeline change:

```bash
cargo bench --bench extraction -- --save-baseline main
cargo bench --bench extraction -- --baseline main
```

## Guardrails

Use the same machine, Rust toolchain, and power mode for comparisons.

- Investigate a reproducible median regression greater than 10%.
- Do not merge a regression greater than 15% without a documented reason.
- Doubling generated input size should take less than 2.5 times as long.
- Deeply nested parser inputs must remain linear and must not overflow the stack.
- Extraction-only benchmarks must not render Markdown, HTML, or text. The end-to-end Markdown group measures rendering explicitly. Output rendering stays lazy.
- A normalization change must not add a repeated full-document scan for each code block, equation, table, or image.

Absolute time limits are not stable across machines. Keep Criterion reports or CI benchmark artifacts when a change intentionally adjusts a baseline.
