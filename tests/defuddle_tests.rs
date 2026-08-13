//! Markdown compatibility fixtures derived from Defuddle behavior.

use legible::{Metadata, extract};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedMetadata {
    title: Option<String>,
    description: Option<String>,
    authors: Option<Vec<String>>,
    site_name: Option<String>,
    canonical_url: Option<String>,
    image: Option<String>,
    published_time: Option<String>,
    modified_time: Option<String>,
    section: Option<String>,
    tags: Option<Vec<String>>,
}

fn compare_metadata(expected: &ExpectedMetadata, actual: &Metadata) -> Result<(), String> {
    macro_rules! check {
        ($field:ident) => {
            if let Some(expected) = &expected.$field
                && Some(expected.as_str()) != actual.$field.as_deref()
            {
                return Err(format!(
                    "metadata field {}: expected {:?}, got {:?}",
                    stringify!($field),
                    expected,
                    actual.$field
                ));
            }
        };
    }
    check!(title);
    check!(description);
    check!(site_name);
    check!(canonical_url);
    check!(image);
    check!(published_time);
    check!(modified_time);
    check!(section);
    if let Some(expected) = &expected.authors
        && expected != &actual.authors
    {
        return Err(format!(
            "metadata field authors: expected {expected:?}, got {:?}",
            actual.authors
        ));
    }
    if let Some(expected) = &expected.tags
        && expected != &actual.tags
    {
        return Err(format!(
            "metadata field tags: expected {expected:?}, got {:?}",
            actual.tags
        ));
    }
    Ok(())
}

fn normalize_newlines(value: &str) -> String {
    let value = value.replace("\r\n", "\n").replace('\r', "\n");
    format!("{}\n", value.trim_end())
}

fn run_fixture(source_path: &Path) -> datatest_stable::Result<()> {
    let directory = source_path.parent().expect("fixture has no directory");
    let expected_path = directory.join("expected.md");
    let source = fs::read_to_string(source_path)?;
    let expected = normalize_newlines(&fs::read_to_string(&expected_path).map_err(|error| {
        format!(
            "cannot read expected Markdown for fixture {} at {}: {error}",
            directory.display(),
            expected_path.display()
        )
    })?);
    let page =
        extract(&source, Some("https://example.test/defuddle-fixture")).map_err(|error| {
            format!(
                "extraction failed for fixture {}: {error}",
                directory.display()
            )
        })?;
    let actual = normalize_newlines(&page.markdown());

    if expected != actual {
        return Err(format!(
            "Markdown mismatch in fixture {}:\n--- expected ---\n{}--- actual ---\n{}",
            directory.display(),
            expected,
            actual
        )
        .into());
    }

    let metadata_path = directory.join("metadata.json");
    if metadata_path.exists() {
        let expected: ExpectedMetadata = serde_json::from_str(&fs::read_to_string(metadata_path)?)?;
        compare_metadata(&expected, page.metadata())?;
    }

    Ok(())
}

datatest_stable::harness! {
    { test = run_fixture, root = "tests/defuddle", pattern = r".*/source\.html$" },
}
