//! Repository-owned snapshot and capability fixtures.

use legible::{Error, Metadata, extract};
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
    favicon: Option<String>,
    published_time: Option<String>,
    modified_time: Option<String>,
    language: Option<String>,
    direction: Option<String>,
    section: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedCapabilities {
    title: Option<String>,
    authors: Option<Vec<String>>,
    word_count_min: Option<usize>,
    must_contain: Option<Vec<String>>,
    must_not_contain: Option<Vec<String>>,
    must_occur_once: Option<Vec<String>>,
    html_must_contain: Option<Vec<String>>,
    html_must_not_contain: Option<Vec<String>>,
    headings_min: Option<usize>,
    images_min: Option<usize>,
    code_blocks_min: Option<usize>,
    tables_min: Option<usize>,
}

fn normalize_newlines(value: &str) -> String {
    let value = value.replace("\r\n", "\n").replace('\r', "\n");
    format!("{}\n", value.trim_end())
}

fn source_url(directory: &Path) -> Result<String, std::io::Error> {
    let path = directory.join("url.txt");
    if path.exists() {
        return fs::read_to_string(path).map(|url| url.trim().to_owned());
    }
    if directory
        .components()
        .any(|component| component.as_os_str() == "compatibility-defuddle")
    {
        Ok("https://example.test/defuddle-fixture".to_owned())
    } else {
        Ok("https://example.test/docs/page.html".to_owned())
    }
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
    check!(favicon);
    check!(published_time);
    check!(modified_time);
    check!(language);
    check!(direction);
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

fn error_name(error: &Error) -> &'static str {
    match error {
        Error::TooManyElements { .. } => "TooManyElements",
        Error::ResourceLimit { .. } => "ResourceLimit",
        Error::Parse(_) => "Parse",
        Error::NoContent => "NoContent",
        Error::NoBody => "NoBody",
        Error::InvalidUrl(_) => "InvalidUrl",
        Error::ContentRootNotFound => "ContentRootNotFound",
    }
}

fn run_snapshot(source_path: &Path) -> datatest_stable::Result<()> {
    let directory = source_path.parent().expect("fixture has no directory");
    let source = fs::read_to_string(source_path)?;
    let url = source_url(directory)?;
    let result = extract(&source, Some(&url));
    let error_path = directory.join("expected.error");
    let markdown_path = directory.join("expected.md");

    if error_path.exists() && markdown_path.exists() {
        return Err(format!(
            "fixture {} has both expected.error and expected.md",
            directory.display()
        )
        .into());
    }
    if error_path.exists() {
        let expected = fs::read_to_string(error_path)?;
        return match result {
            Err(error) if error_name(&error) == expected.trim() => Ok(()),
            Err(error) => Err(format!(
                "error mismatch in {}: expected {}, got {}",
                directory.display(),
                expected.trim(),
                error_name(&error)
            )
            .into()),
            Ok(_) => Err(format!(
                "error mismatch in {}: expected {}, extraction succeeded",
                directory.display(),
                expected.trim()
            )
            .into()),
        };
    }
    if !markdown_path.exists() {
        return Err(format!("fixture {} has no expected result", directory.display()).into());
    }

    let page = result?;
    let actual = normalize_newlines(&page.markdown());
    if std::env::var("LEGIBLE_UPDATE_FIXTURES").as_deref() == Ok("1") {
        fs::write(&markdown_path, &actual)?;
    } else {
        let expected = normalize_newlines(&fs::read_to_string(&markdown_path)?);
        if expected != actual {
            return Err(format!(
                "Markdown mismatch in {}:\n--- expected ---\n{}--- actual ---\n{}",
                directory.display(),
                expected,
                actual
            )
            .into());
        }
    }

    let metadata_path = directory.join("metadata.json");
    if metadata_path.exists() {
        let expected: ExpectedMetadata = serde_json::from_str(&fs::read_to_string(metadata_path)?)?;
        compare_metadata(&expected, page.metadata())?;
    }
    Ok(())
}

fn occurrences(value: &str, pattern: &str) -> usize {
    value.match_indices(pattern).count()
}

fn run_capability(source_path: &Path) -> datatest_stable::Result<()> {
    let directory = source_path.parent().expect("fixture has no directory");
    let source = fs::read_to_string(source_path)?;
    let expected: ExpectedCapabilities =
        serde_json::from_str(&fs::read_to_string(directory.join("expected.json"))?)?;
    let url = source_url(directory)?;
    let page = extract(&source, Some(&url))?;
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
    let html = page.html();
    for text in expected.html_must_contain.unwrap_or_default() {
        if !html.contains(&text) {
            return Err(format!("HTML output does not contain {text:?}:\n{html}").into());
        }
    }
    for text in expected.html_must_not_contain.unwrap_or_default() {
        if html.contains(&text) {
            return Err(format!("HTML output contains excluded {text:?}:\n{html}").into());
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
        (expected.tables_min, occurrences(&html, "<table"), "tables"),
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
    { test = run_snapshot, root = "tests/fixtures/snapshots", pattern = r".*/source\.html$" },
    { test = run_capability, root = "tests/fixtures/capabilities", pattern = r".*/source\.html$" },
}
