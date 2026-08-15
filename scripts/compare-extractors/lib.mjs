import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const REPORT_SCHEMA_VERSION = 1;

const STRUCTURE_NAMES = [
  "headings",
  "lists",
  "code_blocks",
  "tables",
  "figures",
  "images",
  "footnotes",
  "links",
  "math",
  "reply_items",
  "reply_depth",
  "headings_h1",
  "headings_h2",
  "headings_h3",
  "headings_h4",
  "headings_h5",
  "headings_h6",
];

const JUNK_PHRASES = [
  "subscribe",
  "sign up",
  "share this article",
  "advertisement",
  "related articles",
];

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const manifestSchema = JSON.parse(
  readFileSync(
    resolve(scriptDirectory, "../../benchmarks/quality/schema.json"),
    "utf8",
  ),
);
const schemaValidator = new Ajv2020({ allErrors: true, strict: true });
addFormats(schemaValidator);
const validateAgainstSchema = schemaValidator.compile(manifestSchema);

export function normalizeText(value) {
  return String(value)
    .normalize("NFKC")
    .toLocaleLowerCase("en-US")
    .replace(/\s+/gu, " ")
    .trim();
}

export function tokenize(value) {
  return normalizeText(value).match(/[\p{L}\p{N}]+/gu) ?? [];
}

function closingDelimiter(value, start, opening, closing) {
  let depth = 1;
  for (let index = start + 1; index < value.length; index += 1) {
    if (value[index] === "\\") {
      index += 1;
      continue;
    }
    if (value[index] === opening) depth += 1;
    if (value[index] === closing) {
      depth -= 1;
      if (depth === 0) return index;
    }
  }
  return -1;
}

function isEscaped(value, index) {
  let slashes = 0;
  for (
    let cursor = index - 1;
    cursor >= 0 && value[cursor] === "\\";
    cursor -= 1
  ) {
    slashes += 1;
  }
  return slashes % 2 === 1;
}

function scanMarkdownLinks(markdown) {
  const visible = [];
  const imageSources = [];
  let links = 0;
  let images = 0;
  let linkedCharacters = 0;
  for (let index = 0; index < markdown.length; index += 1) {
    if (markdown[index] === "`" && !isEscaped(markdown, index)) {
      let runLength = 1;
      while (markdown[index + runLength] === "`") runLength += 1;
      const delimiter = "`".repeat(runLength);
      const end = markdown.indexOf(delimiter, index + runLength);
      if (end >= 0) {
        visible.push(markdown.slice(index, end + runLength));
        index = end + runLength - 1;
        continue;
      }
    }
    const image = markdown[index] === "!" && markdown[index + 1] === "[";
    const bracket = image ? index + 1 : index;
    if (markdown[bracket] !== "[" || isEscaped(markdown, bracket)) {
      visible.push(markdown[index]);
      continue;
    }
    const labelEnd = closingDelimiter(markdown, bracket, "[", "]");
    if (labelEnd < 0 || markdown[labelEnd + 1] !== "(") {
      visible.push(markdown[index]);
      continue;
    }
    const destinationEnd = closingDelimiter(markdown, labelEnd + 1, "(", ")");
    if (destinationEnd < 0) {
      visible.push(markdown[index]);
      continue;
    }
    const label = markdown.slice(bracket + 1, labelEnd);
    visible.push(label);
    if (image) {
      images += 1;
      imageSources.push(markdown.slice(labelEnd + 2, destinationEnd));
    } else {
      links += 1;
      linkedCharacters += label.length;
    }
    index = destinationEnd;
  }
  return {
    text: visible.join(""),
    links,
    images,
    imageSources,
    linkedCharacters,
  };
}

