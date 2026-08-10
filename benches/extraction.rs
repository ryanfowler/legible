use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use legible::extract;
use std::hint::black_box;

// Criterion benchmarks compile as a separate crate. Include the private DOM module so
// the parser benchmark measures the same parser that extraction uses.
#[allow(dead_code, unused_imports)]
#[path = "../src/dom/mod.rs"]
mod dom;

fn benchmark_page(kind: &str, target_bytes: usize) -> String {
    let mut html = String::with_capacity(target_bytes + 256);
    html.push_str("<!doctype html><html><head><title>Benchmark page</title></head><body><nav><a href='/'>Home</a></nav><main><h1>Benchmark page</h1>");
    let mut index = 0;
    while html.len() < target_bytes {
        match kind {
            "reference" => html.push_str(&format!(
                "<section><h2>Method {index}</h2><pre><code>let value_{index} = parse(input);</code></pre><table><tr><th>Field</th><th>Value</th></tr><tr><td>index</td><td>{index}</td></tr></table></section>"
            )),
            "listing" => html.push_str(&format!(
                "<article><h2><a href='/entry/{index}'>Entry {index}</a></h2><p>This entry contains useful summary text and stable benchmark content.</p></article>"
            )),
            _ => html.push_str(&format!(
                "<section><h2>Section {index}</h2><p>This paragraph contains representative prose, punctuation, and enough detail for content candidate scoring.</p><p>A second paragraph measures extraction cleanup and source-relative quality.</p></section>"
            )),
        }
        index += 1;
    }
    html.push_str("</main><footer>Footer links</footer></body></html>");
    html
}

fn bench_extract(c: &mut Criterion) {
    let mut group = c.benchmark_group("extract");
    for (size, name, kind, bytes, url) in [
        ("small", "prose", "prose", 4_000, "https://example.com"),
        (
            "small",
            "reference",
            "reference",
            4_000,
            "https://example.com",
        ),
        ("medium", "prose", "prose", 50_000, "https://example.com"),
        (
            "medium",
            "reference",
            "reference",
            50_000,
            "https://example.com",
        ),
        (
            "medium",
            "listing",
            "listing",
            50_000,
            "https://example.com",
        ),
        ("large", "prose", "prose", 500_000, "https://example.com"),
        (
            "large",
            "reference",
            "reference",
            500_000,
            "https://example.com",
        ),
        (
            "large",
            "listing",
            "listing",
            500_000,
            "https://example.com",
        ),
    ] {
        let html = benchmark_page(kind, bytes);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new(size, name), &html, |b, html| {
            b.iter(|| extract(black_box(html), Some(url)))
        });
    }
    group.finish();
}

fn bench_lazy_outputs(c: &mut Criterion) {
    let html = benchmark_page("prose", 50_000);
    let page = extract(&html, Some("https://example.com")).unwrap();
    let mut group = c.benchmark_group("lazy_output/medium");
    group.bench_function("markdown", |b| b.iter(|| page.markdown()));
    group.bench_function("text", |b| b.iter(|| page.text()));
    group.bench_function("html", |b| b.iter(|| page.html()));
    group.finish();
}

fn bench_complex_pages(c: &mut Criterion) {
    let mut group = c.benchmark_group("complex_pages");
    for (name, kind, url) in [
        ("prose", "prose", "https://example.com"),
        ("reference", "reference", "https://example.com"),
        ("listing", "listing", "https://example.com"),
    ] {
        let html = benchmark_page(kind, 250_000);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &html, |b, html| {
            b.iter(|| extract(black_box(html), Some(url)))
        });
    }
    group.finish();
}

/// Benchmarks parser scaling on adversarial deeply nested markup.
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
            b.iter(|| dom::Dom::parse_document(black_box(html)))
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
