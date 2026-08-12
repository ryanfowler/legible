//! Specialized page extraction fixtures.

use legible::extract;
use std::fs;
use std::path::Path;

fn normalize_newlines(value: &str) -> String {
    let value = value.replace("\r\n", "\n").replace('\r', "\n");
    format!("{}\n", value.trim_end())
}

fn run_fixture(source_path: &Path) -> datatest_stable::Result<()> {
    let directory = source_path.parent().expect("fixture has no directory");
    let source = fs::read_to_string(source_path)?;
    let url = fs::read_to_string(directory.join("url.txt"))?;
    let page = extract(&source, Some(url.trim()))?;
    let actual = normalize_newlines(&page.markdown());
    let expected = normalize_newlines(&fs::read_to_string(directory.join("expected.md"))?);
    if actual != expected {
        return Err(format!(
            "Markdown mismatch in {}:\n--- expected ---\n{}--- actual ---\n{}",
            directory.display(),
            expected,
            actual
        )
        .into());
    }
    Ok(())
}

datatest_stable::harness! {
    { test = run_fixture, root = "tests/specialized", pattern = r".*/source\.html$" },
}
