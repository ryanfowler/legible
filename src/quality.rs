//! Extraction quality metrics and best-attempt comparison.

use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, Tag};
use crate::scoring::{
    get_link_density_cached, get_normalized_inner_text, get_or_compute_stats,
    get_or_compute_stats_excluding,
};

/// Text and structure measured for one DOM region.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ContentMetrics {
    pub(crate) word_count: usize,
    pub(crate) text_chars: usize,
    pub(crate) paragraph_count: usize,
    pub(crate) heading_count: usize,
    pub(crate) structured_block_count: usize,
    pub(crate) link_density: f64,
    has_alphanumeric_text: bool,
}

impl ContentMetrics {
    /// Measures source content after excluding document-level navigation and
    /// chrome. Semantic headers inside a main/article region remain source
    /// content.
    pub(crate) fn measure_source(dom: &Dom, root: NodeId) -> Self {
        Self::measure_source_with_visibility(dom, root, false)
    }

    /// Measures source content while retaining static visibility markers.
    /// ARIA-hidden content and document chrome remain excluded.
    pub(crate) fn measure_source_relaxed_visibility(dom: &Dom, root: NodeId) -> Self {
        Self::measure_source_with_visibility(dom, root, true)
    }

    fn measure_source_with_visibility(
        dom: &Dom,
        root: NodeId,
        relax_static_visibility: bool,
    ) -> Self {
        let elements = dom.element_descendants_snapshot_with_depth(root);
        let has_primary_region = elements.iter().any(|&(node, _)| {
            matches!(dom.tag(node), Some(Tag::Main | Tag::Article))
                || dom.attr(node, AttrName::Role).is_some_and(is_primary_role)
        });
        let mut in_primary_region = vec![false; dom.len()];
        for &(node, _) in &elements {
            let parent_is_primary = dom
                .parent(node)
                .is_some_and(|parent| in_primary_region[parent.index()]);
            in_primary_region[node.index()] = parent_is_primary
                || matches!(dom.tag(node), Some(Tag::Main | Tag::Article))
                || dom.attr(node, AttrName::Role).is_some_and(is_primary_role);
        }
        let mut excluded = vec![false; dom.len()];
        for &(node, _) in &elements {
            let tag = dom.tag(node);
            let statically_hidden = dom.has_attr(node, AttrName::Hidden)
                || dom.attr(node, AttrName::Style).is_some_and(|style| {
                    let compact = style
                        .bytes()
                        .filter(|byte| !byte.is_ascii_whitespace())
                        .map(char::from)
                        .collect::<String>()
                        .to_ascii_lowercase();
                    compact.contains("display:none") || compact.contains("visibility:hidden")
                });
            let utility_hidden = dom.attr(node, AttrName::Class).is_some_and(|classes| {
                classes.split_whitespace().any(|class| {
                    ["invisible", "d-none", "display-none", "u-hidden"]
                        .iter()
                        .any(|expected| class.eq_ignore_ascii_case(expected))
                })
            });
            let modal_class = (statically_hidden || utility_hidden)
                && dom.attr(node, AttrName::Class).is_some_and(|classes| {
                    classes.split_whitespace().any(|class| {
                        class.eq_ignore_ascii_case("modal") || class.eq_ignore_ascii_case("dialog")
                    })
                });
            let hidden = dom.attr(node, AttrName::AriaHidden) == Some("true")
                || !relax_static_visibility && (statically_hidden || utility_hidden)
                || dom.attr(node, AttrName::Role).is_some_and(|roles| {
                    roles.split_whitespace().any(|role| {
                        role.eq_ignore_ascii_case("dialog")
                            || role.eq_ignore_ascii_case("alertdialog")
                    })
                })
                || dom.attr(node, AttrName::AriaModal) == Some("true")
                || modal_class;
            let hard_non_content = hidden
                || matches!(
                    tag,
                    Some(
                        Tag::Script
                            | Tag::Style
                            | Tag::Template
                            | Tag::Meta
                            | Tag::Link
                            | Tag::Input
                            | Tag::Textarea
                            | Tag::Select
                            | Tag::Button
                            | Tag::Datalist
                            | Tag::Option
                            | Tag::Iframe
                            | Tag::Embed
                            | Tag::Object
                    )
                );
            let role = dom.attr(node, AttrName::Role);
            let document_chrome = matches!(tag, Some(Tag::Header | Tag::Footer | Tag::Nav))
                || role.is_some_and(|roles| {
                    roles.split_whitespace().any(|role| {
                        role.eq_ignore_ascii_case("banner")
                            || role.eq_ignore_ascii_case("navigation")
                    })
                });
            let contextual_sidebar = tag == Some(Tag::Aside)
                || role.is_some_and(|roles| {
                    roles
                        .split_whitespace()
                        .any(|role| role.eq_ignore_ascii_case("complementary"))
                });
            excluded[node.index()] = hard_non_content
                || document_chrome && !in_primary_region[node.index()]
                || contextual_sidebar && has_primary_region && !in_primary_region[node.index()];
        }
        Self::measure_filtered(dom, root, &elements, &excluded)
    }

