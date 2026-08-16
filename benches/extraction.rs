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
            "footnotes" => html.push_str(&format!(
                "<section><h2>Reference {index}</h2><p>This explanation cites a retained definition<sup><a href='#note-{index}' role='doc-noteref'>{index}</a></sup>.</p><aside id='note-{index}' role='doc-footnote'>The reference definition for item {index} contains useful context.</aside></section>"
            )),
            "math" => html.push_str(&format!(
                "<section><h2>Equation {index}</h2><p>The result follows from <math><mfrac><mi>x</mi><mn>{index}</mn></mfrac></math>.</p><div class='katex'><math aria-hidden='true'><msup><mi>x</mi><mn>2</mn></msup></math><span class='katex-html'>x²</span></div></section>"
            )),
            "code" => html.push_str(&format!(
                "<section><h2>Example {index}</h2><div class='highlight language-rust'><div class='toolbar'><button>Copy</button></div><pre><code><span class='line'><span class='line-number'>{index}</span><span>fn example_{index}() {{</span></span><br><span class='line'>    println!(\"value {index}\");</span><br><span class='line'>}}</span></code></pre></div></section>"
            )),
            "tables" => html.push_str(&format!(
                "<section><h2>Dataset {index}</h2><table><thead><tr><th>Name</th><th>Value</th><th>Status</th></tr></thead><tbody><tr><td>entry-{index}</td><td>{index}</td><td>ready</td></tr><tr><td>alternate-{index}</td><td>{}</td><td>complete</td></tr></tbody></table><table role='presentation'><tr><td><p>Layout prose {index} contains a complete explanation that must remain readable after normalization.</p></td></tr></table></section>", index + 1
            )),
            "listing" => html.push_str(&format!(
                "<article><h2><a href='/entry/{index}'>Entry {index}</a></h2><p>This entry contains useful summary text and stable benchmark content.</p></article>"
            )),
            "ordinary-inline" => html.push_str(&ordinary_inline_section(index)),
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

fn ordinary_inline_section(index: usize) -> String {
    format!(
        "<section><h2>Section {index}</h2><p>Normal article text with <strong>strong emphasis containing <em>nested emphasis</em></strong>, <a href='/relative'>relative links</a>, and <code>inline code</code>.</p><blockquote><p>A quoted paragraph keeps ordinary block structure realistic.</p></blockquote><ul><li>First unordered item</li><li>Second unordered item</li></ul><ol><li>First ordered item</li><li>Second ordered item</li></ol><figure><img src='/image.jpg' alt='Useful image'><figcaption>A useful image caption.</figcaption></figure><details><summary>Additional context</summary><p>The expandable explanation remains ordinary semantic content.</p></details><dl><dt>Term</dt><dd>The definition list gives the common workload a native definition structure.</dd></dl></section>"
    )
}

fn ordinary_inline_fragment(target_bytes: usize) -> String {
    let mut html = String::with_capacity(target_bytes + 256);
    html.push_str("<article><h1>Representative inline article</h1>");
    let mut index = 0;
    while html.len() < target_bytes {
        html.push_str(&ordinary_inline_section(index));
        index += 1;
    }
    html.push_str("</article>");
    html
}

