# Extraction quality evaluation

This evaluation measures extraction quality against independent fixture expectations. It does not treat Defuddle output as ground truth. The runner scores Legible and Defuddle against the same requirements.

## Fixture layout

Each fixture uses this layout:

```text
evals/quality/<fixture-id>/
  source.html
  manifest.json
  reference.md       # optional
  metadata.json      # optional
```

`schema.json` defines the manifest fields and category names. The fixture directory name and the manifest `id` must match. `source.html` must contain all input for the test. The runner does not fetch `source_url`.

Use `must_include` for known useful phrases. Use `must_exclude` for known clutter. The runner removes Markdown formatting and link destinations before phrase checks. It then normalizes Unicode, letter case, and whitespace. Use `expected` for minimum or maximum counts. Supported features include total headings, each heading level, lists, code blocks, tables, figures, images, footnotes, links, math expressions, reply items, and reply nesting depth. Reply metrics measure ordered-list-encoded replies. Use them only for discussion or thread fixtures that use that convention.

Use `must_include_image_sources` and `must_exclude_image_sources` when the selected image resource is part of the behavior under test. Values must match the Markdown image destination exactly. Set `reply_markers` to distinct reply metadata text when a fixture has reply structure limits. Reply metrics count only ordered-list items that contain one of those markers. Rich lists inside replies do not affect the metrics.

Add `reference.md` only when a complete, manually adjudicated result adds value. The runner converts the reference and result to visible Markdown text. It removes link destinations before it applies Unicode normalization, lowercase conversion, and sequences of Unicode letters or numbers. It uses token multiplicity to calculate precision, recall, and F1.

Add `metadata.json` for normalized metadata fields that have an unambiguous value. The runner compares only the fields in that file. It normalizes strings and string arrays before comparison.

Set `expected_failure` only when extraction must return a structured extraction error. Set `expected_error` to the required public error variant, such as `NoContent`. The runner does not score quality dimensions for a correct expected failure. A wrong error variant, panic, tool invocation error, unexpected failure, unexpected success, or non-deterministic result is a reliability failure.

## Quality dimensions

The report keeps each dimension separate:

- Content recall is the share of required phrases that appear.
- Noise rejection is the share of forbidden phrases that do not appear.
- Structural fidelity is the share of configured structure limits that pass. It can measure heading levels, figures, and list-encoded reply structure where applicable.
- Metadata accuracy is the share of configured metadata fields that match.
- Reference precision, recall, and F1 use normalized tokens when `reference.md` exists.
- Reliability records success, expected failure behavior, errors, panics, tool failures, and determinism.

The report also records words, links, link density, headings, images, code blocks, tables, lists, footnotes, math, and known junk phrases. Legible results also record candidate-to-result semantic coverage when the selected candidate has strong structural evidence. The semantic coverage record names each category and gives its source count, result count, and bounded ratio. This value is diagnostic data. It does not affect extraction acceptance.

Performance remains separate from quality. Use `cargo bench --bench pipeline` for extraction and lazy-renderer performance.

## Corpus categories

The schema supports news, blogs, essays, technical and API documentation, code-heavy pages, reference and academic pages, data tables, link indexes, legacy HTML, recipes, discussions, social threads, product support, modern application markup, inline peripheral UI, responsive duplicates, math, and media embeds.

The committed corpus contains 134 fixtures. It includes these curated batches:

- 27 article and essay cases cover news, blogs, long-form essays, multi-author reports, images, sidebars, newsletters, related stories, promotions, footnotes, and an access-barrier shell.
- 31 documentation and technical cases cover language and API references, code-heavy guides, callouts, source-present tabs, navigation trees, indexes, version selectors, and tables.
- 16 discussion and social-thread cases cover flat and nested replies, deleted parents, rich reply bodies, short threads, link-heavy threads, and reply-like clutter.
- 15 application and responsive-markup cases cover streamed fragments, source-present templates, hidden content, duplicate views, tabs, accordions, loading shells, access barriers, and empty client shells.
- 16 image and media cases cover lead images, responsive and lazy sources, captions, avatars, thumbnails, diagrams, SVG charts, duplicates, and embed fallbacks.
- 20 metadata-conflict cases cover title, author, date, canonical URL, representative image, tag, language, and section resolution.

All cases in these batches use original, minimized content written for this repository. They do not contain captured third-party pages or private data. Future batches must use the same redistribution and privacy rules.

## Add a real failure

1. Confirm that the supplied HTML contains the useful content.
2. Minimize the HTML while the failure still occurs.
3. Select the closest category and page shape.
4. Add required phrases and forbidden clutter from an independent review.
5. Add structural limits only when the structure is important.
6. Add `reference.md` only after a person adjudicates the full output.
7. Add `metadata.json` only for clear normalized values.
8. Explain the failure and its value in `notes`.
9. Run the focused fixture before changing extraction code.
10. Run the full evaluation and repository checks after the change.

Do not copy a third-party page when its license does not allow redistribution. Use a minimized reproduction or a local ignored corpus instead. Never commit private content or credentials.

## Run the evaluation

Install the pinned local tools once:

```bash
npm --prefix tools/extractor-eval ci
cargo fetch
```

Then run one fixture or all fixtures:

```bash
node tools/extractor-eval/index.mjs --fixture technical-doc
node tools/extractor-eval/index.mjs --all
node tools/extractor-eval/index.mjs --all --json target/quality-report.json
node tools/extractor-eval/index.mjs --all --summary evals/quality/BASELINE.md
```

Use `--summary` to write a compact category-level baseline. The summary includes
the fixture count, extractor revisions, quality dimensions, and reliability. It
does not include large per-fixture artifacts.

The runner invokes Cargo with `--offline`. After the Node and Rust dependencies are present, an evaluation run uses no network access.

Use the JSON report to inspect low semantic coverage:

```bash
jq -r '.results[] | select(.legible.semantic_coverage != null and .legible.semantic_coverage.score < 1) | [.fixture, .legible.semantic_coverage] | @json' target/quality-report.json
```

The initial calibration used the first 125 fixtures. It excluded high-confidence decorative and active content before it measured the selected candidate. It also required at least three source headings and three source list items. These limits removed false warnings from avatar, logo, newsletter, and duplicate-title fixtures. Named code, table, footnote, and math fixtures kept full coverage. Examples include `code-heavy-line-numbers`, `article-results-table`, and `essay-footnotes`. Do not make this score an acceptance signal until a later corpus review finds a stable threshold and a real extraction improvement.

The runner exits with a nonzero status when either extractor has a reliability failure. This rule also applies to an intentional comparator difference. For example, Legible correctly returns `NoContent` for the access-barrier shell, but the pinned Defuddle comparator returns the gate prompt. Inspect `report.json` to distinguish a Legible failure from a recorded comparator failure.