    pub(crate) fn measure(dom: &Dom, root: NodeId) -> Self {
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        let text = get_or_compute_stats(dom, root, &mut store);
        let link_density = get_link_density_cached(dom, root, text.text_length, &mut store);
        let mut metrics = Self::from_text_stats(text, link_density, false);
        for node in std::iter::once(root).chain(dom.descendants(root)) {
            metrics.has_alphanumeric_text |= dom
                .text_node(node)
                .is_some_and(|text| text.chars().any(char::is_alphanumeric));
            metrics.count_structure(dom.tag(node));
        }
        metrics
    }

    fn measure_filtered(
        dom: &Dom,
        root: NodeId,
        elements: &[(NodeId, u32)],
        excluded: &[bool],
    ) -> Self {
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        let text = get_or_compute_stats_excluding(dom, root, &mut store, excluded);
        let link_density = get_link_density_cached(dom, root, text.text_length, &mut store);
        let mut inside_excluded = vec![false; dom.len()];
        let has_alphanumeric_text =
            std::iter::once(root)
                .chain(dom.descendants(root))
                .any(|node| {
                    let parent_is_excluded = dom
                        .parent(node)
                        .is_some_and(|parent| inside_excluded[parent.index()]);
                    inside_excluded[node.index()] = excluded[node.index()] || parent_is_excluded;
                    !inside_excluded[node.index()]
                        && dom
                            .text_node(node)
                            .is_some_and(|text| text.chars().any(char::is_alphanumeric))
                });
        let mut metrics = Self::from_text_stats(text, link_density, has_alphanumeric_text);
        let mut excluded_depth = None;
        for &(node, depth) in elements {
            if let Some(boundary) = excluded_depth {
                if depth > boundary {
                    continue;
                }
                excluded_depth = None;
            }
            if excluded[node.index()] {
                excluded_depth = Some(depth);
                continue;
            }
            metrics.count_structure(dom.tag(node));
        }
        metrics
    }

    fn from_text_stats(
        text: crate::dom::NodeStats,
        link_density: f64,
        has_alphanumeric_text: bool,
    ) -> Self {
        Self {
            word_count: text.word_count as usize,
            text_chars: text.text_length as usize,
            link_density,
            has_alphanumeric_text,
            ..Self::default()
        }
    }

    fn count_structure(&mut self, tag: Option<Tag>) {
        match tag {
            Some(Tag::P) => self.paragraph_count += 1,
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6) => {
                self.heading_count += 1
            }
            Some(
                Tag::Pre
                | Tag::Table
                | Tag::Figure
                | Tag::Blockquote
                | Tag::Details
                | Tag::Dl
                | Tag::Math
                | Tag::Ol
                | Tag::Ul,
            ) => self.structured_block_count += 1,
            _ => {}
        }
    }

    pub(crate) fn has_meaningful_text(self) -> bool {
        self.has_alphanumeric_text && self.word_count > 0 && self.text_chars > 0
    }
}

/// Rates an extraction relative to the meaningful source body.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExtractionQuality {
    pub(crate) word_count: usize,
    pub(crate) text_chars: usize,
    pub(crate) source_word_count: usize,
    pub(crate) source_text_chars: usize,
    pub(crate) coverage: f64,
    pub(crate) paragraph_count: usize,
    pub(crate) heading_count: usize,
    pub(crate) structured_block_count: usize,
    pub(crate) link_density: f64,
    has_alphanumeric_text: bool,
    root_specificity: f64,
}

