# Performance benchmarks

## Development check

Run the small benchmark set after performance-sensitive changes:

```bash
cargo bench --bench smoke
```

It measures medium and large article extraction, end-to-end Markdown
extraction, and lazy Markdown rendering. It uses short Criterion windows, so
use it to catch large regressions, not to make final performance claims.

The full suite is intentionally slower. Run it for focused performance work,
baseline updates, or before changing a performance-sensitive implementation:

```bash
cargo bench --bench extraction
```

The full suite measures these workloads:

- small, medium, and large prose extraction
- medium and large ordinary-inline article extraction with normal inline markup
- large documentation pages
- syntax-highlighted code pages
- KaTeX and MathML pages
- table-heavy pages
- listing and malformed pages
- metadata-heavy pages
- large Guardian and Wikipedia compatibility fixtures
- retained-fragment lowering to a semantic document
- source DOM, actual final pre-IR DOM, IR node, and retained-byte counts in extraction benchmark output
- semantic compression counts in lowering benchmark IDs
- retained-byte, semantic-string, root, and node-layout estimates in lowering benchmark output; the semantic text arena counts as one owned string value
- benchmark-only compact preorder and event-tape representation prototypes
- end-to-end raw HTML to Markdown extraction
- steady-state lazy Markdown, text, and HTML rendering (text statistics are prewarmed)
- deeply nested parser input

See `benches/complex-temporary-storage.md` for the storage inventory and the
final complex-compiler benchmark measurements.

The `ordinary-inline` workload uses repeated article sections with `strong`, `em`,
links, inline `code`, native lists, blockquotes, simple image figures,
`details`/`summary`, and definition lists. It avoids tables, footnotes, math,
responsive images, callouts, and syntax-highlighter markup. The same workload is
available in extraction, retained-fragment lowering, end-to-end Markdown, and
lazy Markdown benchmarks so these phases can be compared directly.

The retained-fragment lowering group isolates semantic compilation from extraction
and output rendering. Its generated fragments represent cleaned content regions
without page chrome. Source evidence and shared cleanup facts are prepared before
timing, as they are in production extraction.

The `compact_ir_prototype` group records the pre-migration arena comparison
as historical data. It compares the production compatibility view with
benchmark-only preorder-node and open/close event-tape adapters. The production
document now uses the event-tape layout. It measures Markdown, HTML, and text
projections from the same semantic fixtures. The
fixtures include semantic payload fields and repeated footnote references. See
`benches/compact-ir-prototype.md` for the layout decision and measurements.

Lazy output groups measure steady-state rendering. Text statistics are initialized
before timing so their one-time cache initialization is not mixed into the render. The complex groups cover highlighted code, tables, math,
footnotes, documentation, listings, malformed markup, metadata, JSON-LD, and large
compatibility fixtures.

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
- Lowering benchmark IDs record selected DOM and semantic IR node counts as `dom-N-ir-N`. The benchmark also prints a `representation/...` line with event-operation capacity, estimated retained bytes, semantic string bytes, root counts, and an estimate for source-sized operation and end-index reservations. The estimates include vector capacity and owned semantic strings. Use these values to compare retained-representation compression without changing stable Criterion benchmark IDs.
- Extraction benchmarks print an `extraction-representation/...` line from opt-in diagnostics. It reports source DOM nodes, the actual selected and cleaned pre-IR DOM nodes, semantic document nodes, and estimated retained bytes for the same input.
- A normalization change must not add a repeated full-document scan for each code block, equation, table, or image.

Absolute time limits are not stable across machines. Keep Criterion reports or CI benchmark artifacts when a change intentionally adjusts a baseline. The current baseline at revision `499e09f3bf2e53164321e991254b9ff124cccb59` is recorded in `benches/private-ir-baseline.md`.
