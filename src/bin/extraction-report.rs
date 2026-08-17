//! Reports phase, clone, scan, and allocator measurements for extraction fixtures.

#[cfg(feature = "bench-instrumentation")]
use std::alloc::{GlobalAlloc, Layout, System};

#[cfg(feature = "bench-instrumentation")]
struct ReportingAllocator;

#[cfg(feature = "bench-instrumentation")]
unsafe impl GlobalAlloc for ReportingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            legible::instrumentation::record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        legible::instrumentation::record_deallocation(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(pointer, layout, new_size) };
        if !next.is_null() {
            legible::instrumentation::record_reallocation(layout.size(), new_size);
        }
        next
    }
}

#[cfg(feature = "bench-instrumentation")]
#[global_allocator]
static ALLOCATOR: ReportingAllocator = ReportingAllocator;

#[cfg(not(feature = "bench-instrumentation"))]
fn main() {
    eprintln!("extraction-report requires --features bench-instrumentation");
    std::process::exit(1);
}

#[cfg(feature = "bench-instrumentation")]
fn main() {
    println!("legible extraction instrumentation");
    println!("rust={}", rustc_version());
    println!("workload,bytes,winner,attempts");
    for fixture in strategy_fixtures() {
        report_fixture(fixture);
    }
    for (name, kind, bytes) in [
        ("small-prose", "prose", 4_000),
        ("medium-prose", "prose", 50_000),
        ("large-prose", "prose", 500_000),
        ("large-ordinary-inline", "ordinary-inline", 500_000),
        ("metadata-heavy", "metadata", 250_000),
        ("json-ld-heavy", "json-ld", 250_000),
    ] {
        let html = generated_page(kind, bytes);
        report(
            "extraction",
            name,
            &html,
            legible::Extractor::builder().diagnostics(true).build(),
            None,
        );
    }
    let html = generated_page("json-ld", 250_000);
    report(
        "extraction",
        "json-ld-retained",
        &html,
        legible::Extractor::builder()
            .diagnostics(true)
            .retain_structured_data(true)
            .build(),
        None,
    );
}

#[cfg(feature = "bench-instrumentation")]
struct Fixture {
    name: &'static str,
    html: &'static str,
    extractor: legible::Extractor,
    expected: ExpectedFixture,
}

#[cfg(feature = "bench-instrumentation")]
#[derive(Clone, Copy)]
struct ExpectedFixture {
    winner: &'static str,
    attempts: &'static str,
    root_id: Option<&'static str>,
    specialized: Option<&'static str>,
}