impl ExtractionQuality {
    pub(crate) fn new(source: ContentMetrics, result: ContentMetrics, specific_root: bool) -> Self {
        let char_coverage = ratio(result.text_chars, source.text_chars);
        Self {
            word_count: result.word_count,
            text_chars: result.text_chars,
            source_word_count: source.word_count,
            source_text_chars: source.text_chars,
            // Character coverage remains stable for languages that do not use
            // spaces between words. Word counts are still useful as absolute
            // quality signals.
            coverage: char_coverage,
            paragraph_count: result.paragraph_count,
            heading_count: result.heading_count,
            structured_block_count: result.structured_block_count,
            link_density: result.link_density,
            has_alphanumeric_text: result.has_alphanumeric_text,
            root_specificity: if specific_root { 1.0 } else { 0.0 },
        }
    }

    /// A short result is good when it retains most of a short source. Longer
    /// results can be good at lower coverage when they retain clear structure.
    pub(crate) fn is_good(self) -> bool {
        if !self.has_alphanumeric_text || self.word_count == 0 || self.text_chars == 0 {
            return false;
        }
        if self.source_word_count <= 60 || self.source_text_chars <= 400 {
            return self.coverage >= 0.45;
        }
        if self.coverage >= 0.6 {
            return true;
        }
        if self.word_count >= 80 && self.coverage >= 0.3 {
            return true;
        }
        if (self.word_count >= 150 || self.text_chars >= 1_000)
            && self.paragraph_count >= 3
            && self.coverage >= 0.05
        {
            return true;
        }
        self.structured_block_count > 0 && self.word_count >= 20 && self.coverage >= 0.25
    }

    pub(crate) fn is_suspiciously_small(self) -> bool {
        !self.has_alphanumeric_text
            || self.word_count == 0
            || self.text_chars == 0
            || self.source_word_count >= 80 && self.word_count < 15
            || self.source_text_chars >= 1_000 && self.coverage < 0.15
    }

    /// Scores attempts without treating the longest result as automatically
    /// best. Specific roots and useful structure offset a moderate loss of
    /// source coverage. Link-heavy results receive only a bounded penalty.
    pub(crate) fn best_attempt_score(self) -> f64 {
        let structure = (self.structured_block_count as f64 * 2.0).min(12.0)
            + (self.paragraph_count as f64 * 0.25).min(4.0)
            + (self.heading_count as f64 * 0.5).min(4.0);
        self.coverage * 100.0 + self.root_specificity * 12.0 + structure
            - (self.link_density * 12.0).min(10.0)
    }
}

