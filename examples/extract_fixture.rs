use legible::{Error, Extractor, SemanticCoverageCategory};
use std::{env, fs, process::ExitCode};

fn error_variant(error: &Error) -> &'static str {
    match error {
        Error::TooManyElements(..) => "TooManyElements",
        Error::NoContent => "NoContent",
        Error::NoBody => "NoBody",
        Error::InvalidUrl(..) => "InvalidUrl",
        Error::ContentRootNotFound => "ContentRootNotFound",
    }
}

#[cfg(test)]
mod tests {
    use super::error_variant;
    use legible::Error;

    #[test]
    fn error_variants_do_not_include_payloads() {
        let invalid_url = url::Url::parse("not a URL").unwrap_err();
        let cases = [
            (Error::TooManyElements(12, 10), "TooManyElements"),
            (Error::InvalidUrl(invalid_url), "InvalidUrl"),
        ];
        for (error, expected) in cases {
            assert_eq!(error_variant(&error), expected);
        }
    }
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
                        "word_count": page.word_count(),
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