#[cfg(feature = "bench-instrumentation")]
fn strategy_fixtures() -> impl Iterator<Item = Fixture> {
    [
        (
            "normal",
            include_str!("../../benches/fixtures/strategies/normal/source.html"),
            legible::Extractor::builder().diagnostics(true).build(),
            ExpectedFixture {
                winner: "normal",
                attempts: "normal",
                root_id: None,
                specialized: None,
            },
        ),
        (
            "relaxed-cleanup",
            include_str!("../../benches/fixtures/strategies/relaxed-cleanup/source.html"),
            legible::Extractor::builder().diagnostics(true).build(),
            ExpectedFixture {
                winner: "relaxed-cleanup",
                attempts: "normal|relaxed-cleanup",
                root_id: None,
                specialized: None,
            },
        ),
        (
            "broad-content",
            include_str!("../../benches/fixtures/strategies/broad-content/source.html"),
            legible::Extractor::builder().diagnostics(true).build(),
            ExpectedFixture {
                winner: "broad-content",
                attempts: "normal|relaxed-cleanup|broad-content",
                root_id: Some("footer"),
                specialized: None,
            },
        ),
        (
            "structured-data-hint",
            include_str!("../../benches/fixtures/strategies/structured-data-hint/source.html"),
            legible::Extractor::builder().diagnostics(true).build(),
            ExpectedFixture {
                winner: "broad-content",
                attempts: "normal|relaxed-cleanup|broad-content|structured-data-hint|body-fallback",
                root_id: None,
                specialized: None,
            },
        ),
        (
            "relaxed-visibility",
            include_str!("../../benches/fixtures/strategies/relaxed-visibility/source.html"),
            legible::Extractor::builder().diagnostics(true).build(),
            ExpectedFixture {
                winner: "relaxed-visibility",
                attempts: "normal|relaxed-cleanup|broad-content|relaxed-visibility",
                root_id: None,
                specialized: None,
            },
        ),
        (
            "body-fallback",
            include_str!("../../benches/fixtures/strategies/body-fallback/source.html"),
            legible::Extractor::builder().diagnostics(true).build(),
            ExpectedFixture {
                winner: "body-fallback",
                attempts: "normal|relaxed-cleanup|broad-content|body-fallback",
                root_id: None,
                specialized: None,
            },
        ),
        (
            "exact-root",
            include_str!("../../benches/fixtures/strategies/exact-root/source.html"),
            legible::Extractor::builder()
                .diagnostics(true)
                .content_root(legible::ContentHint::Id("content".to_owned()))
                .build(),
            ExpectedFixture {
                winner: "normal",
                attempts: "normal",
                root_id: Some("content"),
                specialized: None,
            },
        ),
        (
            "specialized-reddit",
            include_str!("../../benches/fixtures/strategies/specialized-reddit/source.html"),
            legible::Extractor::builder().diagnostics(true).build(),
            ExpectedFixture {
                winner: "normal",
                attempts: "normal",
                root_id: None,
                specialized: Some("reddit"),
            },
        ),
    ]
    .into_iter()
    .map(|(name, html, extractor, expected)| Fixture {
        name,
        html,
        extractor,
        expected,
    })
}

#[cfg(feature = "bench-instrumentation")]
fn report_fixture(fixture: Fixture) {
    report(
        "strategy",
        fixture.name,
        fixture.html,
        fixture.extractor,
        Some(fixture.expected),
    );
}