/// Detects a dominant access gate. A match needs structural and textual
/// evidence, except for explicit machine-generated denial text.
pub(crate) fn is_access_barrier(dom: &Dom, root: NodeId) -> bool {
    let mut buffer = String::new();
    let text = normalize_barrier_text(get_normalized_inner_text(dom, root, &mut buffer));
    if text.is_empty() {
        return false;
    }
    let heading = std::iter::once(root)
        .chain(dom.descendants(root))
        .find(|&node| {
            matches!(dom.tag(node), Some(Tag::H1 | Tag::H2 | Tag::H3))
                && dom.has_non_whitespace_text(node)
        })
        .map(|node| normalize_barrier_text(get_normalized_inner_text(dom, node, &mut buffer)))
        .unwrap_or_default();
    let strong_denial_heading = matches!(
        heading.trim_matches(
            |character: char| character.is_ascii_punctuation() || character.is_whitespace()
        ),
        "access denied"
            | "request blocked"
            | "verify you are human"
            | "acces refuse"
            | "acces restreint"
            | "requete bloquee"
    );
    let exact_gate_heading = strong_denial_heading
        || matches!(
            heading.trim_matches(
                |character: char| character.is_ascii_punctuation() || character.is_whitespace()
            ),
            "subscription required"
                | "subscribe to unlock"
                | "subscribe to unlock this article"
                | "content locked"
                | "article unavailable"
        );
    let heading_gate = [
        "access denied",
        "access restricted",
        "request blocked",
        "verify you are human",
        "subscription required",
        "subscribe to unlock",
        "content locked",
        "article unavailable",
        "acces refuse",
        "acces restreint",
        "requete bloquee",
        "contenu indisponible",
    ]
    .iter()
    .any(|phrase| heading.starts_with(phrase));
    let structural_gate = std::iter::once(root)
        .chain(dom.descendants(root))
        .any(|node| {
            [AttrName::Class, AttrName::Id]
                .into_iter()
                .filter_map(|name| dom.attr(node, name))
                .flat_map(|value| value.split(|character: char| !character.is_ascii_alphanumeric()))
                .any(|token| {
                    matches!(
                        token.to_ascii_lowercase().as_str(),
                        "paywall" | "barrier" | "gate" | "subscribe"
                    )
                })
        });
    let action = [
        "sign in to continue",
        "log in to continue",
        "subscribe to continue",
        "subscribe to unlock",
        "choose a plan",
        "start your trial",
        "verify you are human",
        "enable cookies",
        "try again later",
        "connectez-vous pour continuer",
        "abonnez-vous pour continuer",
        "verifiez que vous etes humain",
        "obtenir une autorisation",
        "autorisation d'acces",
    ]
    .iter()
    .filter(|phrase| text.contains(**phrase))
    .count();
    let automated = text.contains("automated traffic")
        || text.contains("identified as automated")
        || text.contains("bot detection")
        || text.contains("trafic a ete identifie comme automatise")
        || text.contains("activite de bot")
        || text.contains("trafic automatise");
    let request_identifier = text.contains("request id")
        || text.contains("client ip")
        || text.contains("incident id")
        || text.contains("identifiant de requete")
        || text.contains("adresse ip")
        || text.contains("identifiant d'incident");
    let machine_denial = automated
        && (request_identifier
            || text.contains("access denied")
            || text.contains("acces refuse")
            || text.contains("acces restreint")
            || text.contains("verify you are human"));
    let direct_automation_notice = text.contains("your traffic was identified as automated")
        || text.contains("your traffic has been identified as automated")
        || text.contains("votre trafic a ete identifie comme automatise");
    let explicit_machine_denial = (automated
        && (strong_denial_heading || denial_permission_text(&text))
        && (request_identifier || action > 0))
        || direct_automation_notice && request_identifier && action > 0;
    let denial_support = denial_permission_text(&text);
    let offer = [" per month", "/month", "monthly", "annual", "free trial"]
        .iter()
        .filter(|term| text.contains(**term))
        .count()
        + usize::from(text.contains('$') || text.contains('€') || text.contains('£'));

    machine_denial && strong_denial_heading
        || explicit_machine_denial
        || strong_denial_heading && denial_support
        || exact_gate_heading && action > 0
        || heading_gate && structural_gate
        || structural_gate && action > 0 && offer >= 2
}

/// Detects a control-dominated application shell with no explanatory content.
/// Extraction does not execute the client code that would populate such a page.
pub(crate) fn is_interactive_shell(dom: &Dom, root: NodeId) -> bool {
    let metrics = ContentMetrics::measure(dom, root);
    if metrics.word_count > 20 || metrics.paragraph_count > 0 || metrics.heading_count > 0 {
        return false;
    }
    let controls = std::iter::once(root)
        .chain(dom.descendants(root))
        .filter(|&node| {
            matches!(
                dom.tag(node),
                Some(
                    Tag::Button
                        | Tag::Input
                        | Tag::Select
                        | Tag::Textarea
                        | Tag::Form
                        | Tag::Datalist
                )
            )
        })
        .count();
    let data_structure = dom.descendants(root).any(|node| {
        matches!(
            dom.tag(node),
            Some(Tag::Table | Tag::Pre | Tag::Dl | Tag::Ol | Tag::Ul)
        )
    });
    controls >= 2 && !data_structure
}

/// Rejects only very short fragments that contain values but no lexical or
/// structural context.
pub(crate) fn is_incoherent_short_result(dom: &Dom, root: NodeId, metrics: ContentMetrics) -> bool {
    if metrics.text_chars > 200 || metrics.word_count > 20 {
        return false;
    }
    let (alphabetic_chars, digit_chars) = std::iter::once(root)
        .chain(dom.descendants(root))
        .filter_map(|node| dom.text_node(node))
        .flat_map(str::chars)
        .fold((0_usize, 0_usize), |(letters, digits), character| {
            (
                letters + usize::from(character.is_alphabetic()),
                digits + usize::from(character.is_numeric()),
            )
        });
    let has_lexical_text = alphabetic_chars > 0;
    let contextual_structure = dom
        .descendants(root)
        .any(|node| matches!(dom.tag(node), Some(Tag::Th | Tag::Pre | Tag::Math)));
    let unlabeled_values = alphabetic_chars <= 16
        && digit_chars >= alphabetic_chars.saturating_mul(2).max(4)
        && metrics.structured_block_count == 0;
    (!has_lexical_text || unlabeled_values) && !contextual_structure
}