/// Builds a source fragment with the same semantic shapes as a cleaned retained
/// region. The fragment excludes page chrome so lowering is measured separately
/// from extraction and output rendering.
fn retained_fragment(kind: &str, target_bytes: usize) -> String {
    if kind == "ordinary-inline" {
        return ordinary_inline_fragment(target_bytes);
    }

    let mut html = String::with_capacity(target_bytes + 256);
    let mut index = 0;
    while html.len() < target_bytes {
        match kind {
            "reference" => html.push_str(&format!(
                "<section><h2>Method {index}</h2><p>This method parses a representative input value.</p><pre><code class='language-rust' data-language='rust'>let value_{index} = parse(input);</code></pre><table><tr><th>Field</th><th>Value</th></tr><tr><td>index</td><td>{index}</td></tr></table></section>"
            )),
            "footnotes" => html.push_str(&format!(
                "<section><h2>Reference {index}</h2><p>This explanation cites a retained definition<sup><a href='#note-{index}' role='doc-noteref'>{index}</a></sup>.</p><aside id='note-{index}' role='doc-footnote'>The reference definition for item {index} contains useful context.</aside></section>"
            )),
            "math" => html.push_str(&format!(
                "<section><h2>Equation {index}</h2><p>The result follows from <math><mfrac><mi>x</mi><mn>{index}</mn></mfrac></math>.</p><div class='katex'><math aria-hidden='true'><msup><mi>x</mi><mn>2</mn></msup></math><span class='katex-html'>x²</span></div></section>"
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
        (
            "medium",
            "ordinary-inline",
            "ordinary-inline",
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
        (
            "large",
            "ordinary-inline",
            "ordinary-inline",
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

fn bench_lower_retained_fragment(c: &mut Criterion) {
    let mut group = c.benchmark_group("lower_retained_fragment");
    let mut total_semantic_nodes = 0usize;
    let mut fixture_count = 0usize;

    eprintln!(
        "representation/layout: arena_node_bytes={}, node_kind_bytes={}, text_value_bytes={}",
        std::mem::size_of::<document::ArenaNode>(),
        std::mem::size_of::<document::NodeKind>(),
        std::mem::size_of::<document::TextValue>(),
    );

    for (name, kind, bytes) in [
        ("simple-prose", "prose", 4_000),
        ("long-prose", "prose", 250_000),
        ("ordinary-inline", "ordinary-inline", 50_000),
        ("ordinary-inline-large", "ordinary-inline", 500_000),
        ("highlighted-code", "code", 100_000),
        ("math", "math", 100_000),
        ("table-heavy", "tables", 100_000),
        ("documentation", "reference", 100_000),
        ("footnotes", "footnotes", 100_000),
        ("listing", "listing", 100_000),
    ] {
        let html = retained_fragment(kind, bytes);
        let dom = dom::Dom::parse_fragment(&html, dom::Tag::Div).unwrap();
        let root = dom.root();
        let base = url::Url::parse("https://example.com/docs/page").unwrap();
        let context = document::CompileContext::new(Some(base.clone()), Some(&base));
        // Production extraction reuses source evidence and cleanup facts from
        // the retained fragment. Build both outside the timed lowering loop.
        let source_evidence =
            document::SourceEvidence::analyze(&dom, root, &dom::NodeStateStore::new());
        let source_facts = document::SemanticSourceFacts::analyze(&dom, root);
        let document = document::compile_document_with_optional_source_facts_and_evidence(
            &dom,
            root,
            &context,
            Some(&source_facts),
            Some(&source_evidence),
        )
        .unwrap();
        let semantic_nodes = document.len();
        let retained_bytes = document.retained_bytes_estimate();
        let source_sized_bytes = retained_bytes.saturating_add(
            dom.len()
                .saturating_sub(document.node_capacity())
                .saturating_mul(document::Document::node_slot_size()),
        );
        total_semantic_nodes = total_semantic_nodes.saturating_add(semantic_nodes);
        fixture_count += 1;
        eprintln!(
            "representation/{name}: dom_nodes={}, ir_nodes={semantic_nodes}, roots={}, ir_capacity={}, retained_bytes={retained_bytes}, semantic_string_bytes={}, semantic_string_values={}, source_sized_bytes={source_sized_bytes}",
            dom.len(),
            document.root_count(),
            document.node_capacity(),
            document.semantic_string_bytes(),
            document.semantic_string_value_count(),
        );
        group.throughput(Throughput::Elements(dom.len() as u64));
        group.bench_with_input(
            BenchmarkId::new(name, format!("dom-{}-ir-{semantic_nodes}", dom.len())),
            &dom,
            |b, dom| {
                b.iter(|| {
                    document::compile_document_with_optional_source_facts_and_evidence(
                        black_box(dom),
                        root,
                        black_box(&context),
                        Some(&source_facts),
                        Some(&source_evidence),
                    )
                    .unwrap()
                })
            },
        );
    }
    group.finish();
    eprintln!(
        "representation/summary: fixtures={fixture_count}, average_semantic_nodes={:.1}",
        total_semantic_nodes as f64 / fixture_count as f64
    );
}

fn bench_extract_markdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("extract_markdown");
    for (size, name, kind, bytes) in [
        ("small", "prose", "prose", 4_000),
        ("medium", "prose", "prose", 50_000),
        ("medium", "reference", "reference", 50_000),
        ("medium", "ordinary-inline", "ordinary-inline", 50_000),
        ("large", "prose", "prose", 500_000),
        ("large", "ordinary-inline", "ordinary-inline", 500_000),
        ("large", "malformed", "malformed", 250_000),
    ] {
        let html = benchmark_page(kind, bytes);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::new(size, name), &html, |b, html| {
            b.iter(|| {
                let page = extract(black_box(html), Some("https://example.com")).unwrap();
                black_box(page.markdown())
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
        // Measure steady-state lazy rendering. Text statistics are initialized
        // once before timing so their one-time cost is not mixed into a render.
        let _ = page.text();
        group.bench_function(BenchmarkId::new(kind, "markdown"), |b| {
            b.iter(|| black_box(page.markdown()))
        });
        group.bench_function(BenchmarkId::new(kind, "text"), |b| {
            b.iter(|| black_box(page.text()))
        });
        group.bench_function(BenchmarkId::new(kind, "html"), |b| {
            b.iter(|| black_box(page.html()))
        });
    }
    for (size, bytes) in [("medium", 50_000), ("large", 500_000)] {
        let html = benchmark_page("ordinary-inline", bytes);
        let page = extract(&html, Some("https://example.com")).unwrap();
        let _ = page.text();
        group.bench_function(BenchmarkId::new(size, "ordinary-inline/markdown"), |b| {
            b.iter(|| black_box(page.markdown()))
        });
        group.bench_function(BenchmarkId::new(size, "ordinary-inline/text"), |b| {
            b.iter(|| black_box(page.text()))
        });
        group.bench_function(BenchmarkId::new(size, "ordinary-inline/html"), |b| {
            b.iter(|| black_box(page.html()))
        });
    }
    group.finish();
}

fn bench_complex_pages(c: &mut Criterion) {
    let mut group = c.benchmark_group("complex_pages");
    group.sample_size(20);
    for (name, kind, url) in [
        ("prose", "prose", "https://example.com"),
        ("documentation", "reference", "https://example.com"),
        ("footnotes-reference", "footnotes", "https://example.com"),
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
    bench_lower_retained_fragment,
    bench_extract_markdown,
    bench_lazy_outputs,
    bench_complex_pages,
    bench_large_compatibility_fixtures,
    bench_deeply_nested
);
criterion_main!(benches);
