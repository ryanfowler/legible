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
- retained source DOM to semantic document compilation
- source DOM, actual final pre-IR DOM, IR node, and retained-byte counts in extraction benchmark output
- semantic compression counts in compiler benchmark IDs
- retained-byte estimates in compiler benchmark output
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
- Compiler benchmark IDs record selected DOM and semantic IR node counts as `dom-N-ir-N`. The benchmark also prints a `representation/...` line with IR capacity, estimated retained bytes, and the equivalent estimate for a source-sized arena reservation. The estimates include vector capacity and owned semantic strings. Use these values to compare retained-representation compression without changing stable Criterion benchmark IDs.
- Extraction benchmarks print an `extraction-representation/...` line from opt-in diagnostics. It reports source DOM nodes, the actual selected and cleaned pre-IR DOM nodes, semantic document nodes, and estimated retained bytes for the same input.
- A normalization change must not add a repeated full-document scan for each code block, equation, table, or image.

Absolute time limits are not stable across machines. Keep Criterion reports or CI benchmark artifacts when a change intentionally adjusts a baseline.