#[cfg(feature = "bench-instrumentation")]
fn report(
    group: &str,
    name: &str,
    html: &str,
    extractor: legible::Extractor,
    expected: Option<ExpectedFixture>,
) {
    legible::instrumentation::reset();
    let result = extractor.extract(html, Some("https://example.com/articles/measure"));
    let rendered_bytes = result.as_ref().map_or(0, |page| {
        page.html().len() + page.markdown().len() + page.text().len()
    });
    let snapshot = legible::instrumentation::snapshot();
    let (winner, attempts, attempt_names) = result
        .as_ref()
        .ok()
        .and_then(|page| page.diagnostics())
        .map_or(("error", 0, String::new()), |diagnostics| {
            let winner = match diagnostics.selected_strategy {
                legible::ExtractionStrategyInfo::Normal => "normal",
                legible::ExtractionStrategyInfo::RelaxedCleanup => "relaxed-cleanup",
                legible::ExtractionStrategyInfo::BroadContent => "broad-content",
                legible::ExtractionStrategyInfo::StructuredDataHint => "structured-data-hint",
                legible::ExtractionStrategyInfo::RelaxedVisibility => "relaxed-visibility",
                legible::ExtractionStrategyInfo::BodyFallback => "body-fallback",
                _ => "unknown",
            };
            let attempts = diagnostics
                .attempts
                .iter()
                .map(|attempt| match attempt.strategy {
                    legible::ExtractionStrategyInfo::Normal => "normal",
                    legible::ExtractionStrategyInfo::RelaxedCleanup => "relaxed-cleanup",
                    legible::ExtractionStrategyInfo::BroadContent => "broad-content",
                    legible::ExtractionStrategyInfo::StructuredDataHint => "structured-data-hint",
                    legible::ExtractionStrategyInfo::RelaxedVisibility => "relaxed-visibility",
                    legible::ExtractionStrategyInfo::BodyFallback => "body-fallback",
                    _ => "unknown",
                })
                .collect::<Vec<_>>()
                .join("|");
            for attempt in &diagnostics.attempts {
                println!(
                    "attempt-detail/{group}/{name}: strategy={:?}, accepted={}, root_tag={:?}, root_id={:?}, selection={:?}, good={}, suspiciously_small={}, coverage={:.3}",
                    attempt.strategy,
                    attempt.accepted,
                    attempt.selected_root.tag,
                    attempt.selected_root.id,
                    attempt.selected_root.selection_reason,
                    attempt.quality.good,
                    attempt.quality.suspiciously_small,
                    attempt.quality.coverage,
                );
            }
            if let Some(identity) = diagnostics.specialized_extractor.as_deref() {
                println!("specialized/{group}/{name}={identity}");
            }
            if let Some(expected) = expected {
                assert_eq!(
                    diagnostics
                        .attempts
                        .iter()
                        .find(|attempt| {
                            attempt.accepted && attempt.strategy == diagnostics.selected_strategy
                        })
                        .and_then(|attempt| attempt.selected_root.id.as_deref()),
                    expected.root_id,
                    "unexpected selected root for {name}"
                );
                assert_eq!(
                    diagnostics.specialized_extractor.as_deref(),
                    expected.specialized,
                    "unexpected specialized extractor for {name}"
                );
            }
            (winner, diagnostics.attempts.len(), attempts)
        });
    if let Some(expected) = expected {
        if let Err(error) = &result {
            panic!("fixture {name} failed: {error:?}");
        }
        assert_eq!(winner, expected.winner, "unexpected winner for {name}");
        assert_eq!(
            attempt_names, expected.attempts,
            "unexpected attempts for {name}"
        );
    }
    println!("{group}/{name},{},{winner},{attempts}", html.len());
    println!("attempts/{group}/{name}={attempt_names}");
    println!("rendered/{group}/{name}={rendered_bytes}");
    for phase in legible::Phase::all() {
        println!(
            "phase/{group}/{name}/{}={}ns",
            phase.name(),
            snapshot.phases.get(*phase)
        );
    }
    let mut top_phases = legible::Phase::all()
        .iter()
        .map(|phase| (snapshot.phases.get(*phase), phase.name()))
        .collect::<Vec<_>>();
    top_phases.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.0));
    println!(
        "top-phases/{group}/{name}={}",
        top_phases
            .into_iter()
            .take(3)
            .map(|(nanos, name)| format!("{name}:{nanos}ns"))
            .collect::<Vec<_>>()
            .join("|")
    );
    let counters = snapshot.counters;
    let deferred = legible::instrumentation::deferred_work_snapshot();
    println!(
        "counters/{group}/{name}: parse_calls={}, source_full_scans={}, source_element_snapshots={}, content_hint_scans={}, content_excerpt_scans={}, final_dom_node_scans={}, external_footnote_scans={}, dom_clones={}, dom_clone_bytes={}, fragment_copies={}, strategies_started={}, unique_attempt_plans={}, scoring_nodes={}, cleaned_nodes={}, semantic_source_nodes={}, semantic_operations={}, allocations={}, allocated_bytes={}, deallocations={}, deallocated_bytes={}, peak_live_bytes={}, final_live_bytes={}, final_retained_bytes={}, builder_requested_capacity_bytes={}, builder_final_capacity_bytes={}, builder_peak_capacity_bytes={}, builder_reallocations={}, builder_max_open_depth={}, builder_shrink_bytes={}, builder_ops_capacity={}, builder_ends_capacity={}, builder_open_capacity={}, builder_text_capacity={}, builder_payload_capacity={}, builder_footnotes_capacity={}, builder_footnote_index_capacity={}, json_ld_bytes={}, json_ld_parsed_bytes={}, json_ld_retained_bytes={}",
        counters.parse_calls,
        counters.source_full_scans,
        counters.source_element_snapshots,
        deferred.content_hint_scans,
        deferred.content_excerpt_scans,
        deferred.final_dom_node_scans,
        deferred.external_footnote_scans,
        counters.dom_clones,
        counters.dom_clone_bytes,
        counters.fragment_copies,
        counters.strategies_started,
        counters.unique_attempt_plans,
        counters.scoring_nodes,
        counters.cleaned_nodes,
        counters.semantic_source_nodes,
        counters.semantic_operations,
        counters.allocations,
        counters.allocated_bytes,
        counters.deallocations,
        counters.deallocated_bytes,
        counters.peak_live_bytes,
        counters.final_live_bytes,
        counters.final_retained_bytes,
        counters.builder_requested_capacity_bytes,
        counters.builder_final_capacity_bytes,
        counters.builder_peak_capacity_bytes,
        counters.builder_reallocations,
        counters.builder_max_open_depth,
        counters.builder_shrink_bytes,
        counters.builder_ops_capacity,
        counters.builder_ends_capacity,
        counters.builder_open_capacity,
        counters.builder_text_capacity,
        counters.builder_payload_capacity,
        counters.builder_footnotes_capacity,
        counters.builder_footnote_index_capacity,
        counters.json_ld_bytes,
        counters.json_ld_parsed_bytes,
        counters.json_ld_retained_bytes,
    );
    if let Err(error) = result {
        println!("error/{group}/{name}: {error:?}");
    }
}