function countInlineMath(lines) {
  let count = 0;
  for (const line of lines) {
    let opening = -1;
    for (let index = 0; index < line.length; index += 1) {
      if (
        line[index] !== "$" ||
        isEscaped(line, index) ||
        line[index - 1] === "$" ||
        line[index + 1] === "$"
      ) {
        continue;
      }
      if (opening < 0) {
        if (!/\s/u.test(line[index + 1] ?? "")) opening = index;
        continue;
      }
      const previous = line[index - 1] ?? "";
      const next = line[index + 1] ?? "";
      if (!/\s/u.test(previous) && !/\p{N}/u.test(next)) {
        count += 1;
        opening = -1;
      }
    }
  }
  return count;
}

export function markdownVisibleText(markdown) {
  return scanMarkdownLinks(markdown)
    .text.replace(/^\s*(```|~~~)[^\n]*$/gmu, "")
    .replace(/^\s*\[[^\]]+\]:\s+\S+.*$/gmu, "")
    .replace(/<https?:\/\/[^>]+>/giu, "")
    .replace(/`+([^`]+?)`+/gu, "$1")
    .replace(/\*\*([^*\n]+)\*\*/gu, "$1")
    .replace(/__([^_\n]+)__/gu, "$1")
    .replace(/(?<![\p{L}\p{N}])_([^_\n]+)_(?![\p{L}\p{N}])/gu, "$1")
    .replace(/(?<!\*)\*([^*\n]+)\*(?!\*)/gu, "$1")
    .replace(/~~([^~\n]+)~~/gu, "$1")
    .replace(/^\s{0,3}(?:#{1,6}|>|[-+*]|\d+[.)])\s+/gmu, "")
    .replace(/\\([\\`*{}\[\]()#+.!_>~-])/gu, "$1");
}

function countTokenOverlap(left, right) {
  const remaining = new Map();
  for (const token of right) {
    remaining.set(token, (remaining.get(token) ?? 0) + 1);
  }
  let overlap = 0;
  for (const token of left) {
    const count = remaining.get(token) ?? 0;
    if (count > 0) {
      overlap += 1;
      remaining.set(token, count - 1);
    }
  }
  return overlap;
}

export function referenceScores(markdown, reference) {
  const actual = tokenize(markdownVisibleText(markdown));
  const expected = tokenize(markdownVisibleText(reference));
  const overlap = countTokenOverlap(actual, expected);
  const precision = actual.length === 0 ? 0 : overlap / actual.length;
  const recall = expected.length === 0 ? 1 : overlap / expected.length;
  const f1 =
    precision + recall === 0
      ? 0
      : (2 * precision * recall) / (precision + recall);
  return { precision, recall, f1 };
}

function markdownStructure(markdown) {
  const lines = markdown.split(/\r?\n/u);
  let fence = null;
  let codeBlocks = 0;
  let headings = 0;
  let tables = 0;
  let lists = 0;
  let footnotes = 0;
  let mathBlocks = 0;
  let inList = false;
  let inMathBlock = false;
  let replyItems = 0;
  let replyDepth = 0;
  const listIndents = [];
  const headingsByLevel = [0, 0, 0, 0, 0, 0];
  const nonCodeLines = [];

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const fenceMatch = line.match(/^\s{0,3}(`{3,}|~{3,})(.*)$/u);
    if (fence) {
      if (
        fenceMatch &&
        fenceMatch[1][0] === fence.character &&
        fenceMatch[1].length >= fence.length &&
        fenceMatch[2].trim() === ""
      ) {
        fence = null;
      }
      continue;
    }
    if (fenceMatch) {
      fence = {
        character: fenceMatch[1][0],
        length: fenceMatch[1].length,
      };
      codeBlocks += 1;
      continue;
    }
    if (/^\s*\$\$\s*$/u.test(line)) {
      if (!inMathBlock) mathBlocks += 1;
      inMathBlock = !inMathBlock;
      continue;
    }
    if (inMathBlock) continue;
    nonCodeLines.push(line);
    const heading = line.match(/^(#{1,6})\s+\S/u);
    if (heading) {
      headings += 1;
      headingsByLevel[heading[1].length - 1] += 1;
    }
    if (/^\[\^[^\]]+\]:/u.test(line)) footnotes += 1;

    const listItem = line.match(/^(\s*)(?:(\d+)[.)]|[-+*])\s+\S/u);
    const isListItem = listItem !== null;
    if (isListItem && !inList) lists += 1;
    inList = isListItem || (inList && /^\s{2,}\S/u.test(line));
    if (listItem?.[2]) {
      const indent = [...listItem[1]].reduce(
        (width, character) => width + (character === "\t" ? 4 : 1),
        0,
      );
      while (
        listIndents.length > 0 &&
        indent <= listIndents[listIndents.length - 1]
      ) {
        listIndents.pop();
      }
      listIndents.push(indent);
      replyItems += 1;
      replyDepth = Math.max(replyDepth, listIndents.length);
    } else if (line.trim() && !/^\s{2,}\S/u.test(line)) {
      listIndents.length = 0;
    }

    if (
      index > 0 &&
      /^\s*\|?(?:\s*:?-{3,}:?\s*\|)+(?:\s*:?-{3,}:?\s*)\|?\s*$/u.test(line) &&
      lines[index - 1].includes("|")
    ) {
      tables += 1;
    }
  }

  const mathInline = countInlineMath(nonCodeLines);
  return {
    headings,
    lists,
    code_blocks: codeBlocks,
    tables,
    footnotes,
    math: mathBlocks + mathInline,
    reply_items: replyItems,
    reply_depth: replyDepth,
    headings_h1: headingsByLevel[0],
    headings_h2: headingsByLevel[1],
    headings_h3: headingsByLevel[2],
    headings_h4: headingsByLevel[3],
    headings_h5: headingsByLevel[4],
    headings_h6: headingsByLevel[5],
  };
}

function replyStructure(markdown, markers) {
  if (!markers?.length) return null;
  const markerTokens = markers.map(tokenize);
  const containsMarker = (value) => {
    const tokens = tokenize(markdownVisibleText(value));
    return markerTokens.some((marker) =>
      tokens.some((_, start) =>
        marker.every((token, offset) => tokens[start + offset] === token),
      ),
    );
  };
  let replyItems = 0;
  let replyDepth = 0;
  const lines = markdown.split(/\r?\n/u);
  for (let index = 0; index < lines.length; index += 1) {
    const item = lines[index].match(/^(\s*)\d+[.)](?:\s+(.*))?$/u);
    if (!item) continue;
    const indent = [...item[1]].reduce(
      (width, character) => width + (character === "\t" ? 4 : 1),
      0,
    );
    const block = [item[2] ?? ""];
    for (let cursor = index + 1; cursor < lines.length; cursor += 1) {
      if (/^\s*\d+[.)](?:\s+.*)?$/u.test(lines[cursor])) break;
      if (lines[cursor].trim()) block.push(lines[cursor]);
    }
    if (!containsMarker(block.join("\n"))) continue;
    replyItems += 1;
    replyDepth = Math.max(replyDepth, Math.floor(indent / 3) + 1);
  }
  return { reply_items: replyItems, reply_depth: replyDepth };
}

export function outputMetrics(markdown, details = {}) {
  const linkScan = scanMarkdownLinks(markdown);
  const structure = markdownStructure(markdown);
  return {
    words: details.word_count ?? tokenize(markdown).length,
    links: linkScan.links,
    link_density: markdown.length
      ? linkScan.linkedCharacters / markdown.length
      : 0,
    headings: structure.headings,
    images: linkScan.images,
    image_sources: linkScan.imageSources,
    code_blocks: structure.code_blocks,
    tables: Math.max(details.tables ?? 0, structure.tables),
    figures: details.figures ?? 0,
    lists: structure.lists,
    footnotes: structure.footnotes,
    math: structure.math,
    reply_items: structure.reply_items,
    reply_depth: structure.reply_depth,
    headings_h1: structure.headings_h1,
    headings_h2: structure.headings_h2,
    headings_h3: structure.headings_h3,
    headings_h4: structure.headings_h4,
    headings_h5: structure.headings_h5,
    headings_h6: structure.headings_h6,
    junk_phrases: JUNK_PHRASES.filter((phrase) =>
      normalizeText(markdown).includes(phrase),
    ),
    metadata: details.metadata ?? null,
    semantic_coverage: details.semantic_coverage ?? null,
  };
}

export function validateManifest(manifest, directory) {
  if (!validateAgainstSchema(manifest)) {
    const details = validateAgainstSchema.errors
      .map((error) => `${error.instancePath || "/"} ${error.message}`)
      .join("; ");
    throw new Error(`${directory}/manifest.json violates schema: ${details}`);
  }
  if (manifest.id !== basename(directory)) {
    throw new Error(`${directory}: manifest id must match the directory name`);
  }
  return manifest;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function fixtureDirectories(root) {
  return readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    if (!entry.isDirectory()) return [];
    const directory = join(root, entry.name);
    if (existsSync(join(directory, "manifest.json"))) return [directory];
    return fixtureDirectories(directory);
  });
}

export function discoverFixtures(root) {
  return fixtureDirectories(root)
    .map((directory) => {
      const manifest = validateManifest(
        readJson(join(directory, "manifest.json")),
        directory,
      );
      const sourcePath = join(directory, "source.html");
      if (!existsSync(sourcePath)) {
        throw new Error(`${directory}: source.html is required`);
      }
      const referencePath = join(directory, "reference.md");
      const metadataPath = join(directory, "metadata.json");
      return {
        directory,
        manifest,
        sourcePath,
        reference: existsSync(referencePath)
          ? readFileSync(referencePath, "utf8")
          : null,
        expectedMetadata: existsSync(metadataPath)
          ? readJson(metadataPath)
          : null,
      };
    })
    .sort((left, right) => left.manifest.id.localeCompare(right.manifest.id));
}

function mean(values) {
  const present = values.filter(
    (value) => value !== null && value !== undefined,
  );
  if (present.length === 0) return null;
  return present.reduce((sum, value) => sum + value, 0) / present.length;
}

function normalizedMetadataValue(value) {
  if (typeof value === "string") return normalizeText(value);
  if (Array.isArray(value)) return value.map(normalizedMetadataValue);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, item]) => [key, normalizedMetadataValue(item)]),
    );
  }
  return value;
}

