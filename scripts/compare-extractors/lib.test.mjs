import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  aggregateReport,
  comparisonResult,
  discoverFixtures,
  formatMarkdownSummary,
  markdownVisibleText,
  normalizeText,
  outputMetrics,
  referenceScores,
  scoreExtraction,
  validateManifest,
} from "./lib.mjs";

function fixture(overrides = {}) {
  return {
    manifest: {
      id: "sample",
      category: "technical-documentation",
      source_url: "https://example.test/docs",
      page_shape: "document",
      must_include: ["Required phrase"],
      must_exclude: ["Subscription prompt"],
      expected: {
        headings_min: 1,
        code_blocks_min: 1,
        tables_min: 1,
      },
      notes: "A sample fixture.",
      ...overrides.manifest,
    },
    reference: overrides.reference ?? null,
    expectedMetadata: overrides.expectedMetadata ?? null,
  };
}

test("normalizes Unicode and tokenizes letters and numbers", () => {
  assert.equal(normalizeText("  CAFÉ\nConfiguration  "), "café configuration");
  assert.deepEqual(referenceScores("café setting 2", "Café setting 2"), {
    precision: 1,
    recall: 1,
    f1: 1,
  });
});

test("uses visible Markdown text for phrase checks", () => {
  const markdown =
    "Required **inline** `config_value` with [safe text](https://example.test/a_(nested)/campaign-noise).";
  assert.equal(
    normalizeText(markdownVisibleText(markdown)),
    "required inline config_value with safe text.",
  );
  const result = scoreExtraction(
    { success: true, deterministic: true, markdown },
    fixture({
      manifest: {
        must_include: ["Required inline config_value"],
        must_exclude: ["campaign-noise"],
        expected: {},
      },
    }),
  );
  assert.equal(result.required_content_score, 1);
  assert.equal(result.noise_score, 1);
});

test("counts documented Markdown structures", () => {
  const markdown = `# Heading

- first
- second

\`\`\`js
const value = 1;
\`\`\`

| Name | Value |
| --- | --- |
| one | 1 |

![Alt](image.png) [Link](page.html)

[^one]: A note.

$x + y$`;
  assert.deepEqual(outputMetrics(markdown), {
    words: 22,
    links: 1,
    link_density: 4 / markdown.length,
    headings: 1,
    images: 1,
    image_sources: ["image.png"],
    code_blocks: 1,
    tables: 1,
    figures: 0,
    lists: 1,
    footnotes: 1,
    math: 1,
    reply_items: 0,
    reply_depth: 0,
    headings_h1: 1,
    headings_h2: 0,
    headings_h3: 0,
    headings_h4: 0,
    headings_h5: 0,
    headings_h6: 0,
    junk_phrases: [],
    metadata: null,
    semantic_coverage: null,
  });
});

test("scores reference tokens from visible Markdown text", () => {
  assert.deepEqual(
    referenceScores(
      "Read [the guide](https://one.example/path).",
      "Read [the guide](https://two.example/other-path).",
    ),
    { precision: 1, recall: 1, f1: 1 },
  );
});

test("matches fenced code blocks by character and minimum length", () => {
  const markdown = `# Outside

\`\`\`\`markdown
\`\`\`
# Inside
~~~
\`\`\`\`

~~~text
content
~~~~`;
  const metrics = outputMetrics(markdown);
  assert.equal(metrics.code_blocks, 2);
  assert.equal(metrics.headings, 1);
});

test("counts heading levels and ordered-list reply structure", () => {
  const metrics = outputMetrics(`# Thread

## Replies

1. first
   1. nested
      1. deep
1. second`);
  assert.equal(metrics.headings_h1, 1);
  assert.equal(metrics.headings_h2, 1);
  assert.equal(metrics.reply_items, 4);
  assert.equal(metrics.reply_depth, 3);
});

test("does not count a rich bullet list as replies", () => {
  const metrics = outputMetrics(`1. Dana
   - Read the specification
   - Inspect the implementation
1. Jules`);
  assert.equal(metrics.reply_items, 2);
  assert.equal(metrics.reply_depth, 1);
});

test("uses explicit reply markers instead of an ordered rich list", () => {
  const result = scoreExtraction(
    {
      success: true,
      deterministic: true,
      markdown: `1. **Dana**
   1. Prepare the archive
      1. Verify its digest
1. **Jules**`,
    },
    fixture({
      manifest: {
        must_include: ["Dana", "Jules"],
        must_exclude: [],
        reply_markers: ["Dana", "Jules"],
        expected: { reply_items_min: 2, reply_items_max: 2 },
      },
    }),
  );
  assert.equal(result.reply_items, 2);
  assert.equal(result.reply_depth, 1);
  assert.equal(result.structure_score, 1);
});

