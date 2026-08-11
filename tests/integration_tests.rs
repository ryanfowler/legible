//! Content regression tests against Mozilla's Readability test suite.
//!
//! Mozilla's metadata expectations describe its old precedence rules. The checked-in
//! snapshot records Legible's richer resolved metadata for the same source pages.
//! Cases with deliberate liberal-retention differences use exact output fingerprints.
//! They also verify that Legible keeps at least 80% of Mozilla's expected words
//! and does not grow beyond five times the expected word count.

use html5ever::{parse_document, tendril::TendrilSink};
use legible::{Metadata, extract};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ExpectedMetadata {
    title: Option<String>,
    description: Option<String>,
    authors: Vec<String>,
    site_name: Option<String>,
    canonical_url: Option<String>,
    image: Option<String>,
    favicon: Option<String>,
    published_time: Option<String>,
    modified_time: Option<String>,
    language: Option<String>,
    direction: Option<String>,
    section: Option<String>,
    tags: Vec<String>,
}

impl From<&Metadata> for ExpectedMetadata {
    fn from(metadata: &Metadata) -> Self {
        Self {
            title: metadata.title.clone(),
            description: metadata.description.clone(),
            authors: metadata.authors.clone(),
            site_name: metadata.site_name.clone(),
            canonical_url: metadata.canonical_url.clone(),
            image: metadata.image.clone(),
            favicon: metadata.favicon.clone(),
            published_time: metadata.published_time.clone(),
            modified_time: metadata.modified_time.clone(),
            language: metadata.language.clone(),
            direction: metadata.direction.clone(),
            section: metadata.section.clone(),
            tags: metadata.tags.clone(),
        }
    }
}

type CanonicalAttribute = (String, String, String);

#[derive(Debug, PartialEq, Eq)]
enum CanonicalNode {
    StartTag(String, Vec<CanonicalAttribute>),
    EndTag(String),
    Text(String),
}

fn canonical_tag(name: &str) -> String {
    // Legible preserves source heading levels. Mozilla rewrites H1 to H2 for
    // Reader Mode styling. Treat only that known product difference as equal.
    if matches!(name, "h1" | "h2") {
        "primary-heading".to_owned()
    } else {
        name.to_owned()
    }
}

fn append_canonical(root: &Handle, nodes: &mut Vec<CanonicalNode>) {
    let mut stack = vec![(root.clone(), false)];
    while let Some((node, closing)) = stack.pop() {
        match &node.data {
            NodeData::Element { name, attrs, .. } => {
                if closing {
                    nodes.push(CanonicalNode::EndTag(canonical_tag(name.local.as_ref())));
                    continue;
                }

                let attrs = attrs.borrow();
                let has_data_srcset = attrs.iter().any(|attribute| {
                    attribute.name.ns.as_ref().is_empty()
                        && attribute.name.local.as_ref() == "data-srcset"
                });
                let has_srcset = attrs.iter().any(|attribute| {
                    attribute.name.ns.as_ref().is_empty()
                        && attribute.name.local.as_ref() == "srcset"
                });
                let mut attributes: Vec<_> = attrs
                    .iter()
                    .filter(|attribute| {
                        let local = attribute.name.local.as_ref();
                        // Official fixtures use different selected-container IDs and
                        // retain unrelated runtime data. Keep page IDs, image data,
                        // and generated data-old-* attributes under comparison.
                        !(local == "id"
                            && !matches!(
                                attribute.value.as_ref(),
                                "readability-page-1" | "legible-content"
                            ))
                            && !(attribute.name.ns.as_ref().is_empty()
                                && local.starts_with("data-")
                                && !matches!(local, "data-src" | "data-srcset")
                                && !local.starts_with("data-old-"))
                            // Some Mozilla fixtures retain the noscript image src
                            // where this implementation retains data-srcset.
                            && !(name.local.as_ref() == "img"
                                && local == "src"
                                && (has_data_srcset || has_srcset))
                    })
                    .map(|attribute| {
                        let mut value = attribute.value.to_string();
                        if attribute.name.local.as_ref() == "id"
                            && matches!(value.as_str(), "readability-page-1" | "legible-content")
                        {
                            value = "content-root".to_owned();
                        }
                        if attribute.name.local.as_ref() == "src"
                            && value.starts_with("file:///C|/")
                        {
                            value.replace_range(9..10, ":");
                        }
                        (
                            attribute.name.ns.to_string(),
                            attribute.name.local.to_string(),
                            value,
                        )
                    })
                    .collect();
                attributes.sort();
                nodes.push(CanonicalNode::StartTag(
                    canonical_tag(name.local.as_ref()),
                    attributes,
                ));
                stack.push((node.clone(), true));
                stack.extend(
                    node.children
                        .borrow()
                        .iter()
                        .rev()
                        .map(|child| (child.clone(), false)),
                );
            }
            NodeData::Text { contents } => {
                let normalized = contents
                    .borrow()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if !normalized.is_empty() {
                    nodes.push(CanonicalNode::Text(normalized));
                }
            }
            _ => stack.extend(
                node.children
                    .borrow()
                    .iter()
                    .rev()
                    .map(|child| (child.clone(), false)),
            ),
        }
    }
}

