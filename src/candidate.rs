//! Internal content-root candidates.

use crate::dom::{AttrName, Dom, NodeId, Tag};

const STRONG_IDS: &[&str] = &["post", "content", "article-content"];
const ARTICLE_TAG_PRIOR: f64 = 0.003;
const MAIN_TAG_PRIOR: f64 = 0.0025;
const ARTICLE_ROLE_PRIOR: f64 = 0.00275;
const OTHER_SEMANTIC_PRIOR: f64 = 0.0025;
const ADDITIONAL_SIGNAL_BONUS: f64 = 0.0005;
const MAX_SEMANTIC_PRIOR: f64 = 0.004;
const STRONG_CLASSES: &[&str] = &[
    "post-content",
    "post-body",
    "article-content",
    "article-body",
    "entry-content",
    "content-article",
    "markdown-body",
    "post",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateSource {
    Semantic,
    Readability,
    Generic,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CandidateSources(u8);

impl CandidateSources {
    fn insert(&mut self, source: CandidateSource) {
        self.0 |= match source {
            CandidateSource::Semantic => 1 << 0,
            CandidateSource::Readability => 1 << 1,
            CandidateSource::Generic => 1 << 2,
        };
    }

    fn contains(self, source: CandidateSource) -> bool {
        let mut source_only = Self::default();
        source_only.insert(source);
        self.0 & source_only.0 != 0
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    pub(crate) node: NodeId,
    sources: CandidateSources,
    pub(crate) semantic_prior: f64,
    pub(crate) score: f64,
}

impl Candidate {
    pub(crate) fn has_source(self, source: CandidateSource) -> bool {
        self.sources.contains(source)
    }
}

/// A deduplicated candidate collection indexed by stable DOM node ID.
#[derive(Debug)]
pub(crate) struct CandidateSet {
    candidates: Vec<Candidate>,
    positions: Vec<usize>,
}

pub(crate) struct CandidateContext {
    readability_in_subtree: Vec<bool>,
    has_authoritative_ancestor: Vec<bool>,
    authoritative_count: Vec<u32>,
    article_peer_count: Vec<u32>,
    article_peer_score: Vec<f64>,
}

impl CandidateContext {
    pub(crate) fn has_readability(&self, node: NodeId) -> bool {
        self.readability_in_subtree[node.index()]
    }

    pub(crate) fn has_authoritative_ancestor(&self, node: NodeId) -> bool {
        self.has_authoritative_ancestor[node.index()]
    }

    pub(crate) fn has_authoritative_descendant(&self, node: NodeId, own: bool) -> bool {
        self.authoritative_count[node.index()] > u32::from(own)
    }

    pub(crate) fn article_peer_summary(&self, node: NodeId) -> (u32, f64) {
        (
            self.article_peer_count[node.index()],
            self.article_peer_score[node.index()],
        )
    }
}

impl CandidateSet {
    pub(crate) fn discover_semantic(dom: &Dom) -> Self {
        let mut candidates = Self {
            candidates: Vec::new(),
            positions: vec![usize::MAX; dom.len()],
        };

        if let Some(body) = dom.body() {
            candidates.add(body, CandidateSource::Generic, 0.0);
        }

        for (node, _) in dom.element_descendants_snapshot_with_depth(dom.root()) {
            let tag_prior = match dom.tag(node) {
                Some(Tag::Article) => Some(ARTICLE_TAG_PRIOR),
                Some(Tag::Main) => Some(MAIN_TAG_PRIOR),
                _ => None,
            };
            if let Some(prior) = tag_prior {
                candidates.add(node, CandidateSource::Semantic, prior);
            }

            if let Some(role) = dom.attr(node, AttrName::Role) {
                if matches_role(role, "article") {
                    candidates.add(node, CandidateSource::Semantic, ARTICLE_ROLE_PRIOR);
                }
                if matches_role(role, "main") {
                    candidates.add(node, CandidateSource::Semantic, OTHER_SEMANTIC_PRIOR);
                }
            }

            if dom.attr(node, AttrName::Id).is_some_and(|id| {
                STRONG_IDS
                    .iter()
                    .any(|pattern| id.eq_ignore_ascii_case(pattern))
            }) {
                candidates.add(node, CandidateSource::Semantic, OTHER_SEMANTIC_PRIOR);
            }

            if dom.attr(node, AttrName::Class).is_some_and(|class| {
                class.split_whitespace().any(|token| {
                    STRONG_CLASSES
                        .iter()
                        .any(|pattern| token.eq_ignore_ascii_case(pattern))
                })
            }) {
                candidates.add(node, CandidateSource::Semantic, OTHER_SEMANTIC_PRIOR);
            }
        }

        candidates
    }

    pub(crate) fn add_readability(&mut self, node: NodeId, score: f64) {
        self.add(node, CandidateSource::Readability, score);
    }

    pub(crate) fn is_semantic(&self, node: NodeId) -> bool {
        self.get(node)
            .is_some_and(|candidate| candidate.has_source(CandidateSource::Semantic))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Candidate> {
        self.candidates.iter()
    }

    pub(crate) fn is_authoritative_semantic(&self, dom: &Dom, node: NodeId) -> bool {
        matches!(dom.tag(node), Some(Tag::Article | Tag::Main))
            || dom
                .attr(node, AttrName::Role)
                .is_some_and(|roles| matches_role(roles, "article") || matches_role(roles, "main"))
    }

    pub(crate) fn ranking_context(&self, dom: &Dom) -> CandidateContext {
        let mut readability_in_subtree = vec![false; dom.len()];
        let mut authoritative_count = vec![0_u32; dom.len()];
        let mut has_text = vec![false; dom.len()];
        for candidate in &self.candidates {
            readability_in_subtree[candidate.node.index()] =
                candidate.has_source(CandidateSource::Readability);
            if self.is_authoritative_semantic(dom, candidate.node) {
                authoritative_count[candidate.node.index()] = 1;
            }
        }

        let nodes: Vec<_> = dom.descendants(dom.root()).collect();
        for &node in &nodes {
            has_text[node.index()] = dom
                .text_node(node)
                .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()));
        }
        for &node in nodes.iter().rev() {
            if let Some(parent) = dom.parent(node) {
                readability_in_subtree[parent.index()] |= readability_in_subtree[node.index()];
                authoritative_count[parent.index()] = authoritative_count[parent.index()]
                    .saturating_add(authoritative_count[node.index()]);
                has_text[parent.index()] |= has_text[node.index()];
            }
        }

        let mut nearest_authoritative_ancestor = vec![None; dom.len()];
        for (node, _) in dom.element_descendants_snapshot_with_depth(dom.root()) {
            if let Some(parent) = dom.parent(node) {
                nearest_authoritative_ancestor[node.index()] =
                    if self.is_authoritative_semantic(dom, parent) {
                        Some(parent)
                    } else {
                        nearest_authoritative_ancestor[parent.index()]
                    };
            }
        }

        let mut article_peer_count = vec![0_u32; dom.len()];
        let mut article_peer_score = vec![0.0; dom.len()];
        for candidate in &self.candidates {
            let is_article = dom.tag(candidate.node) == Some(Tag::Article)
                || dom
                    .attr(candidate.node, AttrName::Role)
                    .is_some_and(|role| matches_role(role, "article"));
            let Some(parent) = nearest_authoritative_ancestor[candidate.node.index()] else {
                continue;
            };
            if is_article && has_text[candidate.node.index()] {
                article_peer_count[parent.index()] += 1;
                article_peer_score[parent.index()] += candidate.score;
            }
        }

        CandidateContext {
            readability_in_subtree,
            has_authoritative_ancestor: nearest_authoritative_ancestor
                .into_iter()
                .map(|ancestor| ancestor.is_some())
                .collect(),
            authoritative_count,
            article_peer_count,
            article_peer_score,
        }
    }

    fn get(&self, node: NodeId) -> Option<&Candidate> {
        let position = self.positions.get(node.index()).copied()?;
        (position != usize::MAX).then(|| &self.candidates[position])
    }

    fn add(&mut self, node: NodeId, source: CandidateSource, value: f64) {
        if node.index() >= self.positions.len() {
            self.positions.resize(node.index() + 1, usize::MAX);
        }
        let position = self.positions[node.index()];
        if position == usize::MAX {
            self.positions[node.index()] = self.candidates.len();
            let mut sources = CandidateSources::default();
            sources.insert(source);
            self.candidates.push(Candidate {
                node,
                sources,
                semantic_prior: if source == CandidateSource::Semantic {
                    value
                } else {
                    0.0
                },
                score: if source == CandidateSource::Readability {
                    value
                } else {
                    0.0
                },
            });
            return;
        }

        let candidate = &mut self.candidates[position];
        let already_had_source = candidate.sources.contains(source);
        candidate.sources.insert(source);
        match source {
            CandidateSource::Semantic => {
                // Independent semantic signals increase confidence, but a node
                // cannot gain an unbounded score from redundant attributes.
                candidate.semantic_prior = candidate.semantic_prior.max(value);
                if already_had_source {
                    candidate.semantic_prior = (candidate.semantic_prior + ADDITIONAL_SIGNAL_BONUS)
                        .min(MAX_SEMANTIC_PRIOR);
                }
            }
            CandidateSource::Readability => candidate.score = candidate.score.max(value),
            CandidateSource::Generic => {}
        }
    }
}

fn matches_role(roles: &str, expected: &str) -> bool {
    roles
        .split_whitespace()
        .any(|role| role.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_and_merges_semantic_signals() {
        let dom = Dom::parse_document(
            r#"<body><main id="content" role="main"><div class="markdown-body"></div></main></body>"#,
        )
        .unwrap();
        let main = dom.first_descendant_by_tag(dom.root(), Tag::Main).unwrap();
        let markdown = dom.first_descendant_by_tag(dom.root(), Tag::Div).unwrap();
        let candidates = CandidateSet::discover_semantic(&dom);

        let main_candidate = candidates.get(main).unwrap();
        assert!(main_candidate.has_source(CandidateSource::Semantic));
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.node == main)
                .count(),
            1
        );
        assert_eq!(main_candidate.semantic_prior, 0.0035);
        assert!(candidates.is_semantic(markdown));
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.has_source(CandidateSource::Generic))
        );
    }

    #[test]
    fn strong_names_match_complete_tokens_only() {
        let dom = Dom::parse_document(
            r#"<body><div class="postscript"></div><div class="post"></div></body>"#,
        )
        .unwrap();
        let divs: Vec<_> = dom
            .element_descendants_snapshot_with_depth(dom.root())
            .into_iter()
            .map(|(node, _)| node)
            .filter(|&node| dom.tag(node) == Some(Tag::Div))
            .collect();
        let candidates = CandidateSet::discover_semantic(&dom);

        assert!(!candidates.is_semantic(divs[0]));
        assert!(candidates.is_semantic(divs[1]));
    }

    #[test]
    fn semantic_roots_drive_extraction() {
        for opening in [
            "<main>",
            "<article>",
            "<div class=markdown-body>",
            "<div class=entry-content>",
            "<div id=content>",
            "<div role=main>",
            "<div role=article>",
        ] {
            let closing = if opening.starts_with("<main") {
                "</main>"
            } else if opening.starts_with("<article") {
                "</article>"
            } else {
                "</div>"
            };
            let html = format!(
                "<body><p>Outside clutter</p>{opening}<p>Chosen semantic content has enough text to preserve.</p>{closing}</body>"
            );
            let markdown = crate::extract(&html, None).unwrap().markdown();
            assert!(markdown.contains("Chosen semantic content"), "{opening}");
            assert!(!markdown.contains("Outside clutter"), "{opening}");
        }
    }

    #[test]
    fn nested_article_is_more_specific_than_main() {
        let html = r#"<body><main><p>Main introduction</p><article>
            <p>The nested article is the selected specific content root.</p>
        </article></main></body>"#;
        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("nested article"));
        assert!(!markdown.contains("Main introduction"));
    }

    #[test]
    fn main_keeps_multiple_article_cards() {
        let html = r#"<body><main>
            <article><h2>First card</h2><p>First useful summary. It has much more prose, several clauses, and enough detail to outscore the other card.</p></article>
            <article><h2>Second card</h2><p>Second useful summary.</p></article>
        </main></body>"#;
        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("First card"), "{markdown}");
        assert!(markdown.contains("Second card"), "{markdown}");
    }

    #[test]
    fn short_main_beats_long_header_boilerplate() {
        for answer in ["Short useful answer.", "This is a short useful answer."] {
            let html = format!(
                r#"<body>
                    <header><p>This long site header explains navigation, account controls, subscriptions, promotions, and other boilerplate.</p></header>
                    <main><p>{answer}</p></main>
                </body>"#
            );
            let markdown = crate::extract(&html, None).unwrap().markdown();

            assert!(markdown.contains(answer), "{answer}: {markdown}");
            assert!(
                !markdown.contains("long site header"),
                "{answer}: {markdown}"
            );
        }
    }

    #[test]
    fn empty_article_does_not_promote_main() {
        let html = r#"<body><main><p>Unrelated intro</p>
            <article><p>This substantive nested article is the selected content root.</p></article>
            <article></article>
        </main></body>"#;
        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("substantive nested article"));
        assert!(!markdown.contains("Unrelated intro"));
    }

    #[test]
    fn weak_wrappers_do_not_hide_authoritative_semantics() {
        let short_main = r#"<body>
            <header><p>This long site header explains navigation, account controls, subscriptions, promotions, and other boilerplate.</p></header>
            <div id="content"><main><p>Short wrapped answer.</p></main></div>
        </body>"#;
        let markdown = crate::extract(short_main, None).unwrap().markdown();
        assert!(markdown.contains("Short wrapped answer"));
        assert!(!markdown.contains("long site header"));

        let wrapped_cards = r#"<body><main>
            <div class="post"><article><h2>First wrapped card</h2><p>First summary.</p></article></div>
            <div class="post"><article><h2>Second wrapped card</h2><p>Second summary.</p></article></div>
        </main></body>"#;
        let markdown = crate::extract(wrapped_cards, None).unwrap().markdown();
        assert!(markdown.contains("First wrapped card"));
        assert!(markdown.contains("Second wrapped card"));
    }

    #[test]
    fn weak_semantic_name_does_not_override_prose() {
        for weak in [r#"class="post""#, r#"id="content""#] {
            let html = format!(
                r#"<body><div {weak}>Related post</div><section><p>Actual useful answer.</p></section></body>"#
            );
            let markdown = crate::extract(&html, None).unwrap().markdown();

            assert!(
                markdown.contains("Actual useful answer"),
                "{weak}: {markdown}"
            );
        }
    }
}