test("finds continuation-line markers with exact token boundaries", () => {
  const result = scoreExtraction(
    {
      success: true,
      deterministic: true,
      markdown: `1.
   **Moderator notice**
1. **Bo**
1. A bounded procedure`,
    },
    fixture({
      manifest: {
        must_include: ["Moderator notice", "Bo"],
        must_exclude: [],
        reply_markers: ["Moderator notice", "Bo"],
        expected: { reply_items_min: 2, reply_items_max: 2 },
      },
    }),
  );
  assert.equal(result.reply_items, 2);
  assert.equal(result.structure_score, 1);
});

test("checks exact retained image sources", () => {
  const result = scoreExtraction(
    {
      success: true,
      deterministic: true,
      markdown: "![Map](https://example.test/placeholder.gif)",
    },
    fixture({
      manifest: {
        expected: { images_min: 1 },
        must_include_image_sources: ["https://example.test/map.png"],
        must_exclude_image_sources: ["https://example.test/placeholder.gif"],
      },
    }),
  );
  assert.equal(result.structure_score, 1 / 3);
  assert.deepEqual(result.image_sources_missing, [
    "https://example.test/map.png",
  ]);
  assert.deepEqual(result.excluded_image_sources_found, [
    "https://example.test/placeholder.gif",
  ]);
});

test("distinguishes inline math from currency", () => {
  assert.equal(outputMetrics("Use $x + y$ in this equation.").math, 1);
  assert.equal(outputMetrics("Use $x$ and $y$ in these equations.").math, 2);
  assert.equal(outputMetrics("The plans cost $5 and $10 today.").math, 0);
  assert.equal(outputMetrics("Save $5 when you spend $25.").math, 0);
  assert.equal(outputMetrics("The range is $5–$10.").math, 0);
  assert.equal(outputMetrics("$$\n$x$\n$$").math, 1);
});

test("scores requirements independently from comparator output", () => {
  const result = scoreExtraction(
    {
      success: true,
      deterministic: true,
      markdown: `# Required phrase

\`\`\`
code
\`\`\`

| A | B |
| --- | --- |
| 1 | 2 |`,
      metadata: { title: " Sample title " },
    },
    fixture({ expectedMetadata: { title: "sample title" } }),
  );
  assert.equal(result.required_content_score, 1);
  assert.equal(result.noise_score, 1);
  assert.equal(result.structure_score, 1);
  assert.equal(result.metadata_score, 1);
  assert.equal(result.reliability_pass, true);
});

test("does not reward an unexpected extraction failure", () => {
  const result = scoreExtraction(
    { success: false, deterministic: null, error: "No content" },
    fixture({
      reference: "Required phrase",
      expectedMetadata: { title: "Sample title" },
    }),
  );
  assert.equal(result.required_content_score, 0);
  assert.equal(result.noise_score, 0);
  assert.equal(result.structure_score, 0);
  assert.equal(result.metadata_score, 0);
  assert.deepEqual(result.reference, { precision: 0, recall: 0, f1: 0 });
  assert.equal(result.reliability_pass, false);
});

test("treats a deterministic expected extraction failure as not applicable", () => {
  const result = scoreExtraction(
    {
      success: false,
      failure_kind: "extraction",
      deterministic: true,
      error: "No content",
      error_details: { variant: "NoContent" },
    },
    fixture({
      manifest: { expected_failure: true, expected_error: "NoContent" },
      reference: "Required phrase",
      expectedMetadata: { title: "Sample title" },
    }),
  );
  assert.equal(result.reliability_pass, true);
  assert.equal(result.required_content_score, null);
  assert.equal(result.noise_score, null);
  assert.equal(result.structure_score, null);
  assert.equal(result.metadata_score, null);
  assert.equal(result.reference, null);
});

test("rejects a tool failure for an expected-failure fixture", () => {
  const result = scoreExtraction(
    {
      success: false,
      failure_kind: "panic",
      deterministic: true,
      error: "thread 'main' panicked",
    },
    fixture({
      manifest: { expected_failure: true, expected_error: "NoContent" },
    }),
  );
  assert.equal(result.reliability_pass, false);
  assert.equal(result.panic, true);
});

test("rejects the wrong extraction error variant", () => {
  const result = scoreExtraction(
    {
      success: false,
      failure_kind: "extraction",
      deterministic: true,
      error: "No body",
      error_details: { variant: "NoBody" },
    },
    fixture({
      manifest: { expected_failure: true, expected_error: "NoContent" },
    }),
  );
  assert.equal(result.reliability_pass, false);
  assert.equal(result.error_variant, "NoBody");
});

test("excludes an unexpected success from expected-failure quality scores", () => {
  const result = scoreExtraction(
    { success: true, deterministic: true, markdown: "Gate text" },
    fixture({
      manifest: { expected_failure: true, expected_error: "NoContent" },
      reference: "Expected no result",
      expectedMetadata: { title: "Expected no result" },
    }),
  );
  assert.equal(result.reliability_pass, false);
  assert.equal(result.required_content_score, null);
  assert.equal(result.noise_score, null);
  assert.equal(result.structure_score, null);
  assert.equal(result.metadata_score, null);
  assert.equal(result.reference, null);
});

