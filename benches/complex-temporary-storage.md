# Complex compiler storage measurements

This note records the storage review for the private semantic compiler.
Run the measurements with:

```bash
cargo bench --bench extraction -- complex_pages --baseline before-task
cargo bench --bench extraction -- lower_retained_fragment --baseline before-task
```

The lowering command prints `complex-storage/...` lines. These lines report
the tracked analysis capacities held before lowering, the one lowering pass,
and the conditional media-separator pass. They exclude allocator metadata,
parser buffers, and cleanup buffers.

The named baseline was captured at the start of the storage change on the
same machine and Rust toolchain. Criterion reports the statistical change.
Representative before and final medians were:

| Workload | Before median | Final median | Criterion result |
|---|---:|---:|---:|
| prose | 36.543 ms | 27.448 ms | improved |
| documentation | 88.340 ms | 83.138 ms | no significant change |
| footnotes-reference | 59.161 ms | 52.871 ms | improved |
| highlighted-code | 91.786 ms | 61.258 ms | improved |
| math | 76.468 ms | 64.822 ms | improved |
| table-heavy | 75.289 ms | 63.199 ms | improved |
| listing | 52.578 ms | 41.562 ms | improved |
| malformed | 198.10 ms | 180.25 ms | no significant change |
| metadata-heavy | 40.862 ms | 32.150 ms | no significant change |
| json-ld-heavy | 28.844 ms | 27.496 ms | no significant change |
| media-heavy | new workload | 37.517 ms | guardrail baseline |

The retained-fragment lowering group showed improvement for prose,
ordinary-inline pages, math, and footnotes. The code, table, and documentation
cases were within Criterion noise in the final focused run. This checks that
sparse payloads do not make the ordinary compiler slow.

## Storage decisions

The old source-sized result vectors were replaced with sorted sparse values or
node sets for callouts, image payloads and synthetic containers, media payloads,
math payloads and skipped nodes, footnote labels, and media separator evidence.
The old table-analysis map is now an active-table stack. Deferred captions use
feature-local buckets keyed by their active semantic wrapper, so caption
handling stays linear. Callout title text uses one bounded reverse pass.

The sparse representation stores only `K` recognized nodes as `(NodeId,
payload)` entries, or as `NodeId` entries for a set. The old representation
reserved a slot for every source node `N` even when the feature was absent.
The source-sized result payloads therefore change from `O(N)` payload slots to
`O(K)` payload slots. Image analysis moves sources directly into sparse
entries. Math analysis keeps only small boolean worklists while it recognizes
expressions. Media analysis moves source payloads into sparse entries and
drops the source worklist before sorting the result. Dense indexes remain for
hot arbitrary-node lookups: footnote flags, shared semantic facts, and the
figure/caption flags. The compiler checks figure and caption flags for every
source node; focused lowering measurements showed that replacing those hot
booleans with binary-search sparse sets was slower. Sparse payloads build a
dense slot index only when their density justifies the extra index. Media
separator sets use either their sorted node vector or a compact bitset, not
both.

The complex compiler still uses one shared source-facts inventory, one lowering
task traversal, and one conditional media-separator traversal. It does not add
a post-build semantic scan. The `representation/...` lines printed by the
benchmark record retained tape bytes and source-sized reservation estimates.
The final highlighted-code, math, table-heavy, documentation, and footnotes
fixtures report respectively 185,374, 292,408, 378,454, 299,910, and 192,570
bytes of source-sized reservation in the retained representation. The
`complex-storage/...` lines report the tracked analysis working-set capacities
for the same fixtures. Representative tracked values are 426,450 bytes for
math (8,527 source nodes), 78,014 bytes for footnotes (4,247 source nodes),
86,978 bytes for ordinary-inline-large (27,911 source nodes), and 11,226 bytes
for highlighted-code (5,611 source nodes). Callout text keeps its
source-sized scratch ranges because the reverse pass computes every bounded
subtree prefix once; this preserves linear behavior for nested candidates. The
storage inventory above explains which transient source-indexed fields were
removed or deliberately retained.
