//! Markdown compatibility fixtures derived from Defuddle behavior.

use legible::extract;
use std::fs;
use std::path::Path;

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

    Ok(())
}

datatest_stable::harness! {
    { test = run_fixture, root = "tests/defuddle", pattern = r".*/source\.html$" },
}
