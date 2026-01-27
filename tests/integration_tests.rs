//! Integration tests against Mozilla's readability test suite.

use legible::Readability;
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
    #[allow(dead_code)]
    #[serde(default)]
    published_time: Option<String>,
    #[allow(dead_code)]
    dir: Option<String>,
    #[allow(dead_code)]
    lang: Option<String>,
    #[allow(dead_code)]
    readerable: Option<bool>,
}

fn get_test_pages_dir() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest_dir).join("tests/readability-js/test/test-pages")
}

fn normalize_whitespace(s: &str) -> String {
    // Normalize whitespace for comparison
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_text_content(html: &str) -> String {
    // Simple text extraction - remove tags and normalize whitespace
    let re = regex::Regex::new(r"<[^>]+>").unwrap();
    let text = re.replace_all(html, " ");
    normalize_whitespace(&text)
}

/// Run a single test case
fn run_test_case(test_dir: &Path) -> Result<(), String> {
    let test_name = test_dir.file_name().unwrap().to_str().unwrap();

    let source_path = test_dir.join("source.html");
    let expected_path = test_dir.join("expected.html");
    let metadata_path = test_dir.join("expected-metadata.json");

    // Read source HTML
    let source_html = fs::read_to_string(&source_path)
        .map_err(|e| format!("Failed to read source.html: {}", e))?;

    // Read expected HTML if it exists
    let expected_html = if expected_path.exists() {
        Some(
            fs::read_to_string(&expected_path)
                .map_err(|e| format!("Failed to read expected.html: {}", e))?,
        )
    } else {
        None
    };

    // Read expected metadata
    let expected_metadata: Option<ExpectedMetadata> = if metadata_path.exists() {
        let metadata_str = fs::read_to_string(&metadata_path)
            .map_err(|e| format!("Failed to read expected-metadata.json: {}", e))?;
        Some(
            serde_json::from_str(&metadata_str)
                .map_err(|e| format!("Failed to parse metadata: {}", e))?,
        )
    } else {
        None
    };

    // Run Readability
    let mut readability = Readability::new(
        &source_html,
        Some(&format!("http://fakehost/test/{}", test_name)),
        None,
    );
    let result = readability.parse();

    match result {
        Ok(article) => {
            // Check metadata
            if let Some(expected) = expected_metadata {
                if let Some(expected_title) = expected.title
                    && article.title != expected_title
                {
                    return Err(format!(
                        "Title mismatch:\n  Expected: {}\n  Got: {}",
                        expected_title, article.title
                    ));
                }

                if let Some(expected_byline) = expected.byline
                    && article.byline.as_deref() != Some(&expected_byline)
                {
                    return Err(format!(
                        "Byline mismatch:\n  Expected: {}\n  Got: {:?}",
                        expected_byline, article.byline
                    ));
                }

                if let Some(expected_excerpt) = expected.excerpt {
                    // Compare normalized excerpts
                    let got_excerpt = article.excerpt.as_deref().unwrap_or("");
                    if normalize_whitespace(&expected_excerpt) != normalize_whitespace(got_excerpt)
                    {
                        return Err(format!(
                            "Excerpt mismatch:\n  Expected: {}\n  Got: {:?}",
                            expected_excerpt, article.excerpt
                        ));
                    }
                }

                if let Some(expected_site_name) = expected.site_name
                    && article.site_name.as_deref() != Some(&expected_site_name)
                {
                    return Err(format!(
                        "Site name mismatch:\n  Expected: {}\n  Got: {:?}",
                        expected_site_name, article.site_name
                    ));
                }

                if let Some(expected_dir) = expected.dir
                    && article.dir.as_deref() != Some(&expected_dir)
                {
                    return Err(format!(
                        "Direction mismatch:\n  Expected: {}\n  Got: {:?}",
                        expected_dir, article.dir
                    ));
                }
            }

            // Check content by comparing text (to avoid HTML serialization differences)
            if let Some(expected) = expected_html {
                let expected_text = extract_text_content(&expected);
                let got_text = extract_text_content(&article.content);

                // Use a similarity check instead of exact match to handle minor differences
                let similarity = text_similarity(&expected_text, &got_text);
                if similarity < 0.9 {
                    // Use char indices for safe string slicing (handles Unicode)
                    let expected_preview: String = expected_text.chars().take(200).collect();
                    let got_preview: String = got_text.chars().take(200).collect();
                    return Err(format!(
                        "Content text similarity too low: {:.2}%\n  Expected text: {}...\n  Got text: {}...",
                        similarity * 100.0,
                        expected_preview,
                        got_preview
                    ));
                }
            }

            Ok(())
        }
        Err(e) => {
            // Some test cases might be expected to fail
            Err(format!("Readability parse failed: {}", e))
        }
    }
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

// Include generated test functions for each test case
include!(concat!(env!("OUT_DIR"), "/generated_tests.rs"));
