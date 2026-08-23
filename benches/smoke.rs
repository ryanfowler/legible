use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use legible::extract;
use std::hint::black_box;
use std::time::Duration;

fn article(target_bytes: usize) -> String {
    let mut html = String::with_capacity(target_bytes + 128);
    html.push_str(
        "<!doctype html><html><head><title>Benchmark article</title></head><body><nav>Home</nav><main><h1>Benchmark article</h1>",
    );
    let mut index = 0;
    while html.len() < target_bytes {
        html.push_str(&format!(
            "<section><h2>Section {index}</h2><p>Article text with <strong>emphasis</strong>, <em>details</em>, <a href='/reference'>a link</a>, and <code>inline code</code>.</p><blockquote><p>A quoted paragraph keeps ordinary block structure realistic.</p></blockquote><ul><li>First item</li><li>Second item</li></ul></section>"
        ));
        index += 1;
    }
    html.push_str("</main><footer>Footer links</footer></body></html>");
    html
}

fn discussion(reply_count: usize) -> String {
    let mut html = String::from(
        r#"<!doctype html><html><body><main><div class="h-entry"><span role="heading" aria-level="1"><a class="u-url" href="/topic">Benchmark discussion</a></span><a class="user_is_author">Author</a></div><div class="story_text"><p>The primary post explains the benchmark discussion workload.</p></div><ol class="comments">"#,
    );
    for index in 0..reply_count {
        html.push_str(&format!(
            r#"<li><div class="comment" data-shortid="reply-{index}"><div class="byline"><a>User {index}</a><time>Today</time></div><div class="comment_text"><p>Reply {index} contains representative discussion text for extraction.</p></div></div></li>"#,
        ));
    }
    html.push_str("</ol></main></body></html>");
    html
}

fn bench_smoke(c: &mut Criterion) {
    let mut group = c.benchmark_group("smoke");
    // These settings keep the development check short. Use the full extraction
    // suite for stable comparisons and small regressions.
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(100));
    group.measurement_time(Duration::from_millis(400));

    for (name, bytes) in [("medium-extract", 50_000), ("large-extract", 250_000)] {
        let html = article(bytes);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &html, |b, html| {
            b.iter(|| black_box(extract(black_box(html), Some("https://example.com")).unwrap()))
        });
    }

    let html = article(50_000);
    group.throughput(Throughput::Bytes(html.len() as u64));
    group.bench_with_input(
        BenchmarkId::from_parameter("medium-extract-markdown"),
        &html,
        |b, html| {
            b.iter(|| {
                let page = extract(black_box(html), Some("https://example.com")).unwrap();
                black_box(page.markdown())
            })
        },
    );

    let html = article(250_000);
    let page = extract(&html, Some("https://example.com")).unwrap();
    group.throughput(Throughput::Bytes(html.len() as u64));
    group.bench_function("large-render-markdown", |b| {
        b.iter(|| black_box(page.markdown()))
    });

    let html = discussion(250);
    group.throughput(Throughput::Bytes(html.len() as u64));
    group.bench_with_input(
        BenchmarkId::from_parameter("discussion-250-extract"),
        &html,
        |b, html| {
            b.iter(|| black_box(extract(black_box(html), Some("https://example.com")).unwrap()))
        },
    );

    group.finish();
}

criterion_group!(benches, bench_smoke);
criterion_main!(benches);
