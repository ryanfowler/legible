use legible::extract;
use std::env;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn sources(directory: &Path, output: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .expect("read benchmark directory")
        .map(|entry| entry.expect("read directory entry"))
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, output);
        } else if path.file_name().is_some_and(|name| name == "source.html") {
            output.push(path);
        }
    }
}

fn extract_corpus(pages: &[(PathBuf, String)]) -> (usize, usize) {
    let mut markdown_bytes = 0;
    let mut errors = 0;
    for (_, html) in pages {
        match extract(black_box(html), Some("https://example.test/page")) {
            Ok(page) => markdown_bytes += black_box(page.markdown().len()),
            Err(_) => errors += 1,
        }
    }
    (markdown_bytes, errors)
}

fn main() {
    let mut arguments = env::args().skip(1);
    let root = PathBuf::from(
        arguments
            .next()
            .expect("usage: benchmark_corpus ROOT [ROUNDS]"),
    );
    let rounds: usize = arguments
        .next()
        .as_deref()
        .unwrap_or("3")
        .parse()
        .expect("ROUNDS must be an integer");
    assert!(rounds > 0, "ROUNDS must be greater than zero");

    let mut paths = Vec::new();
    sources(&root, &mut paths);
    let pages: Vec<_> = paths
        .iter()
        .map(|path| {
            (
                path.clone(),
                fs::read_to_string(path).expect("read source HTML"),
            )
        })
        .collect();

    for _ in 0..2 {
        black_box(extract_corpus(&pages));
    }

    let mut times = Vec::with_capacity(rounds);
    let mut result = (0, 0);
    for _ in 0..rounds {
        let start = Instant::now();
        result = extract_corpus(&pages);
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    let bytes: usize = pages.iter().map(|(_, html)| html.len()).sum();
    println!(
        "{{\"mode\":\"legible\",\"pages\":{},\"bytes\":{},\"median_ms\":{median:.3},\"per_page_ms\":{:.3},\"markdown_bytes\":{},\"errors\":{}}}",
        pages.len(),
        bytes,
        median / pages.len() as f64,
        result.0,
        result.1,
    );
}
