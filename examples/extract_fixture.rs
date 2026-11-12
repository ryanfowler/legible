use legible::{Error, Extractor, SemanticCoverageCategory};
use std::{env, fs, process::ExitCode};

fn error_variant(error: &Error) -> &'static str {
    match error {
        Error::ResourceLimit { .. } => "ResourceLimit",
        Error::Parse(_) => "Parse",
        Error::NoContent => "NoContent",
        Error::NoBody => "NoBody",
        Error::InvalidUrl(..) => "InvalidUrl",
        Error::ContentRootNotFound => "ContentRootNotFound",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::error_variant;
    use legible::Extractor;

    #[test]
    fn counts_repeated_paragraphs_from_canonical_html() {
        let paragraph = "The harbour opened at dawn and the first ferry left on time.";
        let html = format!(
            "<p>{paragraph}</p><pre><code>{paragraph}</code></pre>\
             <p><img alt=\"{paragraph}\" src=\"a.png\"></p><p>Reply</p><p>Reply</p>"
        );
        assert_eq!(super::duplicate_paragraphs(&html), 0);
        let html =
            format!("<p>{paragraph}</p><blockquote><p><em>{paragraph}</em></p></blockquote>");
        assert_eq!(super::duplicate_paragraphs(&html), 1);

        let count = |first: &str, second: &str| {
            super::duplicate_paragraphs(&format!("<p>{first}</p><p>{second}</p>"))
        };
        // Inline formatting does not change the text.
        let emphasized = paragraph.replace("time.", "<em>time</em>.");
        assert_eq!(count(paragraph, &emphasized), 1);
        let partial = paragraph.replace("harbour", "har<b>bour</b>");
        assert_eq!(count(paragraph, &partial), 1);
        // A hard break separates words.
        let broken = paragraph.replace("opened at", "opened<br>at");
        assert_eq!(count(paragraph, &broken), 1);
        let joined = paragraph.replace("opened at", "openedat");
        assert_eq!(count(&broken, &joined), 0);
    }

    #[test]
    fn error_variants_have_stable_names() {
        let html = "<main><p>Content</p></main>";
        let cases = [
            (
                Extractor::builder()
                    .parse_budget(legible::ParseBudget {
                        max_elements: 1,
                        ..Default::default()
                    })
                    .build()
                    .extract(html, None),
                "ResourceLimit",
            ),
            (legible::extract(html, Some("not a URL")), "InvalidUrl"),
        ];
        for (result, expected) in cases {
            assert_eq!(error_variant(&result.unwrap_err()), expected);
        }
    }
}

/// Shortest normalized paragraph, in characters, that the duplicate count
/// compares. The quality evaluator uses the same value for Markdown.
const DUPLICATE_PARAGRAPH_MIN_CHARACTERS: usize = 40;

