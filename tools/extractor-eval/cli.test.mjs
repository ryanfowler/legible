import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const indexPath = join(scriptDirectory, "index.mjs");

test("accepts expected extraction failures but rejects comparator panics", () => {
  const root = mkdtempSync(join(tmpdir(), "legible-quality-cli-"));
  const fixtureDirectory = join(root, "empty-page");
  const outputDirectory = join(root, "output");
  const fixtureOutput = join(outputDirectory, "empty-page");
  mkdirSync(fixtureDirectory);
  mkdirSync(fixtureOutput, { recursive: true });
  writeFileSync(join(fixtureOutput, "legible.md"), "stale Legible output");
  writeFileSync(join(fixtureOutput, "defuddle.md"), "stale Defuddle output");
  writeFileSync(
    join(fixtureDirectory, "manifest.json"),
    JSON.stringify({
      id: "empty-page",
      category: "application-markup",
      source_url: "https://example.test/empty",
      page_shape: "application",
      must_include: [],
      must_exclude: ["Unexpected content"],
      expected_failure: true,
      expected_error: "NoContent",
      notes: "Exercise structured expected-failure handling.",
    }),
  );
  writeFileSync(join(fixtureDirectory, "source.html"), "<html></html>");

  assert.throws(() =>
    execFileSync(
      process.execPath,
      [indexPath, "--all", "--fixture-root", root, "--output", outputDirectory],
      {
        encoding: "utf8",
        env: {
          ...process.env,
          DEFUDDLE_COMMAND: process.execPath,
          DEFUDDLE_ARGS: JSON.stringify([
            "-e",
            "console.error(\"thread 'main' panicked at test\"); process.exit(1)",
          ]),
        },
        stdio: "pipe",
      },
    ),
  );

  const report = JSON.parse(
    readFileSync(join(outputDirectory, "report.json"), "utf8"),
  );
  assert.equal(report.results[0].legible.failure_kind, "extraction");
  assert.equal(report.results[0].legible.deterministic, true);
  assert.equal(report.results[0].legible.reliability_pass, true);
  assert.equal(report.results[0].legible.noise_score, null);
  assert.equal(report.results[0].defuddle.failure_kind, "panic");
  assert.equal(report.results[0].defuddle.deterministic, true);
  assert.equal(report.results[0].defuddle.reliability_pass, false);
  assert.equal(report.aggregate.defuddle.panic_count, 1);
  assert.equal(existsSync(join(fixtureOutput, "legible.md")), false);
  assert.equal(existsSync(join(fixtureOutput, "defuddle.md")), false);
});

test("keeps revision hashing stable across custom summary writes", () => {
  const root = mkdtempSync(join(tmpdir(), "legible-quality-summary-"));
  const fixtureDirectory = join(root, "empty-page");
  const outputDirectory = join(root, "output");
  const repository = dirname(dirname(scriptDirectory));
  const summaryPath = join(
    repository,
    `.quality-summary-test-${process.pid}.md`,
  );
  mkdirSync(fixtureDirectory);
  writeFileSync(
    join(fixtureDirectory, "manifest.json"),
    JSON.stringify({
      id: "empty-page",
      category: "application-markup",
      source_url: "https://example.test/empty",
      page_shape: "application",
      must_include: [],
      must_exclude: [],
      expected_failure: true,
      expected_error: "NoContent",
      notes: "Exercise stable custom summary hashing.",
    }),
  );
  writeFileSync(join(fixtureDirectory, "source.html"), "<html></html>");
  const arguments_ = [
    indexPath,
    "--all",
    "--fixture-root",
    root,
    "--output",
    outputDirectory,
    "--summary",
    summaryPath,
  ];
  const environment = {
    ...process.env,
    DEFUDDLE_COMMAND: process.execPath,
    DEFUDDLE_ARGS: JSON.stringify(["-e", "console.log('{}')"]),
  };
  try {
    for (let run = 0; run < 2; run += 1) {
      assert.throws(() =>
        execFileSync(process.execPath, arguments_, {
          encoding: "utf8",
          env: environment,
          stdio: "pipe",
        }),
      );
      const revision = JSON.parse(
        readFileSync(join(outputDirectory, "report.json"), "utf8"),
      ).revisions.legible;
      if (run === 0) {
        writeFileSync(join(root, "revision.json"), JSON.stringify(revision));
      } else {
        assert.deepEqual(
          revision,
          JSON.parse(readFileSync(join(root, "revision.json"), "utf8")),
        );
      }
    }
  } finally {
    rmSync(summaryPath, { force: true });
  }
});

test("fails the baseline gate only for a lower Legible score", () => {
  const root = mkdtempSync(join(tmpdir(), "legible-quality-baseline-"));
  const fixtureDirectory = join(root, "simple-article");
  const outputDirectory = join(root, "output");
  mkdirSync(fixtureDirectory);
  writeFileSync(
    join(fixtureDirectory, "manifest.json"),
    JSON.stringify({
      id: "simple-article",
      category: "news-article",
      source_url: "https://example.test/simple",
      page_shape: "article",
      must_include: ["The harbour opened at dawn"],
      must_exclude: ["Missing phrase that never appears"],
      notes: "Exercise the baseline regression gate.",
    }),
  );
  writeFileSync(
    join(fixtureDirectory, "source.html"),
    "<html><body><article><h1>Harbour</h1><p>The harbour opened at dawn and the first ferry left on time for the island.</p></article></body></html>",
  );
  const run = (extraArguments) =>
    execFileSync(
      process.execPath,
      [
        indexPath,
        "--fixture-root",
        root,
        "--output",
        outputDirectory,
        "--gate",
        "legible",
        ...extraArguments,
      ],
      {
        encoding: "utf8",
        env: {
          ...process.env,
          DEFUDDLE_COMMAND: process.execPath,
          DEFUDDLE_ARGS: JSON.stringify(["-e", "console.log('{}')"]),
        },
        stdio: "pipe",
      },
    );

  const baselinePath = join(root, "baseline.json");
  assert.throws(
    () =>
      run(["--fixture", "simple-article", "--write-baseline", baselinePath]),
    (error) => /--write-baseline requires all fixtures/u.test(error.stderr),
  );
  assert.equal(existsSync(baselinePath), false);
  run(["--all", "--write-baseline", baselinePath]);
  const baseline = JSON.parse(readFileSync(baselinePath, "utf8"));
  assert.equal(baseline.fixtures["simple-article"].required_content_score, 1);
  assert.match(
    run(["--all", "--baseline", baselinePath]),
    /No quality regressions against the baseline\./u,
  );

  // A baseline that records a higher score than Legible can reach must fail.
  baseline.fixtures["simple-article"].noise_score = 2;
  writeFileSync(baselinePath, JSON.stringify(baseline));
  assert.throws(
    () => run(["--all", "--baseline", baselinePath]),
    (error) => /simple-article noise_score: 2\.00 -> 1\.00/u.test(error.stdout),
  );
});