function metadataScores(actual, expected) {
  if (!expected) return { score: null, mismatches: [] };
  const fields = Object.keys(expected).sort();
  const mismatches = fields.filter(
    (field) =>
      JSON.stringify(normalizedMetadataValue(actual?.[field])) !==
      JSON.stringify(normalizedMetadataValue(expected[field])),
  );
  return {
    score:
      fields.length === 0
        ? null
        : (fields.length - mismatches.length) / fields.length,
    mismatches,
  };
}

export function scoreExtraction(extraction, fixture) {
  const expectedFailure = fixture.manifest.expected_failure ?? false;
  const expectedError = fixture.manifest.expected_error ?? null;
  const success = extraction.success === true;
  const markdown = success ? extraction.markdown : "";
  const normalized = normalizeText(markdownVisibleText(markdown));
  const requiredMissing = fixture.manifest.must_include.filter(
    (phrase) => !normalized.includes(normalizeText(phrase)),
  );
  const noiseFound = fixture.manifest.must_exclude.filter((phrase) =>
    normalized.includes(normalizeText(phrase)),
  );
  const metrics = outputMetrics(markdown, extraction);
  const explicitReplies = replyStructure(
    markdown,
    fixture.manifest.reply_markers,
  );
  if (explicitReplies) Object.assign(metrics, explicitReplies);
  const structureChecks = Object.entries(fixture.manifest.expected ?? {}).map(
    ([name, limit]) => {
      const match = name.match(/^(.*)_(min|max)$/u);
      const actual = metrics[match[1]];
      return match[2] === "min" ? actual >= limit : actual <= limit;
    },
  );
  const requiredImageSources =
    fixture.manifest.must_include_image_sources ?? [];
  const excludedImageSources =
    fixture.manifest.must_exclude_image_sources ?? [];
  const imageSourcesMissing = requiredImageSources.filter(
    (source) => !metrics.image_sources.includes(source),
  );
  const imageSourcesFound = excludedImageSources.filter((source) =>
    metrics.image_sources.includes(source),
  );
  structureChecks.push(
    ...requiredImageSources.map((source) =>
      metrics.image_sources.includes(source),
    ),
    ...excludedImageSources.map(
      (source) => !metrics.image_sources.includes(source),
    ),
  );
  const metadata = metadataScores(
    extraction.metadata,
    fixture.expectedMetadata,
  );
  const reference = fixture.reference
    ? referenceScores(markdown, fixture.reference)
    : null;
  const correctExpectedFailure =
    expectedFailure &&
    !success &&
    extraction.failure_kind === "extraction" &&
    extraction.deterministic === true &&
    (expectedError === null ||
      extraction.error_details?.variant === expectedError);
  const reliabilityPass = expectedFailure
    ? correctExpectedFailure
    : success && extraction.deterministic === true;
  const configuredScore = (value) => {
    if (expectedFailure || value === null) return null;
    return success ? value : 0;
  };
  return {
    success,
    error: extraction.error ?? null,
    failure_kind: extraction.failure_kind ?? null,
    error_variant: extraction.error_details?.variant ?? null,
    panic: extraction.failure_kind === "panic",
    deterministic: extraction.deterministic ?? null,
    reliability_pass: reliabilityPass,
    required_content_score: configuredScore(
      fixture.manifest.must_include.length === 0
        ? null
        : (fixture.manifest.must_include.length - requiredMissing.length) /
            fixture.manifest.must_include.length,
    ),
    noise_score: configuredScore(
      fixture.manifest.must_exclude.length === 0
        ? null
        : (fixture.manifest.must_exclude.length - noiseFound.length) /
            fixture.manifest.must_exclude.length,
    ),
    structure_score: configuredScore(
      structureChecks.length === 0
        ? null
        : structureChecks.filter(Boolean).length / structureChecks.length,
    ),
    metadata_score: configuredScore(metadata.score),
    reference: expectedFailure
      ? null
      : success || reference === null
        ? reference
        : { precision: 0, recall: 0, f1: 0 },
    required_missing: requiredMissing,
    noise_found: noiseFound,
    structure: Object.fromEntries(
      STRUCTURE_NAMES.map((name) => [name, metrics[name]]),
    ),
    image_sources_missing: imageSourcesMissing,
    excluded_image_sources_found: imageSourcesFound,
    metadata_mismatches: metadata.mismatches,
    words: metrics.words,
    links: metrics.links,
    link_density: metrics.link_density,
    headings: metrics.headings,
    images: metrics.images,
    code_blocks: metrics.code_blocks,
    tables: metrics.tables,
    figures: metrics.figures,
    lists: metrics.lists,
    footnotes: metrics.footnotes,
    math: metrics.math,
    reply_items: metrics.reply_items,
    reply_depth: metrics.reply_depth,
    headings_h1: metrics.headings_h1,
    headings_h2: metrics.headings_h2,
    headings_h3: metrics.headings_h3,
    headings_h4: metrics.headings_h4,
    headings_h5: metrics.headings_h5,
    headings_h6: metrics.headings_h6,
    junk_phrases: metrics.junk_phrases,
    semantic_coverage: metrics.semantic_coverage,
  };
}

