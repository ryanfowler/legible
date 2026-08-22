# Compact DOM payload measurements

Date: 2026-08-22

The compact representation stores common HTML element names as tag keys and
common unqualified attributes as small typed keys. Custom, foreign, mixed-case,
and qualified names stay in per-DOM pools so the parser and serializer preserve
their exact names.

The payload sizes are 32 bytes for `Attribute` and 48 bytes for `ElementData`.
The attribute-heavy storage regression test measures the actual arena and
attribute capacities, including the auxiliary name pools. On the test input,
the compact representation used 71,184 bytes versus 72,208 bytes for the
equivalent legacy payload layout.

The real-world benchmark used a baseline captured immediately before this
change and ran only with `cargo bench --bench real_world`. Extraction medians
changed from +0.35% to +9.22%, and raw HTML to Markdown medians changed from
+1.47% to +8.07%. The elapsed-time results do not show a speed improvement;
the change is accepted as a memory-focused optimization because the measured
DOM storage is lower and every change remains below the repository's 15%
regression guardrail.
