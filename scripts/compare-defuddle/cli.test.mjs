import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
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