fn denial_permission_text(text: &str) -> bool {
    [
        "permission to access",
        "do not have permission",
        "not authorized",
        "authorization required",
        "forbidden",
        "autorisation d'acces",
        "obtenir une autorisation",
        "acces non autorise",
        "vous n'etes pas autorise",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

fn normalize_barrier_text(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|character| match character {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            other => other,
        })
        .collect()
}

fn is_primary_role(roles: &str) -> bool {
    roles
        .split_whitespace()
        .any(|role| role.eq_ignore_ascii_case("main") || role.eq_ignore_ascii_case("article"))
}

fn ratio(value: usize, total: usize) -> f64 {
    if total == 0 {
        f64::from(value == 0)
    } else {
        (value as f64 / total as f64).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(words: usize, chars: usize, blocks: usize, links: f64) -> ContentMetrics {
        ContentMetrics {
            word_count: words,
            text_chars: chars,
            structured_block_count: blocks,
            link_density: links,
            has_alphanumeric_text: true,
            ..ContentMetrics::default()
        }
    }

    #[test]
    fn source_metrics_filter_nested_hidden_and_chrome_regions() {
        let source = Dom::parse_document(
            r#"<body><header><a href="/home">Home nav words</a></header><main><header><h1>Article title</h1></header><p>First visible paragraph.</p><div hidden><p>Hidden outer <span>nested words.</span></p><div><a href="/hidden">Hidden link words</a></div></div><p>Second <a href="/guide">visible guide</a>.</p><footer><p>Article footer note.</p></footer></main><aside role="complementary"><p>Outside sidebar content.</p></aside><footer><p>Global footer content.</p></footer></body>"#,
        )
        .unwrap();
        let expected = Dom::parse_document(
            r#"<body><main><header><h1>Article title</h1></header><p>First visible paragraph.</p><p>Second <a href="/guide">visible guide</a>.</p><footer><p>Article footer note.</p></footer></main></body>"#,
        )
        .unwrap();
        let source_body = source.body().unwrap();
        let expected_main = expected
            .first_descendant_by_tag(expected.root(), Tag::Main)
            .unwrap();
        let actual = ContentMetrics::measure_source(&source, source_body);
        let expected = ContentMetrics::measure(&expected, expected_main);

        assert_eq!(actual.word_count, expected.word_count);
        assert_eq!(actual.text_chars, expected.text_chars);
        assert_eq!(actual.paragraph_count, expected.paragraph_count);
        assert_eq!(actual.heading_count, expected.heading_count);
        assert_eq!(
            actual.structured_block_count,
            expected.structured_block_count
        );
        assert_eq!(actual.link_density, expected.link_density);
        assert_eq!(actual.has_alphanumeric_text, expected.has_alphanumeric_text);
    }

    #[test]
    fn distinguishes_short_valid_and_large_source_tiny_results() {
        let short =
            ExtractionQuality::new(metrics(35, 180, 0, 0.0), metrics(30, 160, 0, 0.0), true);
        assert!(short.is_good());
        assert!(!short.is_suspiciously_small());

        let tiny = ExtractionQuality::new(
            metrics(4_000, 24_000, 0, 0.0),
            metrics(30, 180, 0, 0.0),
            true,
        );
        assert!(!tiny.is_good());
        assert!(tiny.is_suspiciously_small());
    }

    #[test]
    fn accepts_meaningful_link_and_structured_results() {
        let links =
            ExtractionQuality::new(metrics(100, 700, 1, 0.9), metrics(85, 600, 1, 0.9), true);
        assert!(links.is_good());

        let code =
            ExtractionQuality::new(metrics(100, 700, 2, 0.0), metrics(30, 250, 2, 0.0), true);
        assert!(code.is_good());
    }

    #[test]
    fn punctuation_only_result_is_not_good() {
        let source = metrics(20, 120, 0, 0.0);
        let mut punctuation = metrics(20, 120, 0, 0.0);
        punctuation.has_alphanumeric_text = false;
        let quality = ExtractionQuality::new(source, punctuation, true);
        assert!(!quality.is_good());
        assert!(quality.is_suspiciously_small());
    }

    #[test]
    fn classifies_access_gates_without_rejecting_discussion() {
        let denied = Dom::parse_document(
            r#"<body><main class="challenge"><h1>Access denied</h1><p>Automated traffic was detected. Verify you are human.</p><p>Request ID: 123</p></main></body>"#,
        )
        .unwrap();
        assert!(is_access_barrier(&denied, denied.body().unwrap()));

        let wall = Dom::parse_document(
            r#"<body><main class="paywall"><h1>Subscribe to unlock this article</h1><p>Choose a plan and start your trial.</p><p>$9 per month. $90 annual.</p></main></body>"#,
        )
        .unwrap();
        assert!(is_access_barrier(&wall, wall.body().unwrap()));

        let french = Dom::parse_document(
            r#"<html lang="fr"><body><main><h1>Accès restreint</h1><p>Votre trafic a été identifié comme automatisé (bot). Si vous souhaitez obtenir une autorisation d’accès à ce contenu, contactez-nous.</p><p>Adresse IP : 192.0.2.1. Identifiant de requête : abc.</p></main></body></html>"#,
        )
        .unwrap();
        assert!(is_access_barrier(&french, french.body().unwrap()));

        let generic_heading = Dom::parse_document(
            r#"<body><main><h1>Something went wrong</h1><p>Your traffic was identified as automated. Verify you are human.</p><p>Request ID: 123.</p></main></body>"#,
        )
        .unwrap();
        assert!(is_access_barrier(
            &generic_heading,
            generic_heading.body().unwrap()
        ));

        let discussion = Dom::parse_document(
            r#"<body><main class="challenge"><article><h1>How bot detection works</h1><p>This article explains automated traffic and request IDs without blocking the reader.</p><p>A sample plan costs $9 per month.</p></article></main></body>"#,
        )
        .unwrap();
        assert!(!is_access_barrier(&discussion, discussion.body().unwrap()));

        let troubleshooting = Dom::parse_document(
            r#"<body><main class="challenge"><article><h1>Access denied troubleshooting</h1><p>Bot detection systems can classify automated traffic. Support engineers use a request ID to find the relevant diagnostic record.</p><p>This guide explains the policy and its recovery design for application developers.</p></article></main></body>"#,
        )
        .unwrap();
        assert!(!is_access_barrier(
            &troubleshooting,
            troubleshooting.body().unwrap()
        ));

        let recovery_guide = Dom::parse_document(
            r#"<body><article class="barrier"><h1>Human verification recovery</h1><p>Bot detection can associate automated traffic with a request ID. If the prompt says verify you are human, follow the documented recovery procedure.</p><p>The rest of this support article explains diagnosis, accessibility, and account recovery in detail.</p></article></body>"#,
        )
        .unwrap();
        assert!(!is_access_barrier(
            &recovery_guide,
            recovery_guide.body().unwrap()
        ));

        let short_guide = Dom::parse_document(
            r#"<body><article><h1>Access denied troubleshooting</h1><p>If a stale browser session causes this message, enable cookies and reload the application.</p><p>This short guide explains the recovery steps.</p></article></body>"#,
        )
        .unwrap();
        assert!(!is_access_barrier(
            &short_guide,
            short_guide.body().unwrap()
        ));

        let forbidden = Dom::parse_document(
            r#"<body><main><h1>Access denied</h1><p>You do not have permission to access this resource.</p></main></body>"#,
        )
        .unwrap();
        assert!(is_access_barrier(&forbidden, forbidden.body().unwrap()));
    }

    #[test]
    fn short_coherence_uses_lexical_and_structural_context() {
        let ruler = Dom::parse_fragment("<div>11.1×10¹⁹ 2.2×10¹⁹</div>", Tag::Div).unwrap();
        let root = ruler.root();
        assert!(is_incoherent_short_result(
            &ruler,
            root,
            ContentMetrics::measure(&ruler, root)
        ));

        for html in [
            "<p>Status: 200 OK</p>",
            "<table><tr><th>Value</th></tr><tr><td>42</td></tr></table>",
            "<pre><code>42</code></pre>",
            "<math><mn>42</mn></math>",
        ] {
            let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
            let root = dom.root();
            assert!(
                !is_incoherent_short_result(&dom, root, ContentMetrics::measure(&dom, root)),
                "{html}"
            );
        }
    }

    #[test]
    fn best_attempt_is_not_selected_only_by_length() {
        let focused =
            ExtractionQuality::new(metrics(100, 700, 2, 0.2), metrics(75, 520, 2, 0.0), true);
        let broad_links =
            ExtractionQuality::new(metrics(100, 700, 2, 0.2), metrics(85, 600, 0, 0.95), false);
        assert!(focused.best_attempt_score() > broad_links.best_attempt_score());
    }
}
