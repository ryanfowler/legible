//! Capability-focused web extraction fixtures.

use legible::extract;
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    title: Option<String>,
    authors: Option<Vec<String>>,
    word_count_min: Option<usize>,
    must_contain: Option<Vec<String>>,
    must_not_contain: Option<Vec<String>>,
    must_occur_once: Option<Vec<String>>,
    headings_min: Option<usize>,
    images_min: Option<usize>,
    code_blocks_min: Option<usize>,
    tables_min: Option<usize>,
}

fn occurrences(value: &str, pattern: &str) -> usize {
    value.match_indices(pattern).count()
}

fn run_fixture(source_path: &Path) -> datatest_stable::Result<()> {
    let directory = source_path.parent().expect("fixture has no directory");
    let source = fs::read_to_string(source_path)?;
    let expected: Expected =
        serde_json::from_str(&fs::read_to_string(directory.join("expected.json"))?)?;
    let page = extract(&source, Some("https://example.test/docs/page.html"))?;
    let markdown = page.markdown();

    if let Some(title) = expected.title
        && page.metadata().title.as_deref() != Some(title.as_str())
    {
        return Err(format!("expected title {title:?}, got {:?}", page.metadata().title).into());
    }
    if let Some(authors) = expected.authors
        && page.metadata().authors != authors
    {
        return Err(format!(
            "expected authors {authors:?}, got {:?}",
            page.metadata().authors
        )
        .into());
    }
    if let Some(minimum) = expected.word_count_min
        && page.word_count() < minimum
    {
        return Err(format!(
            "expected at least {minimum} words, got {}",
            page.word_count()
        )
        .into());
    }
    for text in expected.must_contain.unwrap_or_default() {
        if !markdown.contains(&text) {
            return Err(format!("output does not contain {text:?}:\n{markdown}").into());
        }
    }
    for text in expected.must_not_contain.unwrap_or_default() {
        if markdown.contains(&text) {
            return Err(format!("output contains excluded text {text:?}:\n{markdown}").into());
        }
    }
    for text in expected.must_occur_once.unwrap_or_default() {
        let count = occurrences(&markdown, &text);
        if count != 1 {
            return Err(format!("expected {text:?} once, got {count}:\n{markdown}").into());
        }
    }
    for (minimum, actual, label) in [
        (
            expected.headings_min,
            occurrences(&markdown, "# "),
            "headings",
        ),
        (expected.images_min, occurrences(&markdown, "!["), "images"),
        (
            expected.code_blocks_min,
            occurrences(&markdown, "```") / 2,
            "code blocks",
        ),
        (
            expected.tables_min,
            occurrences(&page.html(), "<table"),
            "tables",
        ),
    ] {
        if minimum.is_some_and(|minimum| actual < minimum) {
            return Err(format!(
                "expected at least {} {label}, got {actual}",
                minimum.unwrap()
            )
            .into());
        }
    }
    Ok(())
}

datatest_stable::harness! {
    { test = run_fixture, root = "tests/web", pattern = r".*/source\.html$" },
}
