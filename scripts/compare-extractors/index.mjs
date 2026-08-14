#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  aggregateReport,
  comparisonResult,
  discoverFixtures,
  fixtureOutputDirectory,
  formatHumanReport,
  formatMarkdownSummary,
} from "./lib.mjs";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repository = resolve(scriptDirectory, "../..");
const defaultFixtureRoot = join(repository, "benchmarks/quality");
const defaultOutputRoot = join(repository, "target/quality-comparison");

function usage() {
  return `usage: node scripts/compare-extractors/index.mjs [--all | --fixture ID] [options]

Options:
  --all                 Run every discovered quality fixture (default).
  --fixture ID          Run one fixture by manifest id.
  --fixture-root PATH   Use another quality fixture root.
  --output PATH         Write per-fixture artifacts and report.json here.
  --json PATH           Also write the aggregate JSON report to PATH.
  --summary PATH        Also write a compact Markdown summary to PATH.
  --help                Show this help.

DEFUDDLE_COMMAND and DEFUDDLE_ARGS can replace the pinned local Defuddle wrapper.`;
}

function parseArguments(arguments_) {
  const options = {
    all: false,
    fixture: null,
    fixtureRoot: defaultFixtureRoot,
    outputRoot: defaultOutputRoot,
    jsonPath: null,
    summaryPath: null,
  };
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    const value = () => {
      index += 1;
      if (index >= arguments_.length) {
        throw new Error(`${argument} requires a value`);
      }
      return arguments_[index];
    };
    switch (argument) {
      case "--all":
        options.all = true;
        break;
      case "--fixture":
        options.fixture = value();
        break;
      case "--fixture-root":
        options.fixtureRoot = resolve(value());
        break;
      case "--output":
        options.outputRoot = resolve(value());
        break;
      case "--json":
        options.jsonPath = resolve(value());
        break;
      case "--summary":
        options.summaryPath = resolve(value());
        break;
      case "--help":
      case "-h":
        console.log(usage());
        process.exit(0);
        break;
      default:
        throw new Error(`unknown argument: ${argument}`);
    }
  }
  if (options.all && options.fixture) {
    throw new Error("--all and --fixture cannot be used together");
  }
  if (!options.fixture) options.all = true;
  return options;
}

function parseExternalArguments() {
  const arguments_ = JSON.parse(process.env.DEFUDDLE_ARGS ?? "[]");
  if (
    !Array.isArray(arguments_) ||
    arguments_.some((value) => typeof value !== "string")
  ) {
    throw new Error("DEFUDDLE_ARGS must be a JSON array of strings");
  }
  return arguments_;
}

function toolOutput(output) {
  try {
    const parsed = JSON.parse(output);
    if (typeof parsed.markdown === "string") return parsed;
    if (parsed.success === false && parsed.error) {
      return {
        success: false,
        failure_kind:
          parsed.error.kind === "extraction" ? "extraction" : "tool",
        error: parsed.error.message ?? String(parsed.error),
        error_details: parsed.error,
      };
    }
  } catch {
    // An external comparison command can write plain Markdown.
  }
  return { markdown: output };
}

function commandFailure(error) {
  const standardError = error?.stderr?.toString().trim();
  const message = standardError || error?.message || String(error);
  const panic = /(?:thread .* panicked|panicked at)/iu.test(message);
  return {
    success: false,
    failure_kind: panic ? "panic" : "tool",
    error: message,
  };
}

function runCommand(command, arguments_, options) {
  try {
    const output = toolOutput(
      execFileSync(command, arguments_, { ...options, encoding: "utf8" }),
    );
    return { success: true, ...output };
  } catch (error) {
    return commandFailure(error);
  }
}

function runCommandTwice(command, arguments_, options) {
  const first = runCommand(command, arguments_, options);
  const second = runCommand(command, arguments_, options);
  return {
    ...first,
    deterministic: JSON.stringify(first) === JSON.stringify(second),
  };
}

function runLegible(fixture) {
  return runCommandTwice(
    "cargo",
    [
      "run",
      "--quiet",
      "--offline",
      "--example",
      "extract_fixture",
      "--",
      "--json",
      "--url",
      fixture.manifest.source_url,
      fixture.sourcePath,
    ],
    { cwd: repository },
  );
}

function normalizeDefuddleResult(result) {
  const author = result.author;
  const authors = Array.isArray(author)
    ? author
    : typeof author === "string" && author.trim()
      ? [author]
      : [];
  const markdown = result.contentMarkdown ?? result.content ?? "";
  return {
    success: true,
    markdown,
    word_count: result.wordCount,
    tables: (result.content?.match(/<table\b/giu) ?? []).length,
    figures: (result.content?.match(/<figure\b/giu) ?? []).length,
    metadata: {
      title: result.title ?? null,
      description: result.description ?? null,
      authors,
      site_name: result.site ?? result.siteName ?? null,
      canonical_url: result.canonicalUrl ?? result.url ?? null,
      image: result.image ?? null,
      favicon: result.favicon ?? null,
      published_time: result.published ?? result.publishedTime ?? null,
      modified_time: result.modified ?? result.modifiedTime ?? null,
      language: result.language ?? null,
      direction: result.direction ?? null,
      section: result.section ?? null,
      tags: result.tags ?? [],
    },
  };
}