#[cfg(feature = "bench-instrumentation")]
fn rustc_version() -> String {
    if let Some(version) = option_env!("RUSTC_VERSION") {
        return version.to_owned();
    }
    let output = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .expect("rustc --version is required for the extraction report");
    assert!(
        output.status.success(),
        "rustc --version is required for the extraction report"
    );
    String::from_utf8(output.stdout)
        .expect("rustc --version must be valid UTF-8")
        .trim()
        .to_owned()
}

#[cfg(all(test, feature = "bench-instrumentation"))]
#[test]
fn strategy_fixture_contracts_are_stable() {
    for fixture in strategy_fixtures() {
        report_fixture(fixture);
    }
}

#[cfg(feature = "bench-instrumentation")]
fn generated_page(kind: &str, target_bytes: usize) -> String {
    let mut html = String::with_capacity(target_bytes + 256);
    html.push_str("<!doctype html><html><head><title>Measurement page</title>");
    if kind == "metadata" {
        for index in 0..200 {
            html.push_str(&format!(
                "<meta property='article:tag' content='measure-{index}'><meta name='citation_author' content='Author {index}'>"
            ));
        }
    } else if kind == "json-ld" {
        html.push_str("<script type='application/ld+json'>[");
        for index in 0..200 {
            if index > 0 {
                html.push(',');
            }
            html.push_str(&format!(
                r#"{{"@context":"https://schema.org","@type":"Article","headline":"Entry {index}","articleBody":"Representative structured article text {index}."}}"#
            ));
        }
        html.push_str("]</script>");
    }
    html.push_str("</head><body><nav>Home</nav><main><h1>Measurement page</h1>");
    let mut index = 0;
    while html.len() < target_bytes {
        if kind == "ordinary-inline" {
            html.push_str(&format!(
                "<section><h2>Section {index}</h2><p>Normal article text with <strong>strong emphasis containing <em>nested emphasis</em></strong>, <a href='/relative'>relative links</a>, and <code>inline code</code>.</p><blockquote><p>A quoted paragraph keeps ordinary block structure realistic.</p></blockquote><ul><li>First unordered item</li><li>Second unordered item</li></ul><ol><li>First ordered item</li><li>Second ordered item</li></ol><figure><img src='/image.jpg' alt='Useful image'><figcaption>A useful image caption.</figcaption></figure><details><summary>Additional context</summary><p>The expandable explanation remains ordinary semantic content.</p></details><dl><dt>Term</dt><dd>The definition list gives the common workload a native definition structure.</dd></dl></section>"
            ));
        } else {
            html.push_str(&format!(
                "<section><h2>Section {index}</h2><p>This paragraph contains representative prose, punctuation, and enough detail for measurement.</p><p>A second paragraph keeps source analysis realistic.</p></section>"
            ));
        }
        index += 1;
    }
    html.push_str("</main><footer>Footer</footer></body></html>");
    html
}