fn canonicalize_html(html: &str) -> Vec<CanonicalNode> {
    let dom = parse_document(RcDom::default(), Default::default()).one(html);
    let body = dom
        .document
        .children
        .borrow()
        .iter()
        .find_map(|html| {
            html.children
                .borrow()
                .iter()
                .find(|node| {
                    matches!(&node.data, NodeData::Element { name, .. } if name.local.as_ref() == "body")
                })
                .cloned()
        })
        .expect("HTML parser did not create a body element");

    let mut nodes = Vec::new();
    for child in body.children.borrow().iter() {
        append_canonical(child, &mut nodes);
    }
    nodes
}

fn compare_html(expected: &str, actual: &str) -> Result<(), String> {
    let expected = canonicalize_html(expected);
    let actual = canonicalize_html(actual);
    if expected == actual {
        return Ok(());
    }

    let mismatch = expected
        .iter()
        .zip(&actual)
        .position(|(expected, actual)| expected != actual)
        .unwrap_or(expected.len().min(actual.len()));
    Err(format!(
        "Content mismatch at canonical node {mismatch}:\n  Expected: {:?}\n  Got: {:?}",
        expected.get(mismatch),
        actual.get(mismatch)
    ))
}

fn compare_retained_words(expected: &str, actual: &str) -> Result<(), String> {
    let expected = canonicalize_html(expected);
    let actual = canonicalize_html(actual);
    let expected_words: Vec<_> = expected
        .iter()
        .filter_map(|node| match node {
            CanonicalNode::Text(text) => Some(text.split_whitespace()),
            _ => None,
        })
        .flatten()
        .collect();
    let actual_words: Vec<_> = actual
        .iter()
        .filter_map(|node| match node {
            CanonicalNode::Text(text) => Some(text.split_whitespace()),
            _ => None,
        })
        .flatten()
        .collect();
    let expected_count = expected_words.len();
    let actual_count = actual_words.len();
    let mut actual_counts = HashMap::new();
    for word in actual_words {
        *actual_counts.entry(word).or_insert(0_usize) += 1;
    }
    let retained = expected_words
        .into_iter()
        .filter(|expected| {
            let Some(count) = actual_counts.get_mut(expected) else {
                return false;
            };
            if *count == 0 {
                return false;
            }
            *count -= 1;
            true
        })
        .count();
    let coverage = if expected_count == 0 {
        1.0
    } else {
        retained as f64 / expected_count as f64
    };
    if coverage < 0.80 {
        let percentage = coverage * 100.0;
        return Err(format!(
            "Liberal extraction retained only {percentage:.1}% of expected words"
        ));
    }
    if expected_count > 0 && actual_count > expected_count.saturating_mul(5) {
        return Err(format!(
            "Liberal extraction grew from {expected_count} to {actual_count} words"
        ));
    }
    Ok(())
}

fn output_fingerprint(html: &str) -> String {
    // Keep the established liberal-output snapshots stable across the neutral
    // product marker rename.
    let html = html.replace("legible-content", "readability-page-1");
    let hash = html.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

fn run_test_case(source_path: &Path) -> datatest_stable::Result<()> {
    let test_dir = source_path.parent().unwrap();

    let expected_path = test_dir.join("expected.html");

    // Read source HTML
    let source_html = fs::read_to_string(source_path)?;

    // Read expected HTML if it exists
    let expected_html = if expected_path.exists() {
        Some(fs::read_to_string(&expected_path)?)
    } else {
        None
    };

    let page = extract(&source_html, Some("http://fakehost/test/page.html"))?;
    let snapshots: HashMap<String, ExpectedMetadata> =
        serde_json::from_str(include_str!("metadata-snapshots.json"))?;
    let case = test_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let expected_metadata = snapshots
        .get(case)
        .ok_or_else(|| format!("Missing metadata snapshot for {case}"))?;
    let actual_metadata = ExpectedMetadata::from(page.metadata());
    if expected_metadata != &actual_metadata {
        return Err(format!(
            "Metadata mismatch for {case}:\n  Expected: {expected_metadata:#?}\n  Got: {actual_metadata:#?}"
        )
        .into());
    }

    if let Some(expected) = expected_html {
        let actual = page.html();
        let liberal_snapshots: HashMap<String, String> =
            serde_json::from_str(include_str!("liberal-output-snapshots.json"))?;
        if let Some(snapshot) = liberal_snapshots.get(case) {
            compare_retained_words(&expected, &actual)?;
            let actual_fingerprint = output_fingerprint(&actual);
            if snapshot != &actual_fingerprint {
                return Err(format!(
                    "Liberal output changed for {case}: expected {snapshot}, got {actual_fingerprint}"
                )
                .into());
            }
        } else {
            compare_html(&expected, &actual)?;
        }
    }

    Ok(())
}

datatest_stable::harness! {
    { test = run_test_case, root = "tests/readability-js/test/test-pages", pattern = r".*/source\.html$" },
}
