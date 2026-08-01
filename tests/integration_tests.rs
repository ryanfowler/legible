//! Integration tests against Mozilla's readability test suite.

use legible::parse;
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedMetadata {
    title: Option<String>,
    byline: Option<String>,
    excerpt: Option<String>,
    site_name: Option<String>,
    #[serde(default)]
    published_time: Option<String>,
    dir: Option<String>,
    lang: Option<String>,
    #[allow(dead_code)]
    readerable: Option<bool>,
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_text_content(html: &str) -> String {
    let re = regex::Regex::new(r"<[^>]+>").unwrap();
    let text = re.replace_all(html, " ");
    normalize_whitespace(&text)
}

fn text_similarity(a: &str, b: &str) -> f64 {
    let words_a: std::collections::HashSet<_> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<_> = b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f64 / union as f64
}

fn run_test_case(source_path: &Path) -> datatest_stable::Result<()> {
    let test_dir = source_path.parent().unwrap();
    let test_name = test_dir.file_name().unwrap().to_str().unwrap();

    let expected_path = test_dir.join("expected.html");
    let metadata_path = test_dir.join("expected-metadata.json");

    // Read source HTML
    let source_html = fs::read_to_string(source_path)?;

    // Read expected HTML if it exists
    let expected_html = if expected_path.exists() {
        Some(fs::read_to_string(&expected_path)?)
    } else {
        None
    };

    // Read expected metadata
    let expected_metadata: Option<ExpectedMetadata> = if metadata_path.exists() {
        let metadata_str = fs::read_to_string(&metadata_path)?;
        Some(serde_json::from_str(&metadata_str)?)
    } else {
        None
    };

    // Run Readability
    let article = parse(
        &source_html,
        Some(&format!("http://fakehost/test/{}", test_name)),
        None,
    )?;

    // Check metadata
    if let Some(expected) = expected_metadata {
        if let Some(expected_title) = expected.title
            && article.title != expected_title
        {
            return Err(format!(
                "Title mismatch:\n  Expected: {}\n  Got: {}",
                expected_title, article.title
            )
            .into());
        }

        if let Some(expected_byline) = expected.byline
            && article.byline.as_deref() != Some(&expected_byline)
        {
            return Err(format!(
                "Byline mismatch:\n  Expected: {}\n  Got: {:?}",
                expected_byline, article.byline
            )
            .into());
        }

        if let Some(expected_excerpt) = expected.excerpt {
            let got_excerpt = article.excerpt.as_deref().unwrap_or("");
            if normalize_whitespace(&expected_excerpt) != normalize_whitespace(got_excerpt) {
                return Err(format!(
                    "Excerpt mismatch:\n  Expected: {}\n  Got: {:?}",
                    expected_excerpt, article.excerpt
                )
                .into());
            }
        }

        if let Some(expected_site_name) = expected.site_name
            && article.site_name.as_deref() != Some(&expected_site_name)
        {
            return Err(format!(
                "Site name mismatch:\n  Expected: {}\n  Got: {:?}",
                expected_site_name, article.site_name
            )
            .into());
        }

        if let Some(expected_published_time) = expected.published_time
            && article.published_time.as_deref() != Some(&expected_published_time)
        {
            return Err(format!(
                "Published time mismatch:\n  Expected: {}\n  Got: {:?}",
                expected_published_time, article.published_time
            )
            .into());
        }

        if let Some(expected_dir) = expected.dir
            && article.dir.as_deref() != Some(&expected_dir)
        {
            return Err(format!(
                "Direction mismatch:\n  Expected: {}\n  Got: {:?}",
                expected_dir, article.dir
            )
            .into());
        }

        if let Some(expected_lang) = expected.lang
            && article.lang.as_deref() != Some(&expected_lang)
        {
            return Err(format!(
                "Language mismatch:\n  Expected: {}\n  Got: {:?}",
                expected_lang, article.lang
            )
            .into());
        }
    }

    // Check content by comparing text (to avoid HTML serialization differences)
    if let Some(expected) = expected_html {
        let expected_text = extract_text_content(&expected);
        let got_text = extract_text_content(&article.html());

        let similarity = text_similarity(&expected_text, &got_text);
        if similarity < 0.9 {
            let expected_preview: String = expected_text.chars().take(200).collect();
            let got_preview: String = got_text.chars().take(200).collect();
            return Err(format!(
                "Content text similarity too low: {:.2}%\n  Expected text: {}...\n  Got text: {}...",
                similarity * 100.0,
                expected_preview,
                got_preview
            )
            .into());
        }
    }

    Ok(())
}

datatest_stable::harness! {
    { test = run_test_case, root = "tests/readability-js/test/test-pages", pattern = r".*/source\.html$" },
}
