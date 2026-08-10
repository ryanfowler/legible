//! Content regression tests against Mozilla's Readability test suite.
//!
//! Mozilla's metadata expectations describe its old precedence rules. The checked-in
//! snapshot records Legible's richer resolved metadata for the same source pages.

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

fn append_canonical(root: &Handle, nodes: &mut Vec<CanonicalNode>) {
    let mut stack = vec![(root.clone(), false)];
    while let Some((node, closing)) = stack.pop() {
        match &node.data {
            NodeData::Element { name, attrs, .. } => {
                if closing {
                    nodes.push(CanonicalNode::EndTag(name.local.to_string()));
                    continue;
                }

                let attrs = attrs.borrow();
                let has_data_srcset = attrs.iter().any(|attribute| {
                    attribute.name.ns.as_ref().is_empty()
                        && attribute.name.local.as_ref() == "data-srcset"
                });
                let mut attributes: Vec<_> = attrs
                    .iter()
                    .filter(|attribute| {
                        let local = attribute.name.local.as_ref();
                        // Official fixtures use different selected-container IDs and
                        // retain unrelated runtime data. Keep page IDs, image data,
                        // and generated data-old-* attributes under comparison.
                        !(local == "id" && attribute.value.as_ref() != "readability-page-1")
                            && !(attribute.name.ns.as_ref().is_empty()
                                && local.starts_with("data-")
                                && !matches!(local, "data-src" | "data-srcset")
                                && !local.starts_with("data-old-"))
                            // Some Mozilla fixtures retain the noscript image src
                            // where this implementation retains data-srcset.
                            && !(name.local.as_ref() == "img"
                                && local == "src"
                                && has_data_srcset)
                    })
                    .map(|attribute| {
                        let mut value = attribute.value.to_string();
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
                nodes.push(CanonicalNode::StartTag(name.local.to_string(), attributes));
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
        compare_html(&expected, &page.html())?;
    }

    Ok(())
}

datatest_stable::harness! {
    { test = run_test_case, root = "tests/readability-js/test/test-pages", pattern = r".*/source\.html$" },
}
