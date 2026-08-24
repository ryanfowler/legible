use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use legible::extract;
use std::hint::black_box;

fn fixtures() -> [(&'static str, &'static str, &'static str); 10] {
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
        (
            "go-net-http",
            include_str!("fixtures/go-net-http/source.html"),
            "https://pkg.go.dev/net/http",
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

fn bench_mozilla_readability_html(c: &mut Criterion) {
    let mut group = c.benchmark_group("mozilla_readability_html");
    group.sample_size(10);

    for (name, html, url) in fixtures() {
        let page = extract(html, Some(url)).unwrap();
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| black_box(page.html()))
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

criterion_group!(
    benches,
    bench_mozilla_readability_fixtures,
    bench_mozilla_readability_html,
    bench_mozilla_readability_markdown
);
criterion_main!(benches);