export function comparisonResult(fixture, legible, defuddle) {
  const legibleScore = scoreExtraction(legible, fixture);
  const defuddleScore = scoreExtraction(defuddle, fixture);
  return {
    fixture: fixture.manifest.id,
    category: fixture.manifest.category,
    page_shape: fixture.manifest.page_shape,
    legible: legibleScore,
    defuddle: defuddleScore,
    differences: {
      legible_missing: legibleScore.required_missing,
      defuddle_missing: defuddleScore.required_missing,
      legible_noise: legibleScore.noise_found,
      defuddle_noise: defuddleScore.noise_found,
    },
  };
}

function extractorSummary(results, name) {
  const scores = results.map((result) => result[name]);
  return {
    success_rate: mean(scores.map((score) => (score.success ? 1 : 0))),
    reliability_rate: mean(
      scores.map((score) => (score.reliability_pass ? 1 : 0)),
    ),
    panic_count: scores.filter((score) => score.panic).length,
    tool_failure_count: scores.filter((score) => score.failure_kind === "tool")
      .length,
    required_content_score: mean(
      scores.map((score) => score.required_content_score),
    ),
    noise_score: mean(scores.map((score) => score.noise_score)),
    structure_score: mean(scores.map((score) => score.structure_score)),
    metadata_score: mean(scores.map((score) => score.metadata_score)),
    reference_precision: mean(
      scores.map((score) => score.reference?.precision),
    ),
    reference_recall: mean(scores.map((score) => score.reference?.recall)),
    reference_f1: mean(scores.map((score) => score.reference?.f1)),
  };
}

