# Defuddle compatibility fixtures

This directory contains minimized compatibility fixtures derived from generic behavior in the Defuddle test corpus. Defuddle is available at <https://github.com/kepano/defuddle> under the MIT license.

Each runnable fixture contains `source.html` and `expected.md`. A metadata fixture can also contain `metadata.json`. The Rust test harness runs without Node.js.

The top-level directories identify the likely owner of a failure:

| Directory | Primary owner |
|---|---|
| `code-blocks/` | normalization |
| `content-patterns/` | cleanup |
| `elements/` | normalization or Markdown |
| `footnotes/` | normalization |
| `headings/` | normalization |
| `hidden/` | cleanup |
| `images/` | normalization |
| `math/` | normalization |
| `metadata/` | metadata |
| `table-layout/` | normalization |
| `general/` | selection |

Run all fixtures with:

```bash
cargo test --test fixture_tests compatibility-defuddle
```
