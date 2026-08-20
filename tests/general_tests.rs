//! Authoritative Markdown fixtures for general content extraction.

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

fn normalize_newlines(value: &str) -> String {
    let value = value.replace("\r\n", "\n").replace('\r', "\n");
    format!("{}\n", value.trim_end())
}

fn compare_metadata(expected: &ExpectedMetadata, actual: &Metadata) -> Result<(), String> {
    macro_rules! check {
        ($field:ident) => {
            if let Some(expected) = &expected.$field {
                if Some(expected.as_str()) != actual.$field.as_deref() {
                    return Err(format!(
                        "metadata field {}: expected {:?}, got {:?}",
                        stringify!($field),
                        expected,
                        actual.$field
                    ));
                }
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
        Error::TooManyElements(..) => "TooManyElements",
        Error::ResourceLimit { .. } => "ResourceLimit",
        Error::Parse(_) => "Parse",
        Error::NoContent => "NoContent",
        Error::NoBody => "NoBody",
        Error::InvalidUrl(_) => "InvalidUrl",
        Error::ContentRootNotFound => "ContentRootNotFound",
    }
}

fn run_fixture(source_path: &Path) -> datatest_stable::Result<()> {
    let directory = source_path.parent().expect("fixture has no directory");
    let source = fs::read_to_string(source_path)?;
    let url_path = directory.join("url.txt");
    let source_url = if url_path.exists() {
        fs::read_to_string(url_path)?
    } else {
        "https://example.test/docs/page.html".to_owned()
    };
    let result = extract(&source, Some(source_url.trim()));
    let error_path = directory.join("expected.error");
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

    let page = result?;
    let actual = normalize_newlines(&page.markdown());
    let expected_path = directory.join("expected.md");

    if std::env::var("LEGIBLE_UPDATE_FIXTURES").as_deref() == Ok("1") {
        fs::write(expected_path, &actual)?;
    } else {
        let expected = normalize_newlines(&fs::read_to_string(expected_path)?);
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

datatest_stable::harness! {
    { test = run_fixture, root = "tests/general", pattern = r".*/source\.html$" },
}
