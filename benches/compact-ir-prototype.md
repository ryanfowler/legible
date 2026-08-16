# Compact semantic representation prototype

This note records the compact representation experiment that selected the
production layout. The production semantic document now uses the event tape
layout described below. The recorded pre-migration measurements remain as
historical data. The current benchmark compares the production compatibility
view with the preorder alternative.

## Layouts

The prototype adapts the pre-migration private semantic arena into two
layouts. The current production compatibility view is also measured against
these layouts. Both layouts use the same side payload table and the same owned
payload values.
This keeps the comparison focused on traversal and structural storage. Text is
not yet stored in a shared arena.

### Preorder nodes

```text
#[repr(C)]
struct PreorderNode {
    subtree_end: u32,
    payload: u32,
    aux: u16,
    kind: u8,
    flags: u8,
}
```

A node's descendants occupy `[index + 1, subtree_end)`. The renderer keeps a
small stack of end positions and emits a close action when it reaches an end.
The layout has no sibling links and no root vector.

### Event tape

```text
#[repr(C)]
struct Op {
    payload: u32,
    aux: u16,
    opcode: u8,
    flags: u8,
}
```

Container nodes emit an open operation and a close operation. Leaf nodes emit
one operation. The opcode stores the semantic kind and the open or close bit.
The renderer scans operations in order and does not maintain subtree state.

## Measured layout sizes

The benchmark was run on `DO-Premium-Intel` with Rust 1.97.1.

| Layout | Header size |
|---|---:|
| Pre-migration `ArenaNode` | 80 bytes |
| Pre-migration `NodeKind` | 64 bytes |
| Preorder node | 12 bytes |
| Event operation | 8 bytes |
| Shared prototype payload slot | 64 bytes |

The payload table is shared by both candidates. It contains only nodes that
carry uncommon values. Unit semantic nodes do not allocate a payload slot.
Footnote payloads store an ID. Each candidate keeps one separate footnote-label
table, so repeated references do not duplicate the label string.

The pre-migration arena used owned `String` values for text and its existing
estimate included string capacity. The prototype uses exact-sized boxed payload strings.
The report provides non-string bytes and string bytes separately. Use the
non-string values for the structural comparison. The total values are useful
for retained-storage planning, but they are not a header-only comparison.

## Representation measurements

The following values are representative retained-fragment measurements from
the prototype run. They include vector capacity and owned payload strings.

| Fixture | Arena | Preorder | Events | Semantic nodes | Event operations |
|---|---:|---:|---:|---:|---:|
| simple prose | 16,934 | 9,838 | 9,934 | 168 | 264 |
| ordinary inline | 256,867 | 161,139 | 161,663 | 2,733 | 4,165 |
| highlighted code | 144,818 | 88,762 | 88,762 | 1,496 | 2,244 |
| table-heavy | 905,974 | 668,214 | 676,334 | 10,150 | 16,240 |
| math | 354,358 | 231,576 | 228,328 | 4,060 | 5,684 |
| semantic payloads | 483,758 | 321,798 | 325,254 | 5,616 | 8,856 |
| footnotes | 403,406 | 245,246 | 243,702 | 3,474 | 5,018 |
| documentation | 624,106 | 393,234 | 397,602 | 6,188 | 9,828 |

## Traversal result

The benchmark measures a renderer-shaped semantic projection for Markdown,
HTML, and text. It compares the production compatibility view with both
sequential layouts. The recorded arena values below are from the pre-migration
run. It verifies that all three projections produce identical output for
each fixture before timing. The projections preserve the semantic fields used
by the current document, including heading levels, link fragment flags, image
dimensions, table metadata, callout kinds, code, math, media, and footnote
labels.

These are traversal projections, not the production renderers. They keep the
comparison fair because both candidate layouts implement the same operation
mix. They do not prove final Markdown or HTML speed. The next renderer work
must compare complete output-compatible interpreters with the current
renderers.

The event tape was faster in most measured projections, but it was not faster
every time. The ordinary-inline midpoint results in the recorded run were
about 54 us, 261 us, and 57 us for event Markdown, HTML, and text. The
corresponding preorder results were about 91 us, 281 us, and 85 us. The event
tape also reduced the simple-prose Markdown projection from about 6.2 us for
the pre-migration arena to about 3.1 us for events. Results vary with
fixture and output format.

## Decision

Use the **event tape** as the production semantic representation.

It gives the simplest sequential renderer loop and the strongest measured
traversal result. Its additional close operations increase structural item
count, but the retained bytes were within about 2% of the preorder layout on
the measured fixtures. The current builder retains type-specific payload
side tables and an internal end index for compatibility consumers. A later
renderer change can consume the operations directly and remove that index.

The benchmark also includes `build-preorder` and `build-events` measurements.
These include the benchmark adapter and payload lowering. They are separate
from the steady-state traversal measurements. The report includes payload-slot
counts, payload-slot bytes, string bytes, string-value counts, footnote slots,
non-string representation bytes, and logical stack peaks. It also reports the
initial and grown `Vec` capacity estimates for the arena task stack, preorder
close stack, and event builder stack. The event renderer uses no traversal
stack; its explicit close operations carry the close payload metadata.

The prototype does not decide the text-storage design. A separate experiment
must measure a contiguous canonical text arena.

## Reproduction

```bash
cargo bench --bench extraction -- compact_ir_prototype --noplot
```

The benchmark prints layout sizes, retained bytes, structural bytes, payload
slot counts and bytes, string bytes and value counts, stack accounting, and
Criterion timings. It runs field-level projection assertions and a 10,000-level
stack-safety assertion before timing. Its fixtures cover ordinary prose, inline
semantics, highlighted code, tables, math, semantic payload fields, footnotes,
and documentation.
