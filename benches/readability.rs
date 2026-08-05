use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use legible::{Document, Extractor, extract, is_probably_readable};
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
            b.iter(|| Document::parse(black_box(html)))
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
    for (name, _url) in small_articles {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new("small", name), &html, |b, html| {
            b.iter(|| extract(black_box(html)))
        });
    }

    // Benchmark medium articles
    for (name, _url) in medium_articles {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new("medium", name), &html, |b, html| {
            b.iter(|| extract(black_box(html)))
        });
    }

    // Benchmark large articles
    for (name, _url) in large_articles {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new("large", name), &html, |b, html| {
            b.iter(|| extract(black_box(html)))
        });
    }

    group.finish();
}

/// Benchmark the full four-pass fallback path.
fn bench_parse_retries(c: &mut Criterion) {
    let html = load_test_page("medium-2");
    let extractor = Extractor::builder()
        .retry_length_threshold(usize::MAX)
        .build()
        .unwrap();
    c.bench_function("parse_retries/medium-2", |b| {
        b.iter(|| extractor.extract(black_box(&html)))
    });
}

/// Benchmark extraction separately from each on-demand renderer.
fn bench_output_formats(c: &mut Criterion) {
    let html = load_test_page("medium-2");
    let extractor = Extractor::default();
    c.bench_function("extract_only/medium-2", |b| {
        b.iter(|| extractor.extract(black_box(&html)))
    });
    for (name, render) in [
        (
            "extract_and_html",
            legible::Article::to_html as fn(&legible::Article) -> String,
        ),
        ("extract_and_markdown", legible::Article::to_markdown),
        ("extract_and_text", legible::Article::to_text),
    ] {
        c.bench_function(&format!("{name}/medium-2"), |b| {
            b.iter(|| render(&extractor.extract(black_box(&html)).unwrap()))
        });
    }
    c.bench_function("extract_and_all_formats/medium-2", |b| {
        b.iter(|| {
            let article = extractor.extract(black_box(&html)).unwrap();
            (article.to_html(), article.to_markdown(), article.to_text())
        })
    });
}

/// Benchmark repeated rendering without parse, extraction, or tree-freezing work.
fn bench_render_only(c: &mut Criterion) {
    let extractor = Extractor::default();
    let fixtures = [
        ("basic-tags-cleaning", load_test_page("basic-tags-cleaning")),
        ("medium-2", load_test_page("medium-2")),
        ("wikipedia-2", load_test_page("wikipedia-2")),
        ("guardian-1", load_test_page("guardian-1")),
        (
            "noisy-small-article",
            format!(
                "<body>{}<article><p>{}</p></article></body>",
                "<aside>navigation and advertising</aside>".repeat(2_000),
                "retained article text ".repeat(100)
            ),
        ),
        (
            "large-retained-article",
            format!(
                "<article>{}</article>",
                "<p>retained article text</p>".repeat(2_000)
            ),
        ),
    ];

    let mut render = c.benchmark_group("render_only");
    for (name, html) in fixtures {
        let article = extractor.extract(&html).unwrap();
        render.bench_function(BenchmarkId::new("markdown", name), |b| {
            b.iter(|| article.to_markdown())
        });
        render.bench_function(BenchmarkId::new("text", name), |b| {
            b.iter(|| article.to_text())
        });
        render.bench_function(BenchmarkId::new("all_formats", name), |b| {
            b.iter(|| (article.to_html(), article.to_markdown(), article.to_text()))
        });
    }
    render.finish();
}

/// Benchmark is_probably_readable check
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
            b.iter(|| is_probably_readable(black_box(html), None))
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

    for (name, _url) in complex_pages {
        let html = load_test_page(name);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new("page", name), &html, |b, html| {
            b.iter(|| extract(black_box(html)))
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
            b.iter(|| Document::parse(black_box(html)))
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_dom_parse,
    bench_parse,
    bench_parse_retries,
    bench_output_formats,
    bench_render_only,
    bench_readerable,
    bench_complex_pages,
    bench_deeply_nested
);
criterion_main!(benches);
