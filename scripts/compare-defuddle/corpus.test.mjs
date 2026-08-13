import assert from "node:assert/strict";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { discoverFixtures } from "./lib.mjs";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const fixtures = discoverFixtures(
  resolve(scriptDirectory, "../../benchmarks/quality"),
);

function fixtureIds(prefix) {
  return fixtures
    .filter((fixture) => fixture.manifest.id.startsWith(prefix))
    .map((fixture) => fixture.manifest.id);
}

test("keeps the discussion benchmark batch complete", () => {
  const discussion = fixtures.filter(
    (fixture) =>
      fixture.manifest.id.startsWith("discussion-") ||
      fixture.manifest.id === "social-thread-media-links",
  );
  assert.ok(discussion.length >= 16);
  assert.ok(
    discussion.filter(
      (fixture) => (fixture.manifest.expected?.reply_depth_min ?? 0) >= 2,
    ).length >= 5,
  );
  assert.ok(
    discussion
      .filter((fixture) =>
        Object.keys(fixture.manifest.expected ?? {}).some((name) =>
          name.startsWith("reply_"),
        ),
      )
      .every((fixture) => fixture.manifest.reply_markers?.length > 0),
  );
  for (const id of [
    "discussion-sponsored-card",
    "discussion-pagination",
    "social-thread-media-links",
  ]) {
    assert.ok(discussion.some((fixture) => fixture.manifest.id === id));
  }
  for (const id of [
    "discussion-code-reply",
    "discussion-quote-reply",
    "discussion-list-reply",
    "social-thread-media-links",
  ]) {
    assert.ok(discussion.some((fixture) => fixture.manifest.id === id));
  }
});

test("keeps the application and responsive benchmark batch complete", () => {
  const application = fixtures
    .filter(
      (fixture) =>
        fixture.manifest.id.startsWith("app-") ||
        fixture.manifest.id === "responsive-duplicate-copies",
    )
    .map((fixture) => fixture.manifest.id);
  assert.ok(application.length >= 15);
  assert.ok(application.includes("app-hidden-complete-content"));
  assert.ok(application.includes("app-hidden-junk-negative"));
  assert.ok(application.includes("app-empty-shell"));
  assert.ok(application.includes("app-template-content"));
});

test("keeps the media benchmark batch complete", () => {
  assert.ok(fixtureIds("media-").length >= 16);
});

test("keeps the metadata conflict benchmark batch complete", () => {
  const metadata = fixtures.filter((fixture) =>
    fixture.manifest.id.startsWith("metadata-"),
  );
  assert.ok(metadata.length >= 20);
  assert.ok(metadata.every((fixture) => fixture.expectedMetadata !== null));
});