/// Counts `<p>` elements in canonical HTML whose normalized text repeats an
/// earlier paragraph. Canonical HTML has no attributes on `<p>`, and
/// paragraphs do not nest, so the structure is exact: code blocks, image alt
/// text, and Markdown syntax cannot cause a false duplicate.
fn duplicate_paragraphs(html: &str) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut duplicates = 0;
    for (start, _) in html.match_indices("<p>") {
        let body = &html[start + 3..];
        let Some(end) = body.find("</p>") else {
            break;
        };
        // Inline tags keep their text adjacent, so `<em>time</em>.` reads as
        // `time.`. A `<br>` separates words.
        let mut text = String::new();
        let mut rest = &body[..end];
        while let Some(open) = rest.find('<') {
            text.push_str(&rest[..open]);
            let Some(close) = rest[open..].find('>') else {
                rest = "";
                break;
            };
            let tag = &rest[open + 1..open + close];
            if tag.trim_start_matches('/').starts_with("br") {
                text.push(' ');
            }
            rest = &rest[open + close + 1..];
        }
        text.push_str(rest);
        let text = text
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&");
        let normalized = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if normalized.chars().count() < DUPLICATE_PARAGRAPH_MIN_CHARACTERS {
            continue;
        }
        if !seen.insert(normalized) {
            duplicates += 1;
        }
    }
    duplicates
}

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1).peekable();
    let mut json = false;
    let mut source_url = String::from("https://example.test/docs/page.html");
    let mut path = None;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--json") => json = true,
            Some("--url") => {
                let Some(value) = arguments.next() else {
                    eprintln!("--url requires a value");
                    return ExitCode::FAILURE;
                };
                source_url = value.to_string_lossy().into_owned();
            }
            _ if path.is_none() => path = Some(argument),
            _ => {
                eprintln!("unexpected argument: {}", argument.to_string_lossy());
                return ExitCode::FAILURE;
            }
        }
    }
    let Some(path) = path else {
        eprintln!(
            "usage: cargo run --example extract_fixture -- [--json] [--url SOURCE_URL] SOURCE_HTML"
        );
        return ExitCode::FAILURE;
    };
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {}: {error}", path.to_string_lossy());
            return ExitCode::FAILURE;
        }
    };
    match Extractor::builder()
        .diagnostics(true)
        .build()
        .extract(&source, Some(&source_url))
    {
        Ok(page) => {
            let markdown = page.markdown();
            if json {
                let semantic_coverage = page
                    .diagnostics()
                    .and_then(|diagnostics| {
                        diagnostics.attempts.iter().find(|attempt| attempt.accepted)
                    })
                    .and_then(|attempt| attempt.semantic_coverage.as_ref())
                    .map(|coverage| {
                        serde_json::json!({
                            "score": coverage.score,
                            "categories": coverage.categories.iter().map(|category| {
                                let name = match category.category {
                                    SemanticCoverageCategory::CodeBlocks => "code_blocks",
                                    SemanticCoverageCategory::DataTables => "data_tables",
                                    SemanticCoverageCategory::SubstantialListItems => "substantial_list_items",
                                    SemanticCoverageCategory::Visuals => "visuals",
                                    SemanticCoverageCategory::Headings => "headings",
                                    SemanticCoverageCategory::FootnoteDefinitions => "footnote_definitions",
                                    SemanticCoverageCategory::MathExpressions => "math_expressions",
                                    _ => "unknown",
                                };
                                serde_json::json!({
                                    "category": name,
                                    "source_count": category.source_count,
                                    "result_count": category.result_count,
                                    "coverage": category.coverage,
                                })
                            }).collect::<Vec<_>>(),
                        })
                    });
                println!(
                    "{}",
                    serde_json::json!({
                        "markdown": markdown,
                        "word_count": page.content().metrics().word_count,
                        "metadata": {
                            "title": &page.metadata().title,
                            "description": &page.metadata().description,
                            "authors": &page.metadata().authors,
                            "site_name": &page.metadata().site_name,
                            "canonical_url": &page.metadata().canonical_url,
                            "image": &page.metadata().image,
                            "favicon": &page.metadata().favicon,
                            "published_time": &page.metadata().published_time,
                            "modified_time": &page.metadata().modified_time,
                            "language": &page.metadata().language,
                            "direction": &page.metadata().direction,
                            "section": &page.metadata().section,
                            "tags": &page.metadata().tags,
                        },
                        "duplicate_paragraphs": duplicate_paragraphs(&page.html()),
                        "tables": page.html().match_indices("<table").count(),
                        "figures": page.html().match_indices("<figure").count(),
                        "semantic_coverage": semantic_coverage,
                    })
                );
            } else {
                print!("{markdown}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "success": false,
                        "error": {
                            "kind": "extraction",
                            "message": error.to_string(),
                            "variant": error_variant(&error),
                        },
                    })
                );
                return ExitCode::SUCCESS;
            }
            eprintln!("extraction failed: {error}");
            ExitCode::FAILURE
        }
    }
}
