use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use legible::{Extractor, extract};
use std::hint::black_box;

// Criterion benchmarks compile as a separate crate. Include the private DOM module so
// the parser benchmark measures the same parser that extraction uses.
#[allow(unused_imports)]
#[path = "../src/document/mod.rs"]
mod document;
#[allow(dead_code, unused_imports)]
#[path = "../src/dom/mod.rs"]
mod dom;

fn benchmark_page(kind: &str, target_bytes: usize) -> String {
    let mut html = String::with_capacity(target_bytes + 256);
    html.push_str("<!doctype html><html><head><title>Benchmark page</title>");
    if kind == "metadata" {
        for index in 0..200 {
            html.push_str(&format!(
                "<meta property='article:tag' content='benchmark-{index}'><meta name='citation_author' content='Author {index}'>"
            ));
        }
    } else if kind == "json-ld" {
        html.push_str("<script type='application/ld+json'>[");
        for index in 0..200 {
            if index > 0 {
                html.push(',');
            }
            html.push_str(&format!(
                r#"{{"@type":"Article","headline":"Entry {index}","articleBody":"Representative structured article text {index}."}}"#
            ));
        }
        html.push_str("]</script>");
    }
    html.push_str("</head><body><nav><a href='/'>Home</a></nav><main><h1>Benchmark page</h1>");
    let mut index = 0;
    while html.len() < target_bytes {
        match kind {
            "reference" => html.push_str(&format!(
                "<section><h2>Method {index}</h2><pre><code>let value_{index} = parse(input);</code></pre><table><tr><th>Field</th><th>Value</th></tr><tr><td>index</td><td>{index}</td></tr></table></section>"
            )),
            "code" => html.push_str(&format!(
                "<section><h2>Example {index}</h2><div class='highlight language-rust'><div class='toolbar'><button>Copy</button></div><pre><code><span class='line'><span class='line-number'>{index}</span><span>fn example_{index}() {{</span></span><br><span class='line'>    println!(\"value {index}\");</span><br><span class='line'>}}</span></code></pre></div></section>"
            )),
            "math" => html.push_str(&format!(
                "<section><h2>Equation {index}</h2><p>The result follows from <math><mfrac><mi>x</mi><mn>{index}</mn></mfrac></math>.</p><div class='katex'><math aria-hidden='true'><msup><mi>x</mi><mn>2</mn></msup></math><span class='katex-html'>x²</span></div></section>"
            )),
            "tables" => html.push_str(&format!(
                "<section><h2>Dataset {index}</h2><table><thead><tr><th>Name</th><th>Value</th><th>Status</th></tr></thead><tbody><tr><td>entry-{index}</td><td>{index}</td><td>ready</td></tr><tr><td>alternate-{index}</td><td>{}</td><td>complete</td></tr></tbody></table><table role='presentation'><tr><td><p>Layout prose {index} contains a complete explanation that must remain readable after normalization.</p></td></tr></table></section>", index + 1
            )),
            "listing" => html.push_str(&format!(
                "<article><h2><a href='/entry/{index}'>Entry {index}</a></h2><p>This entry contains useful summary text and stable benchmark content.</p></article>"
            )),
            "malformed" => html.push_str(&format!(
                "<section><h2>Broken {index}<p>Malformed markup still contains representative prose and useful extraction content.<table><tr><td>{index}<td>value</section>"
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

fn normalized_fragment(kind: &str, target_bytes: usize) -> String {
    let mut html = String::with_capacity(target_bytes + 256);
    let mut index = 0;
    while html.len() < target_bytes {
        match kind {
            "reference" => html.push_str(&format!(
                "<section><h2>Method {index}</h2><p>This method parses a representative input value.</p><pre><code class='language-rust' data-language='rust'>let value_{index} = parse(input);</code></pre><table><tr><th>Field</th><th>Value</th></tr><tr><td>index</td><td>{index}</td></tr></table></section>"
            )),
            "code" => html.push_str(&format!(
                "<section><h2>Example {index}</h2><div class='highlight language-rust'><pre><code><span data-line><span class='line-number'>{index}</span><span>fn example_{index}() {{</span></span><span data-line>    println!(\"value {index}\");</span><span data-line>}}</span></code></pre></div></section>"
            )),
            "tables" => html.push_str(&format!(
                "<section><h2>Dataset {index}</h2><table><thead><tr><th>Name</th><th>Value</th><th>Status</th></tr></thead><tbody><tr><td>entry-{index}</td><td>{index}</td><td>ready</td></tr><tr><td>alternate-{index}</td><td>{}</td><td>complete</td></tr></tbody></table></section>", index + 1
            )),
            "listing" => html.push_str(&format!(
                "<article><h2><a href='/entry/{index}'>Entry {index}</a></h2><p>This entry contains useful summary text and stable benchmark content.</p></article>"
            )),
            _ => html.push_str(&format!(
                "<section><h2>Section {index}</h2><p>This paragraph contains representative normalized prose and punctuation.</p><p>A second paragraph measures semantic compilation.</p></section>"
            )),
        }
        index += 1;
    }
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
        let measured = Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(&html, Some(url))
            .unwrap();
        let representation = &measured
            .diagnostics()
            .unwrap()
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap()
            .representation;
        eprintln!(
            "extraction-representation/{size}-{name}: source_dom_nodes={}, final_dom_nodes={}, ir_nodes={}, retained_bytes={}, source_bytes={}",
            representation.source_dom_nodes,
            representation.final_dom_nodes,
            representation.document_nodes,
            representation.estimated_document_bytes,
            html.len()
        );
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new(size, name), &html, |b, html| {
            b.iter(|| extract(black_box(html), Some(url)).unwrap())
        });
    }
    group.finish();
}

fn bench_document_compile(c: &mut Criterion) {
    let mut group = c.benchmark_group("document_compile");
    for (name, kind, bytes) in [
        ("simple-prose", "prose", 4_000),
        ("long-prose", "prose", 250_000),
        ("highlighted-code", "code", 100_000),
        ("table-heavy", "tables", 100_000),
        ("documentation", "reference", 100_000),
        ("listing", "listing", 100_000),
    ] {
        let html = normalized_fragment(kind, bytes);
        let dom = dom::Dom::parse_fragment(&html, dom::Tag::Div).unwrap();
        let root = dom.root();
        let base = url::Url::parse("https://example.com/docs/page").unwrap();
        let context = document::CompileContext::new(Some(base.clone()), Some(&base));
        let document = document::compile_document(&dom, root, &context).unwrap();
        let semantic_nodes = document.len();
        let retained_bytes = document.retained_bytes_estimate();
        let source_sized_bytes = retained_bytes.saturating_add(
            dom.len()
                .saturating_sub(document.node_capacity())
                .saturating_mul(document::Document::node_slot_size()),
        );
        eprintln!(
            "representation/{name}: dom_nodes={}, ir_nodes={semantic_nodes}, ir_capacity={}, retained_bytes={retained_bytes}, source_sized_bytes={source_sized_bytes}",
            dom.len(),
            document.node_capacity()
        );
        group.throughput(Throughput::Elements(dom.len() as u64));
        group.bench_with_input(
            BenchmarkId::new(name, format!("dom-{}-ir-{semantic_nodes}", dom.len())),
            &dom,
            |b, dom| {
                b.iter(|| {
                    document::compile_document(black_box(dom), root, black_box(&context)).unwrap()
                })
            },
        );
    }
    group.finish();
}

fn bench_extract_markdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("extract_markdown");
    for (size, name, kind, bytes) in [
        ("small", "prose", "prose", 4_000),
        ("medium", "prose", "prose", 50_000),
        ("medium", "reference", "reference", 50_000),
        ("large", "prose", "prose", 500_000),
        ("large", "malformed", "malformed", 250_000),
    ] {
        let html = benchmark_page(kind, bytes);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new(size, name), &html, |b, html| {
            b.iter(|| {
                let page = extract(black_box(html), Some("https://example.com")).unwrap();
                page.markdown()
            })
        });
    }
    group.finish();
}

fn bench_lazy_outputs(c: &mut Criterion) {
    let mut group = c.benchmark_group("lazy_output");
    for (kind, bytes) in [("short", 4_000), ("long", 250_000), ("reference", 50_000)] {
        let source_kind = if kind == "reference" {
            "reference"
        } else {
            "prose"
        };
        let html = benchmark_page(source_kind, bytes);
        let page = extract(&html, Some("https://example.com")).unwrap();
        group.bench_function(BenchmarkId::new(kind, "markdown"), |b| {
            b.iter(|| page.markdown())
        });
        group.bench_function(BenchmarkId::new(kind, "text"), |b| b.iter(|| page.text()));
        group.bench_function(BenchmarkId::new(kind, "html"), |b| b.iter(|| page.html()));
    }
    group.finish();
}

fn bench_complex_pages(c: &mut Criterion) {
    let mut group = c.benchmark_group("complex_pages");
    group.sample_size(20);
    for (name, kind, url) in [
        ("prose", "prose", "https://example.com"),
        ("documentation", "reference", "https://example.com"),
        ("highlighted-code", "code", "https://example.com"),
        ("math", "math", "https://example.com"),
        ("table-heavy", "tables", "https://example.com"),
        ("listing", "listing", "https://example.com"),
        ("malformed", "malformed", "https://example.com"),
        ("metadata-heavy", "metadata", "https://example.com"),
        ("json-ld-heavy", "json-ld", "https://example.com"),
    ] {
        let html = benchmark_page(kind, 250_000);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &html, |b, html| {
            b.iter(|| extract(black_box(html), Some(url)).unwrap())
        });
    }
    group.finish();
}

fn bench_large_compatibility_fixtures(c: &mut Criterion) {
    let mut group = c.benchmark_group("large_compatibility_fixtures");
    group.sample_size(10);
    for (name, html, url) in [
        (
            "guardian-article",
            include_str!("fixtures/guardian-article/source.html"),
            "https://www.theguardian.com/example",
        ),
        (
            "wikipedia-reference",
            include_str!("fixtures/wikipedia-reference/source.html"),
            "https://en.wikipedia.org/wiki/Example",
        ),
    ] {
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), html, |b, html| {
            b.iter(|| extract(black_box(html), Some(url)).unwrap())
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
    bench_document_compile,
    bench_extract_markdown,
    bench_lazy_outputs,
    bench_complex_pages,
    bench_large_compatibility_fixtures,
    bench_deeply_nested
);
criterion_main!(benches);
