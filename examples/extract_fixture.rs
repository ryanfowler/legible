use legible::extract;
use std::{env, fs, process::ExitCode};

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let first = arguments.next();
    let (json, path) = match first.as_deref().and_then(|value| value.to_str()) {
        Some("--json") => (true, arguments.next()),
        _ => (false, first),
    };
    let Some(path) = path else {
        eprintln!("usage: cargo run --example extract_fixture -- [--json] SOURCE_HTML");
        return ExitCode::FAILURE;
    };
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {}: {error}", path.to_string_lossy());
            return ExitCode::FAILURE;
        }
    };
    match extract(&source, Some("https://example.test/docs/page.html")) {
        Ok(page) => {
            let markdown = page.markdown();
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "markdown": markdown,
                        "word_count": page.word_count(),
                        "metadata": {
                            "title": &page.metadata().title,
                            "authors": &page.metadata().authors,
                            "canonical_url": &page.metadata().canonical_url,
                        },
                        "tables": page.html().match_indices("<table").count(),
                    })
                );
            } else {
                print!("{markdown}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("extraction failed: {error}");
            ExitCode::FAILURE
        }
    }
}