async function builtInDefuddleRunner() {
  let Defuddle;
  let parseHTML;
  try {
    ({ Defuddle } = await import("defuddle/node"));
    ({ parseHTML } = await import("linkedom"));
  } catch (error) {
    throw new Error(
      `Pinned Defuddle dependencies are not installed. Run npm --prefix scripts/compare-extractors ci. ${error.message}`,
    );
  }
  return async (fixture) => {
    const html = readFileSync(fixture.sourcePath, "utf8");
    const extract = async () => {
      const { document } = parseHTML(html);
      const result = await Defuddle(document, fixture.manifest.source_url, {
        markdown: true,
        separateMarkdown: true,
        useAsync: false,
      });
      return normalizeDefuddleResult(result);
    };
    const run = async () => {
      try {
        return await extract();
      } catch (error) {
        return commandFailure(error);
      }
    };
    const first = await run();
    const second = await run();
    return {
      ...first,
      deterministic: JSON.stringify(first) === JSON.stringify(second),
    };
  };
}

function externalDefuddleRunner(command) {
  const externalArguments = parseExternalArguments();
  return async (fixture) =>
    runCommandTwice(command, [...externalArguments, fixture.sourcePath], {
      cwd: repository,
      env: {
        ...process.env,
        LEGIBLE_SOURCE_URL: fixture.manifest.source_url,
      },
    });
}

function packageVersion() {
  const packageJson = JSON.parse(
    readFileSync(join(scriptDirectory, "package.json"), "utf8"),
  );
  return `npm:defuddle@${packageJson.dependencies.defuddle}`;
}

function gitRevision(summaryPath) {
  try {
    const commit = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: repository,
      encoding: "utf8",
    }).trim();
    const relativeSummary = summaryPath
      ? relative(repository, summaryPath)
      : null;
    const excludedSummary =
      relativeSummary &&
      relativeSummary !== "" &&
      !relativeSummary.startsWith("..")
        ? relativeSummary
        : null;
    const pathspec = ["--", "."];
    if (excludedSummary) pathspec.push(`:(exclude)${excludedSummary}`);
    const status = execFileSync(
      "git",
      ["status", "--porcelain", "--untracked-files=all", ...pathspec],
      { cwd: repository, encoding: "utf8" },
    );
    if (!status) return { commit, dirty: false, diff_sha256: null };

    const hash = createHash("sha256");
    hash.update(
      execFileSync("git", ["diff", "--binary", "HEAD", ...pathspec], {
        cwd: repository,
      }),
    );
    const untracked = execFileSync(
      "git",
      ["ls-files", "--others", "--exclude-standard", "-z"],
      { cwd: repository, encoding: "utf8" },
    )
      .split("\0")
      .filter((path) => path && path !== excludedSummary)
      .sort();
    for (const path of untracked) {
      hash.update(path);
      hash.update("\0");
      hash.update(readFileSync(join(repository, path)));
      hash.update("\0");
    }
    return { commit, dirty: true, diff_sha256: hash.digest("hex") };
  } catch {
    return { commit: "unknown", dirty: null, diff_sha256: null };
  }
}

function writeJson(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

function writeFixtureArtifacts(outputRoot, fixture, legible, defuddle, result) {
  const destination = fixtureOutputDirectory(outputRoot, fixture);
  mkdirSync(destination, { recursive: true });
  rmSync(join(destination, "legible.md"), { force: true });
  rmSync(join(destination, "defuddle.md"), { force: true });
  if (legible.success) {
    writeFileSync(join(destination, "legible.md"), legible.markdown);
  }
  if (defuddle.success) {
    writeFileSync(join(destination, "defuddle.md"), defuddle.markdown);
  }
  writeJson(join(destination, "result.json"), result);
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  let fixtures = discoverFixtures(options.fixtureRoot);
  if (options.fixture) {
    fixtures = fixtures.filter(
      (fixture) => fixture.manifest.id === options.fixture,
    );
    if (fixtures.length === 0) {
      throw new Error(`fixture not found: ${options.fixture}`);
    }
  }
  if (fixtures.length === 0) {
    throw new Error(`no quality fixtures found under ${options.fixtureRoot}`);
  }

  const defuddleCommand = process.env.DEFUDDLE_COMMAND;
  const runDefuddle = defuddleCommand
    ? externalDefuddleRunner(defuddleCommand)
    : await builtInDefuddleRunner();
  const results = [];
  for (const fixture of fixtures) {
    const legible = runLegible(fixture);
    const defuddle = await runDefuddle(fixture);
    const result = comparisonResult(fixture, legible, defuddle);
    writeFixtureArtifacts(
      options.outputRoot,
      fixture,
      legible,
      defuddle,
      result,
    );
    results.push(result);
  }

  const report = aggregateReport(results, {
    legible: gitRevision(options.summaryPath),
    defuddle: defuddleCommand
      ? `external:${defuddleCommand}`
      : packageVersion(),
  });
  const reportPath = join(options.outputRoot, "report.json");
  writeJson(reportPath, report);
  if (options.jsonPath) writeJson(options.jsonPath, report);
  if (options.summaryPath) {
    mkdirSync(dirname(options.summaryPath), { recursive: true });
    writeFileSync(options.summaryPath, formatMarkdownSummary(report));
  }
  console.log(formatHumanReport(report));
  console.log(`\nWrote comparison artifacts to ${options.outputRoot}`);

  if (
    results.some(
      (result) =>
        !result.legible.reliability_pass || !result.defuddle.reliability_pass,
    )
  ) {
    process.exitCode = 1;
  }
}

main().catch((error) => {
  console.error(error.message);
  console.error(usage());
  process.exitCode = 1;
});