function qualityValue(score) {
  return mean([
    score.required_content_score,
    score.noise_score,
    score.structure_score,
    score.metadata_score,
    score.reference?.f1,
  ]);
}

export function aggregateReport(results, revisions) {
  const comparisons = results
    .map((result) => {
      const legible = qualityValue(result.legible);
      const defuddle = qualityValue(result.defuddle);
      return {
        fixture: result.fixture,
        delta:
          legible === null || defuddle === null ? null : legible - defuddle,
      };
    })
    .filter((item) => item.delta !== null)
    .sort(
      (left, right) =>
        right.delta - left.delta || left.fixture.localeCompare(right.fixture),
    );
  return {
    schema_version: REPORT_SCHEMA_VERSION,
    revisions,
    fixture_count: results.length,
    aggregate: {
      legible: extractorSummary(results, "legible"),
      defuddle: extractorSummary(results, "defuddle"),
    },
    by_category: Object.fromEntries(
      [...new Set(results.map((result) => result.category))]
        .sort()
        .map((category) => {
          const categoryResults = results.filter(
            (result) => result.category === category,
          );
          return [
            category,
            {
              fixture_count: categoryResults.length,
              legible: extractorSummary(categoryResults, "legible"),
              defuddle: extractorSummary(categoryResults, "defuddle"),
            },
          ];
        }),
    ),
    largest_legible_wins: comparisons
      .filter((item) => item.delta > 0)
      .slice(0, 5),
    largest_legible_regressions: comparisons
      .filter((item) => item.delta < 0)
      .reverse()
      .slice(0, 5),
    results,
  };
}

