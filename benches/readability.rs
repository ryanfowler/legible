use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use legible::{is_probably_readerable, parse};
use std::fs;

const TEST_PAGES_DIR: &str = "tests/readability-js/test/test-pages";

/// Load test page HTML from the Mozilla test suite
fn load_test_page(name: &str) -> String {
    let path = format!("{}/{}/source.html", TEST_PAGES_DIR, name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e))
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

criterion_group!(benches, bench_parse, bench_readerable, bench_complex_pages);
criterion_main!(benches);