test("enforces the committed manifest schema", () => {
  const manifest = { ...fixture().manifest, unsupported_field: true };
  assert.throws(
    () => validateManifest(manifest, "/tmp/sample"),
    /must NOT have additional properties/u,
  );
});

test("discovers fixtures in stable id order", () => {
  const root = mkdtempSync(join(tmpdir(), "legible-quality-"));
  for (const id of ["second", "first"]) {
    const directory = join(root, id);
    mkdirSync(directory);
    writeFileSync(join(directory, "source.html"), "<main>Content</main>");
    writeFileSync(
      join(directory, "manifest.json"),
      JSON.stringify({
        id,
        category: "blog-post",
        source_url: `https://example.test/${id}`,
        page_shape: "article",
        must_include: ["Content"],
        must_exclude: [],
        notes: "Discovery order coverage.",
      }),
    );
  }
  assert.deepEqual(
    discoverFixtures(root).map((item) => item.manifest.id),
    ["first", "second"],
  );
});

test("builds a stable aggregate with comparison rankings", () => {
  const sample = fixture();
  const result = comparisonResult(
    sample,
    {
      success: true,
      deterministic: true,
      markdown:
        "# Required phrase\n\n```\ncode\n```\n\n| A | B |\n| --- | --- |",
    },
    {
      success: true,
      deterministic: true,
      markdown: "# Other phrase\n\n```\ncode\n```\n\n| A | B |\n| --- | --- |",
    },
  );
  const report = aggregateReport([result], {
    legible: "abc123",
    defuddle: "npm:defuddle@0.19.2",
  });
  assert.equal(report.schema_version, 1);
  assert.equal(report.aggregate.legible.required_content_score, 1);
  assert.equal(report.by_category["technical-documentation"].fixture_count, 1);
  assert.equal(report.largest_legible_wins[0].fixture, "sample");
  assert.ok(Math.abs(report.largest_legible_wins[0].delta - 1 / 3) < 1e-12);
});

test("retains diagnostics-only semantic coverage in fixture reports", () => {
  const result = comparisonResult(
    fixture(),
    {
      success: true,
      deterministic: true,
      markdown: "Required phrase",
      semantic_coverage: {
        score: 0.5,
        categories: [
          {
            category: "code_blocks",
            source_count: 2,
            result_count: 1,
            coverage: 0.5,
          },
        ],
      },
    },
    { success: true, deterministic: true, markdown: "Required phrase" },
  );

  assert.equal(result.legible.semantic_coverage.score, 0.5);
  assert.equal(result.defuddle.semantic_coverage, null);
});

test("formats a compact deterministic Markdown summary", () => {
  const sample = fixture();
  const result = comparisonResult(
    sample,
    { success: true, deterministic: true, markdown: "Required phrase" },
    { success: true, deterministic: true, markdown: "Other phrase" },
  );
  const report = aggregateReport([result], {
    legible: { commit: "abc123", dirty: false, diff_sha256: null },
    defuddle: "npm:defuddle@0.19.2",
  });
  const summary = formatMarkdownSummary(report);
  assert.match(summary, /This curated corpus/u);
  assert.match(summary, /Fixtures: 1/u);
  assert.match(summary, /Legible revision: `abc123`/u);
  assert.match(summary, /npm:defuddle@0\.19\.2/u);
  assert.match(summary, /technical-documentation/u);
  assert.match(summary, /Largest comparator gaps: none/u);
  assert.match(summary, /Legible reliability failures: none/u);
  assert.match(summary, /Defuddle reliability failures: none/u);
});

test("puts the baseline-excluding worktree hash in the summary", () => {
  const report = aggregateReport([], {
    legible: {
      commit: "abc123",
      dirty: true,
      diff_sha256: "stable-corpus-hash",
    },
    defuddle: "npm:defuddle@0.19.2",
  });
  const summary = formatMarkdownSummary(report);
  assert.match(summary, /abc123 \(dirty corpus stable-corpus-hash\)/u);
});

test("lists reliability failures for both extractors", () => {
  const sample = fixture();
  const result = comparisonResult(
    sample,
    {
      success: false,
      failure_kind: "tool",
      deterministic: true,
      error: "Legible tool failed",
    },
    {
      success: false,
      failure_kind: "tool",
      deterministic: true,
      error: "Defuddle tool failed",
    },
  );
  const summary = formatMarkdownSummary(
    aggregateReport([result], {
      legible: "abc123",
      defuddle: "npm:defuddle@0.19.2",
    }),
  );
  assert.match(summary, /Legible reliability failures: sample/u);
  assert.match(summary, /Defuddle reliability failures: sample/u);
});