export function formatScore(value) {
  return value === null || value === undefined ? "-" : value.toFixed(2);
}

export function formatHumanReport(report) {
  const legibleRevision = report.revisions.legible;
  const legibleLabel =
    typeof legibleRevision === "string"
      ? legibleRevision
      : `${legibleRevision.commit}${
          legibleRevision.dirty ? ` (dirty ${legibleRevision.diff_sha256})` : ""
        }`;
  const lines = [
    `Compared ${report.fixture_count} fixture(s).`,
    `Legible revision: ${legibleLabel}`,
    `Defuddle revision: ${report.revisions.defuddle}`,
    "",
  ];
  for (const result of report.results) {
    lines.push(
      `${result.fixture}: Legible recall ${formatScore(result.legible.required_content_score)}, noise ${formatScore(result.legible.noise_score)}, structure ${formatScore(result.legible.structure_score)}; Defuddle recall ${formatScore(result.defuddle.required_content_score)}, noise ${formatScore(result.defuddle.noise_score)}, structure ${formatScore(result.defuddle.structure_score)}`,
    );
  }
  const formatComparisons = (items) =>
    items.length === 0
      ? "none"
      : items
          .map(
            (item) =>
              `${item.fixture} (${item.delta >= 0 ? "+" : ""}${item.delta.toFixed(3)})`,
          )
          .join(", ");
  lines.push(
    "",
    `Largest Legible wins: ${formatComparisons(report.largest_legible_wins)}`,
    `Largest Legible regressions: ${formatComparisons(report.largest_legible_regressions)}`,
  );
  return lines.join("\n");
}

