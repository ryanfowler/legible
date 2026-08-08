use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use legible::{Document, Options, ReaderableOptions, is_probably_readerable, parse};
use std::fs;
use std::hint::black_box;

const TEST_PAGES_DIR: &str = "tests/readability-js/test/test-pages";

/// Load test page HTML from the Mozilla test suite
fn load_test_page(name: &str) -> String {
    let path = format!("{}/{}/source.html", TEST_PAGES_DIR, name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e))
}

/// Benchmark construction of the custom DOM without extraction work.
fn bench_dom_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("dom_parse");
    for name in ["medium-2", "wikipedia-2", "guardian-1"] {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &html, |b, html| {
            b.iter(|| Document::new(black_box(html)))
        });
    }
    group.finish();
}

/// Benchmark parsing articles of different sizes
fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");

    // Small articles (~1-2KB)
    let small_articles = [
        ("basic-tags-cleaning", "https://example.com"),
        ("replace-brs", "https://example.com"),
    ];

    // Medium articles (~50-80KB)
    let medium_articles = [
        ("medium-2", "https://medium.com"),
        ("ars-1", "https://arstechnica.com"),
        ("heise", "https://heise.de"),
    ];

    // Large articles (~500KB-1MB+)
    let large_articles = [
        ("nytimes-5", "https://nytimes.com"),
        ("wikipedia-2", "https://wikipedia.org"),
        ("yahoo-2", "https://yahoo.com"),
    ];

    // Benchmark small articles
    for (name, url) in small_articles {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new("small", name), &html, |b, html| {
            b.iter(|| parse(black_box(html), Some(url), None))
        });
    }

    // Benchmark medium articles
    for (name, url) in medium_articles {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new("medium", name), &html, |b, html| {
            b.iter(|| parse(black_box(html), Some(url), None))
        });
    }

    // Benchmark large articles
    for (name, url) in large_articles {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new("large", name), &html, |b, html| {
            b.iter(|| parse(black_box(html), Some(url), None))
        });
    }

    group.finish();
}

/// Benchmark the full four-pass fallback path.
fn bench_parse_retries(c: &mut Criterion) {
    let html = load_test_page("medium-2");
    let options = Options::new().char_threshold(usize::MAX);
    c.bench_function("parse_retries/medium-2", |b| {
        b.iter(|| {
            parse(
                black_box(&html),
                Some("https://medium.com"),
                Some(options.clone()),
            )
        })
    });
}

/// Build readerable inputs that force the heuristic to inspect the whole tree.
fn adversarial_readerable_cases() -> Vec<(&'static str, String)> {
    let mut nested_candidates = String::from("<body>");
    for _ in 0..1_024 {
        nested_candidates.push_str("<article>");
    }
    nested_candidates.push_str("<p>article text</p>");
    nested_candidates.push_str(&"</article>".repeat(1_024));
    nested_candidates.push_str("</body>");

    let mut nested_list = String::from("<body><li>");
    for _ in 0..1_024 {
        nested_list.push_str("<div>");
    }
    for _ in 0..128 {
        nested_list.push_str("<p>x</p>");
    }
    nested_list.push_str(&"</div>".repeat(1_024));
    nested_list.push_str("</li></body>");

    let mut br_parents = String::from("<body>");
    for _ in 0..4_096 {
        br_parents.push_str("<div><br><span>text</span></div>");
    }
    br_parents.push_str("</body>");

    vec![
        ("nested-candidates", nested_candidates),
        ("nested-list-items", nested_list),
        ("many-br-parents", br_parents),
    ]
}

/// Benchmark is_probably_readerable check
fn bench_readerable(c: &mut Criterion) {
    let mut group = c.benchmark_group("is_probably_readerable");

    let test_cases = [
        ("basic-tags-cleaning", "small"),
        ("medium-2", "medium"),
        ("wikipedia-2", "large"),
    ];

    for (name, size) in test_cases {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new(size, name), &html, |b, html| {
            b.iter(|| is_probably_readerable(black_box(html), None))
        });
    }

    // Use an unreachable score so these cases never return from the candidate loop.
    // They exercise overlapping subtree lengths, deep paragraph exclusions, and the
    // old linear search for previously seen BR parents.
    let options = ReaderableOptions::new()
        .min_content_length(0)
        .min_score(f64::MAX);
    for (name, html) in adversarial_readerable_cases() {
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new("adversarial", name), &html, |b, html| {
            b.iter(|| is_probably_readerable(black_box(html), Some(options.clone())))
        });
    }

    group.finish();
}

/// Benchmark complex real-world pages with lots of markup
fn bench_complex_pages(c: &mut Criterion) {
    let mut group = c.benchmark_group("complex_pages");

    // These pages have complex markup structures
    let complex_pages = [
        ("buzzfeed-1", "https://buzzfeed.com"), // ~378KB, lots of social widgets
        ("engadget", "https://engadget.com"),   // ~350KB, tech blog with embeds
        ("guardian-1", "https://theguardian.com"), // ~1.16MB, news with heavy layout
    ];

    for (name, url) in complex_pages {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new("page", name), &html, |b, html| {
            b.iter(|| parse(black_box(html), Some(url), None))
        });
    }

    group.finish();
}

/// Benchmark parser scaling on adversarial deeply nested markup.
fn bench_deeply_nested(c: &mut Criterion) {
    let mut group = c.benchmark_group("deeply_nested_document");

    for depth in [1_000, 2_000, 4_000, 8_000] {
        let mut html = String::with_capacity(depth * 11);
        html.push_str("<!doctype html>");
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push('x');
        for _ in 0..depth {
            html.push_str("</div>");
        }

        group.throughput(Throughput::Elements(depth as u64));
        group.bench_with_input(BenchmarkId::from_parameter(depth), &html, |b, html| {
            b.iter(|| Document::new(black_box(html)))
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_dom_parse,
    bench_parse,
    bench_parse_retries,
    bench_readerable,
    bench_complex_pages,
    bench_deeply_nested
);
criterion_main!(benches);
