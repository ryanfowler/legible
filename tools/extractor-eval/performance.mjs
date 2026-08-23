#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";

const directory = resolve(fileURLToPath(new URL(".", import.meta.url)));
const repository = resolve(directory, "../..");
const defaultRoot = join(repository, "tests/fixtures/compatibility/readability/test/test-pages");
const args = process.argv.slice(2);
const worker = args[0] === "--worker";
const mode = worker ? args[1] : null;
const root = resolve(args[worker ? 2 : 0] ?? defaultRoot);
const rounds = Number(args[worker ? 3 : 1] ?? 3);

function sources(directoryPath) {
  return readdirSync(directoryPath, { withFileTypes: true })
    .flatMap((entry) => {
      const path = join(directoryPath, entry.name);
      if (entry.isDirectory()) return sources(path);
      return entry.name === "source.html" ? [path] : [];
    })
    .sort();
}

function loadPages() {
  return sources(root).map((path) => ({
    path,
    html: readFileSync(path, "utf8"),
  }));
}

async function runNodeWorker(extractor) {
  // Defuddle reports malformed metadata on some fixtures. Keep diagnostics out
  // of the timing and preserve the same behavior for every Node comparator.
  console.error = console.warn = () => {};
  const pages = loadPages();
  let extractPage;
  if (extractor === "defuddle") {
    const { Defuddle } = await import("defuddle/node");
    const { parseHTML } = await import("linkedom");
    extractPage = async (page) => {
      const { document } = parseHTML(page.html);
      const result = await Defuddle(document, "https://example.test/page", {
        markdown: true,
        separateMarkdown: true,
        useAsync: false,
      });
      return result.contentMarkdown ?? result.content ?? "";
    };
  } else {
    const { extract, extractSync } = await import(
      "@firecrawl/html-extractor"
    );
    const options = {
      url: "https://example.test/page",
      includeImages: true,
      includeLinks: true,
      includeMetadata: true,
      includeTables: true,
    };
    extractPage = (page) =>
      extractor === "firecrawl-async"
        ? extract(page.html, options).then((result) => result.markdown ?? "")
        : (extractSync(page.html, options).markdown ?? "");
  }
  const run = async () => {
    let markdownBytes = 0;
    let errors = 0;
    for (const page of pages) {
      try {
        markdownBytes += (await extractPage(page)).length;
      } catch {
        errors += 1;
      }
    }
    return { markdownBytes, errors };
  };

  for (let index = 0; index < 2; index += 1) await run();
  const times = [];
  let result;
  for (let index = 0; index < rounds; index += 1) {
    const start = performance.now();
    result = await run();
    times.push(performance.now() - start);
  }
  times.sort((left, right) => left - right);
  const median = times[Math.floor(times.length / 2)];
  const resourceUsage = process.resourceUsage();
  return {
    mode: extractor,
    pages: pages.length,
    bytes: pages.reduce((total, page) => total + Buffer.byteLength(page.html), 0),
    median_ms: Number(median.toFixed(3)),
    per_page_ms: Number((median / pages.length).toFixed(3)),
    markdown_bytes: result.markdownBytes,
    errors: result.errors,
    max_rss_mb: Number((resourceUsage.maxRSS / 1024).toFixed(1)),
  };
}

function runLegible() {
  const output = execFileSync(
    "cargo",
    [
      "run",
      "--quiet",
      "--offline",
      "--release",
      "--example",
      "benchmark_corpus",
      "--",
      root,
      String(rounds),
    ],
    { cwd: repository, encoding: "utf8" },
  );
  return JSON.parse(output);
}

if (worker) {
  if (!new Set(["defuddle", "firecrawl-sync", "firecrawl-async"]).has(mode)) {
    throw new Error(`unknown worker mode: ${mode}`);
  }
  console.log(JSON.stringify(await runNodeWorker(mode)));
} else {
  const nodeResults = ["defuddle", "firecrawl-sync", "firecrawl-async"].map(
    (extractor) =>
      JSON.parse(
        execFileSync(
          process.execPath,
          [fileURLToPath(import.meta.url), "--worker", extractor, root, String(rounds)],
          { cwd: directory, encoding: "utf8" },
        ),
      ),
  );
  const results = [runLegible(), ...nodeResults];
  console.log(JSON.stringify({ corpus: root, rounds, results }, null, 2));
}