function formatSummaryRevision(revision) {
  if (typeof revision === "string") return revision;
  if (!revision || typeof revision !== "object") return "unknown";
  const suffix = revision.dirty
    ? ` (dirty corpus ${revision.diff_sha256 ?? "unknown"})`
    : "";
  return `${revision.commit ?? "unknown"}${suffix}`;
}

function summaryRow(label, summary) {
  return `| ${label} | ${formatScore(summary.required_content_score)} | ${formatScore(summary.noise_score)} | ${formatScore(summary.structure_score)} | ${formatScore(summary.metadata_score)} | ${formatScore(summary.reference_f1)} | ${formatScore(summary.reliability_rate)} |`;
}

export function formatMarkdownSummary(report) {
  const lines = [
    "# Extraction quality baseline",
    "",
    "> This curated corpus measures specific extraction cases. It is not an absolute measure of all web pages.",
    "",
    `- Fixtures: ${report.fixture_count}`,
    `- Legible revision: \`${formatSummaryRevision(report.revisions.legible)}\``,
    `- Defuddle revision: \`${report.revisions.defuddle}\``,
    "",
    "## Aggregate results",
    "",
    "| Extractor | Content recall | Noise rejection | Structural fidelity | Metadata accuracy | Reference F1 | Reliability |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    summaryRow("Legible", report.aggregate.legible),
    summaryRow("Defuddle", report.aggregate.defuddle),
    "",
    "## Results by category",
    "",
    "| Category | Fixtures | Extractor | Content recall | Noise rejection | Structural fidelity | Metadata accuracy | Reference F1 | Reliability |",
    "| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
  ];
  for (const [category, result] of Object.entries(report.by_category)) {
    for (const [label, summary] of [
      ["Legible", result.legible],
      ["Defuddle", result.defuddle],
    ]) {
      lines.push(
        `| ${category} | ${result.fixture_count} | ${label} | ${formatScore(summary.required_content_score)} | ${formatScore(summary.noise_score)} | ${formatScore(summary.structure_score)} | ${formatScore(summary.metadata_score)} | ${formatScore(summary.reference_f1)} | ${formatScore(summary.reliability_rate)} |`,
      );
    }
  }
  const reliabilityFailures = report.results
    .filter((result) => !result.legible.reliability_pass)
    .map((result) => result.fixture);
  const comparatorReliabilityFailures = report.results
    .filter((result) => !result.defuddle.reliability_pass)
    .map((result) => result.fixture);
  const comparatorGaps = report.largest_legible_regressions.map(
    (item) => `${item.fixture} (${item.delta.toFixed(3)})`,
  );
  lines.push(
    "",
    "## Current gaps",
    "",
    `- Largest comparator gaps: ${comparatorGaps.join(", ") || "none"}`,
    `- Legible reliability failures: ${reliabilityFailures.join(", ") || "none"}`,
    `- Defuddle reliability failures: ${comparatorReliabilityFailures.join(", ") || "none"}`,
    "",
  );
  return lines.join("\n");
}

export function fixtureOutputDirectory(outputRoot, fixture) {
  return join(outputRoot, fixture.manifest.id);
}

export function fixtureIdFromPath(path) {
  return basename(dirname(path));
}
