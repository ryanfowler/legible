use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use legible::extract;
use std::hint::black_box;

fn fixtures() -> [(&'static str, &'static str, &'static str); 9] {
    [
        (
            "medium-2",
            include_str!("fixtures/readability-js/medium-2/source.html"),
            "https://medium.com/@ckirchoff/on-behalf-of-literally-429fab868ca8",
        ),
        (
            "ars-1",
            include_str!("fixtures/readability-js/ars-1/source.html"),
            "https://arstechnica.com/information-technology/2015/04/just-released-minecraft-exploit-makes-it-easy-to-crash-game-servers/",
        ),
        (
            "heise",
            include_str!("fixtures/readability-js/heise/source.html"),
            "http://www.heise.de/mac-and-i/meldung/1Password-fuer-Mac-generiert-Einmal-Passwoerter-2596987.html",
        ),
        (
            "nytimes-5",
            include_str!("fixtures/readability-js/nytimes-5/source.html"),
            "https://www.nytimes.com/es/",
        ),
        (
            "wikipedia-2",
            include_str!("fixtures/readability-js/wikipedia-2/source.html"),
            "https://en.wikipedia.org/wiki/New_Zealand",
        ),
        (
            "yahoo-2",
            include_str!("fixtures/readability-js/yahoo-2/source.html"),
            "https://us.yahoo.com/news/",
        ),
        (
            "buzzfeed-1",
            include_str!("fixtures/readability-js/buzzfeed-1/source.html"),
            "http://www.buzzfeed.com/markdistefano/diet-pills-burns-up",
        ),
        (
            "engadget",
            include_str!("fixtures/readability-js/engadget/source.html"),
            "https://www.engadget.com/2017/11/03/xbox-one-x-review/",
        ),
        (
            "guardian-1",
            include_str!("fixtures/readability-js/guardian-1/source.html"),
            "https://www.theguardian.com/environment/2019/jan/03/what-is-the-sea-telling-us-maori-tribes-fearful-over-whale-strandings",
        ),
    ]
}

fn bench_mozilla_readability_fixtures(c: &mut Criterion) {
    let mut group = c.benchmark_group("mozilla_readability");
    group.sample_size(10);

    for (name, html, url) in fixtures() {
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), html, |b, html| {
            b.iter(|| extract(black_box(html), Some(url)).unwrap())
        });
    }

    group.finish();
}

fn bench_mozilla_readability_markdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("mozilla_readability_markdown");
    group.sample_size(10);

    for (name, html, url) in fixtures() {
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), html, |b, html| {
            b.iter(|| {
                let page = extract(black_box(html), Some(url)).unwrap();
                black_box(page.markdown())
            })
        });
    }

    group.finish();
}

fn obvious_article(direction: Option<&str>) -> String {
    let direction = direction.map_or(String::new(), |value| format!(" dir='{value}'"));
    let section = "<section><h2>Operational guidance</h2><p>This technical article explains configuration, validation, compatibility, deployment, diagnostics, maintenance, recovery, and predictable production behavior.</p><p>It gives readers practical details, clear operating guidance, verification steps, and safe failure handling.</p></section>";
    let chrome_link = "<li class='navigation-item' data-rank='1' data-area='products' data-track='navigation' data-layout='wide'><a href='/product' title='Product documentation' aria-label='Product documentation' data-action='open' data-source='header'>Product documentation</a></li>";
    format!(
        "<html><head><title>Reliable system operations</title></head><body><header><nav><ul>{}</ul></nav></header><main><article{direction}><h1>Reliable system operations</h1>{}</article></main><aside>Related documentation</aside><footer><nav><ul>{}</ul></nav></footer></body></html>",
        chrome_link.repeat(100),
        section.repeat(100),
        chrome_link.repeat(100),
    )
}

fn bench_high_confidence_root(c: &mut Criterion) {
    let fast = obvious_article(None);
    // Directional source policy requires generic scoring. Keep this input
    // otherwise equal so the benchmark shows the cost avoided on a probe hit.
    let scored = obvious_article(Some("ltr"));
    let mut group = c.benchmark_group("high_confidence_root");
    group.sample_size(10);

    for (name, html) in [("fast", &fast), ("generic_scoring", &scored)] {
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), html, |b, html| {
            b.iter(|| extract(black_box(html), None).unwrap())
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_mozilla_readability_fixtures,
    bench_mozilla_readability_markdown,
    bench_high_confidence_root
);
criterion_main!(benches);
