use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use legible::{Extractor, extract};
use std::fs;
use std::hint::black_box;

const TEST_PAGES_DIR: &str = "tests/readability-js/test/test-pages";

fn load_test_page(name: &str) -> String {
    let path = format!("{TEST_PAGES_DIR}/{name}/source.html");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("Failed to read {path}: {error}"))
}

fn bench_extract(c: &mut Criterion) {
    let mut group = c.benchmark_group("extract");
    for (size, name, url) in [
        ("small", "basic-tags-cleaning", "https://example.com"),
        ("small", "replace-brs", "https://example.com"),
        ("medium", "medium-2", "https://medium.com"),
        ("medium", "ars-1", "https://arstechnica.com"),
        ("medium", "heise", "https://heise.de"),
        ("large", "nytimes-5", "https://nytimes.com"),
        ("large", "wikipedia-2", "https://wikipedia.org"),
        ("large", "yahoo-2", "https://yahoo.com"),
    ] {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new(size, name), &html, |b, html| {
            b.iter(|| extract(black_box(html), Some(url)))
        });
    }
    group.finish();
}

fn bench_lazy_outputs(c: &mut Criterion) {
    let html = load_test_page("medium-2");
    let page = extract(&html, Some("https://medium.com")).unwrap();
    let mut group = c.benchmark_group("lazy_output/medium-2");
    group.bench_function("markdown", |b| b.iter(|| page.markdown()));
    group.bench_function("text", |b| b.iter(|| page.text()));
    group.bench_function("html", |b| b.iter(|| page.html()));
    group.finish();
}

fn bench_complex_pages(c: &mut Criterion) {
    let mut group = c.benchmark_group("complex_pages");
    for (name, url) in [
        ("buzzfeed-1", "https://buzzfeed.com"),
        ("engadget", "https://engadget.com"),
        ("guardian-1", "https://theguardian.com"),
    ] {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &html, |b, html| {
            b.iter(|| extract(black_box(html), Some(url)))
        });
    }
    group.finish();
}

fn bench_deeply_nested(c: &mut Criterion) {
    let mut group = c.benchmark_group("deeply_nested_document");
    let extractor = Extractor::default();
    for depth in [1_000, 2_000, 4_000, 8_000] {
        let mut html = String::with_capacity(depth * 11);
        html.push_str("<!doctype html><body>");
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push('x');
        for _ in 0..depth {
            html.push_str("</div>");
        }
        group.throughput(Throughput::Elements(depth as u64));
        group.bench_with_input(BenchmarkId::from_parameter(depth), &html, |b, html| {
            b.iter(|| extractor.extract(black_box(html), None))
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_extract,
    bench_lazy_outputs,
    bench_complex_pages,
    bench_deeply_nested
);
criterion_main!(benches);
