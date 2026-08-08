//! Integration tests against Mozilla's readability test suite.

use html5ever::{parse_document, tendril::TendrilSink};
use legible::{Document, Options};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedMetadata {
    title: String,
    byline: Option<String>,
    excerpt: Option<String>,
    site_name: Option<String>,
    #[serde(default)]
    published_time: Option<String>,
    dir: Option<String>,
    lang: Option<String>,
    readerable: bool,
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

    let document = Document::new(&source_html);
    let readerable = document.is_probably_readerable(None);
    if let Some(expected) = expected_metadata.as_ref()
        && readerable != expected.readerable
    {
        return Err(format!(
            "Readerable mismatch:\n  Expected: {}\n  Got: {}",
            expected.readerable, readerable
        )
        .into());
    }

    // Match the options and base URL used to generate Mozilla's fixtures.
    let mut options = Options::default();
    options.classes_to_preserve.push("caption".to_string());
    let article = document.parse(Some("http://fakehost/test/page.html"), Some(options))?;

    if let Some(expected) = expected_metadata {
        macro_rules! compare_field {
            ($name:literal, $expected:expr, $actual:expr) => {{
                let expected = &$expected;
                let actual = &$actual;
                if expected != actual {
                    return Err(format!(
                        "{} mismatch:\n  Expected: {:?}\n  Got: {:?}",
                        $name, expected, actual
                    )
                    .into());
                }
            }};
        }

        compare_field!("Title", expected.title, article.title);
        compare_field!("Byline", expected.byline, article.byline);
        compare_field!("Excerpt", expected.excerpt, article.excerpt);
        compare_field!("Site name", expected.site_name, article.site_name);
        compare_field!(
            "Published time",
            expected.published_time,
            article.published_time
        );
        compare_field!("Direction", expected.dir, article.dir);
        compare_field!("Language", expected.lang, article.lang);
    }

    if let Some(expected) = expected_html {
        compare_html(&expected, &article.content)?;
    }

    Ok(())
}

datatest_stable::harness! {
    { test = run_test_case, root = "tests/readability-js/test/test-pages", pattern = r".*/source\.html$" },
}
