#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureRoot = resolve(process.argv[2] ?? join(repository, "tests/web"));
const outputRoot = resolve(
  process.argv[3] ?? join(repository, "target/defuddle-comparison"),
);
const defuddleCommand = process.env.DEFUDDLE_COMMAND;
const defuddleArgs = JSON.parse(process.env.DEFUDDLE_ARGS ?? "[]");
if (
  !Array.isArray(defuddleArgs) ||
  defuddleArgs.some((value) => typeof value !== "string")
) {
  throw new Error("DEFUDDLE_ARGS must be a JSON array of strings");
}

function sources(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sources(path);
    return entry.name === "source.html" ? [path] : [];
  });
}

function metrics(markdown, details = {}) {
  const links = [...markdown.matchAll(/(?<!!)\[([^\]]*)\]\([^)]+\)/gu)];
  const junkPhrases = [
    "subscribe",
    "sign up",
    "share this article",
    "advertisement",
    "related articles",
  ];
  const linkedCharacters = links.reduce(
    (total, link) => total + link[1].length,
    0,
  );
  return {
    words:
      details.word_count ??
      (markdown.trim() ? markdown.trim().split(/\s+/u).length : 0),
    link_density: markdown.length ? linkedCharacters / markdown.length : 0,
    headings: (markdown.match(/^#{1,6}\s/gmu) ?? []).length,
    images: (markdown.match(/!\[/gu) ?? []).length,
    code_blocks: Math.floor((markdown.match(/^```/gmu) ?? []).length / 2),
    tables: details.tables ?? 0,
    junk_phrases: junkPhrases.filter((phrase) =>
      markdown.toLowerCase().includes(phrase),
    ),
    metadata: details.metadata ?? null,
  };
}

function toolOutput(output) {
  try {
    const parsed = JSON.parse(output);
    if (typeof parsed.markdown === "string") return parsed;
  } catch {
    // Plain Markdown is the supported default for external tools.
  }
  return { markdown: output };
}

mkdirSync(outputRoot, { recursive: true });
for (const source of sources(fixtureRoot)) {
  const name = relative(fixtureRoot, dirname(source));
  const destination = join(outputRoot, name);
  mkdirSync(destination, { recursive: true });
  const legible = toolOutput(
    execFileSync(
      "cargo",
      [
        "run",
        "--quiet",
        "--example",
        "extract_fixture",
        "--",
        "--json",
        source,
      ],
      { cwd: repository, encoding: "utf8" },
    ),
  );
  writeFileSync(join(destination, "legible.md"), legible.markdown);
  const comparison = { legible: metrics(legible.markdown, legible) };

  if (defuddleCommand) {
    // The command receives the source path as its final shell-escaped argument.
    // It must write Defuddle Markdown to standard output.
    const defuddle = toolOutput(
      execFileSync(defuddleCommand, [...defuddleArgs, source], {
        cwd: repository,
        encoding: "utf8",
      }),
    );
    writeFileSync(join(destination, "defuddle.md"), defuddle.markdown);
    comparison.defuddle = metrics(defuddle.markdown, defuddle);
    if (legible.metadata && defuddle.metadata) {
      comparison.metadata_agreement = Object.fromEntries(
        Object.keys(legible.metadata).map((field) => [
          field,
          JSON.stringify(legible.metadata[field]) ===
            JSON.stringify(defuddle.metadata[field]),
        ]),
      );
    }
  }
  writeFileSync(
    join(destination, "metrics.json"),
    `${JSON.stringify(comparison, null, 2)}\n`,
  );
}
console.log(`Wrote comparison artifacts to ${outputRoot}`);
