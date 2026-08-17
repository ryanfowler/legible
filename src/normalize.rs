//! Source preparation and relevance cleanup for retained content.

mod headings;
mod images;
mod lists;
mod media;
mod svg;

use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::is_element_without_content;

/// Prepares and cleans retained source markup before semantic compilation.
///
/// The selected DOM keeps semantic source evidence for the document compiler.
/// These passes only protect meaningful media and remove artifacts that affect
/// relevance or result quality.
#[cfg(test)]
fn normalize_semantics(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    prepare_media_before_cleanup(dom, root);
    cleanup_selected_content(dom, root, nodes, false);
}

/// Removes selected-content artifacts that affect extraction quality.
///
/// Responsive source selection, figure recognition, and heading semantics stay
/// in source form until semantic compilation.
pub(crate) fn cleanup_selected_content(
    dom: &mut Dom,
    root: NodeId,
    nodes: &mut Vec<NodeId>,
    flatten_javascript_links: bool,
) {
    images::remove_duplicates(dom, root, nodes);
    headings::remove_artifacts(dom, root);
    if flatten_javascript_links {
        flatten_javascript_links_for_quality(dom, root);
    }
}

/// Preserves the established DOM-based link-density metric until result metrics use the IR.
pub(crate) fn flatten_javascript_links_for_quality(dom: &mut Dom, root: NodeId) {
    let links: Vec<_> = dom
        .descendants(root)
        .filter(|&node| {
            dom.tag(node) == Some(Tag::A)
                && dom
                    .attr(node, AttrName::Href)
                    .is_some_and(|href| href.starts_with("javascript:"))
        })
        .collect();
    for link in links {
        let replacement = if dom.first_child(link) == dom.last_child(link)
            && dom
                .first_child(link)
                .is_some_and(|child| dom.is_text(child))
        {
            dom.create_text(&dom.text(link))
        } else {
            dom.create_html_element(Tag::Span).inspect(|&span| {
                dom.move_children(link, span);
            })
        };
        if let Ok(replacement) = replacement {
            dom.replace_with(link, replacement);
        }
    }
}

/// Protects meaningful media from hard cleanup.
pub(crate) fn prepare_media_before_cleanup(dom: &mut Dom, root: NodeId) {
    media::prepare(dom, root);
}

/// Removes SVG implementation details and replaces accessible charts before scoring.
pub(crate) fn normalize_svg_before_scoring(dom: &mut Dom, root: NodeId) {
    svg::normalize(dom, root);
}

/// Removes decorative media while source sizing and naming evidence is intact.
pub(crate) fn remove_decorative_media_before_cleanup(dom: &mut Dom, root: NodeId) {
    images::remove_decorative_media(dom, root);
}

pub(crate) fn adjacent_lead_media(dom: &Dom, root: NodeId) -> Option<NodeId> {
    images::adjacent_lead_media(dom, root)
}

pub(crate) fn adopt_external_footnotes(
    definitions: &crate::document::ExternalFootnoteDefinitions,
    fragment: &mut Dom,
    fragment_root: NodeId,
) {
    crate::document::adopt_external_footnotes(definitions, fragment, fragment_root);
}

pub(crate) fn collect_external_footnotes(
    dom: &Dom,
) -> crate::document::ExternalFootnoteDefinitions {
    crate::document::collect_external_footnotes(dom)
}

/// Preserves explicit ARIA document structure in the scoring-only DOM.
///
/// Readability preparation can turn leaf `div` elements into paragraphs. Run
/// the same heading and list passes first so that operation cannot erase
/// author-provided semantics. The retained fragment runs the full pipeline.
pub(crate) fn normalize_scoring_structure(dom: &mut Dom, root: NodeId) {
    headings::normalize_roles(dom, root);
    lists::normalize_for_scoring(dom, root);
}

pub(crate) fn has_primary_heading_semantics(dom: &Dom, node: NodeId) -> bool {
    matches!(dom.tag(node), Some(Tag::H1 | Tag::H2)) || headings::has_primary_role(dom, node)
}

pub(crate) use crate::document::accessible_math_nodes;

/// Removes empty retained blocks after semantic source protection is complete.
#[cfg(test)]
pub(crate) fn remove_empty_content(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    let mut source_facts = Some(crate::document::SemanticSourceFacts::analyze(dom, root));
    let source_evidence =
        crate::document::SourceEvidence::analyze(dom, root, &crate::dom::NodeStateStore::new());
    remove_empty_content_with_source_facts(dom, root, nodes, &mut source_facts, &source_evidence);
}

/// Removes empty blocks while updating facts shared with semantic compilation.
pub(crate) fn remove_empty_content_with_source_facts(
    dom: &mut Dom,
    root: NodeId,
    nodes: &mut Vec<NodeId>,
    source_facts: &mut Option<crate::document::SemanticSourceFacts>,
    source_evidence: &crate::document::SourceEvidence,
) -> Option<crate::document::RetainedStream> {
    remove_empty_nodes(dom, root, nodes, source_facts, source_evidence)
}

fn has_visible_heading_content(dom: &Dom, heading: NodeId) -> bool {
    std::iter::once(heading)
        .chain(dom.descendants(heading))
        .any(|node| {
            dom.text_node(node)
                .is_some_and(crate::render::markdown::has_visible_inline_text)
                || dom.tag(node) == Some(Tag::Img)
                    && (dom
                        .attr_by_local_name(node, "alt")
                        .is_some_and(crate::render::markdown::has_visible_inline_text)
                        || dom
                            .attr(node, AttrName::Src)
                            .is_some_and(|source| !source.trim().is_empty()))
        })
}

fn remove_empty_nodes(
    dom: &mut Dom,
    root: NodeId,
    nodes: &mut Vec<NodeId>,
    source_facts: &mut Option<crate::document::SemanticSourceFacts>,
    source_evidence: &crate::document::SourceEvidence,
) -> Option<crate::document::RetainedStream> {
    // A source-fact snapshot already owns the source order used by complex
    // lowering. Build a second, complete stream only for the ordinary path,
    // where it can replace the compiler's DOM traversal. Source facts may be
    // created below for code-specific cleanup, so capture this decision first.
    let build_retained_stream = source_facts.is_none();
    if let Some(source_facts) = source_facts.as_ref() {
        nodes.clear();
        nodes.extend(source_facts.nodes().iter().copied());
    } else {
        nodes.clear();
        nodes.extend(dom.descendants(root));
    }

    // Build the retained stream before detach operations. It carries the
    // source depth required by ordinary lowering, so final cleanup does not
    // need a DOM-sized position index or a second topology reconstruction.
    let mut retained_stream = build_retained_stream
        .then(|| crate::document::RetainedStream::from_preorder(dom, root, nodes));

    // Whitespace-only syntax token elements contain significant code text.
    // Record code ancestry in one preorder pass so empty-node cleanup does not
    // need an ancestor scan for each element. Shared source facts avoid
    // recomputing this evidence in the semantic compiler.
    let mut legacy_multiline = source_facts
        .is_none()
        .then(|| crate::document::code_multiline_content(dom, nodes));
    let mut legacy_code_blocks = source_facts.is_none().then(|| vec![false; dom.len()]);
    let mut share_source_facts = false;
    let mut in_preformatted_code = vec![false; dom.len()];
    let source_facts_were_provided = source_facts.is_some();
    if let (Some(multiline), Some(code_blocks)) =
        (legacy_multiline.as_ref(), legacy_code_blocks.as_mut())
    {
        for &node in nodes.iter() {
            let multiline = multiline[node.index()];
            code_blocks[node.index()] =
                crate::document::is_multiline_code_with_evidence(dom, node, multiline);
            share_source_facts |= matches!(dom.tag(node), Some(Tag::Code | Tag::Pre)) && multiline;
            in_preformatted_code[node.index()] = dom.parent(node).is_some_and(|parent| {
                dom.tag(parent) == Some(Tag::Pre)
                    || code_blocks[parent.index()]
                    || in_preformatted_code[parent.index()]
            });
        }
    }
    if share_source_facts
        && let (Some(multiline), Some(code_blocks)) =
            (legacy_multiline.take(), legacy_code_blocks.take())
    {
        *source_facts = Some(crate::document::SemanticSourceFacts::from_precomputed(
            dom,
            root,
            nodes,
            multiline,
            code_blocks,
        ));
    }
    if source_facts_were_provided || share_source_facts {
        for &node in nodes.iter() {
            in_preformatted_code[node.index()] = dom.parent(node).is_some_and(|parent| {
                let parent_is_code = source_facts.as_ref().is_some_and(|source_facts| {
                    source_facts.is_code_block(parent)
                        || dom.tag(parent) == Some(Tag::Code)
                            && source_facts.multiline_content(parent)
                });
                dom.tag(parent) == Some(Tag::Pre)
                    || parent_is_code
                    || in_preformatted_code[parent.index()]
            });
        }
    }
    let mut has_text = vec![false; dom.len()];
    for &node in nodes.iter().rev() {
        has_text[node.index()] |= dom.text_node(node).is_some_and(|text| !text.is_empty());
        if has_text[node.index()]
            && let Some(parent) = dom.parent(node)
        {
            has_text[parent.index()] = true;
        }
    }

    for (position, &node) in nodes.iter().enumerate().rev() {
        let significant_code_whitespace =
            in_preformatted_code[node.index()] && has_text[node.index()];
        if dom.parent(node).is_some()
            && !significant_code_whitespace
            && !source_evidence.math(node)
            && !source_evidence.accessible_math(node)
            && matches!(
                dom.tag(node),
                Some(
                    Tag::Div
                        | Tag::Section
                        | Tag::P
                        | Tag::Span
                        | Tag::Aside
                        | Tag::Footer
                        | Tag::Header
                        | Tag::H1
                        | Tag::H2
                        | Tag::H3
                        | Tag::H4
                        | Tag::H5
                        | Tag::H6
                )
            )
            && (is_element_without_content(dom, node)
                || matches!(
                    dom.tag(node),
                    Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
                ) && !has_visible_heading_content(dom, node))
        {
            let parent = dom.parent(node);
            if let Some(retained_stream) = retained_stream.as_mut() {
                retained_stream.mark_removed(position);
            }
            dom.detach(node);
            if let Some(source_facts) = source_facts.as_mut() {
                source_facts.node_detached(node, parent);
            }
        }
    }
    if let Some(source_facts) = source_facts.as_mut() {
        source_facts.refresh_after_cleanup(dom);
    }

    retained_stream.take().map(|mut stream| {
        stream.compact_removed();
        stream
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn semantic_markdown(dom: &Dom, root: NodeId) -> String {
        let document = crate::document::compile_document(
            dom,
            root,
            &crate::document::CompileContext::default(),
        )
        .unwrap();
        crate::render::markdown::render_markdown(
            &document,
            0,
            crate::render::markdown::MarkdownConfig::default(),
        )
    }

    fn normalized(html: &str) -> (Dom, NodeId) {
        let mut dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        let mut nodes = Vec::new();
        normalize_semantics(&mut dom, root, &mut nodes);
        remove_empty_content(&mut dom, root, &mut nodes);
        (dom, root)
    }

    #[test]
    fn final_cleanup_stream_excludes_detached_subtrees() {
        let mut dom = Dom::parse_fragment(
            "<div><p>Keep this paragraph.</p><div></div><p>Keep this too.</p></div>",
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let mut nodes: Vec<_> = dom.descendants(root).collect();
        let empty = nodes
            .iter()
            .copied()
            .find(|&node| dom.tag(node) == Some(Tag::Div) && dom.first_child(node).is_none())
            .expect("fixture has an empty retained wrapper");
        let mut source_facts = None;
        let retained = remove_empty_content_with_source_facts(
            &mut dom,
            root,
            &mut nodes,
            &mut source_facts,
            &crate::document::SourceEvidence::default(),
        )
        .expect("ordinary cleanup should produce a retained source stream");
        assert!(!retained.contains(&empty));
        assert!(retained.iter().all(|&node| dom.parent(node).is_some()));

        let streamed = crate::document::compile_document_with_optional_source_facts_and_evidence_and_retained_nodes(
            &dom,
            root,
            &crate::document::CompileContext::default(),
            None,
            None,
            Some(&retained),
        )
        .unwrap();
        let direct = crate::document::compile_document(
            &dom,
            root,
            &crate::document::CompileContext::default(),
        )
        .unwrap();
        assert_eq!(streamed.debug_tape(), direct.debug_tape());
    }

    #[test]
    fn flattens_nested_identical_formatting() {
        let (dom, root) = normalized(
            "<p><strong>Listen to this <strong>post</strong>:</strong> <em>one <i>two</i></em></p>",
        );
        assert_eq!(
            semantic_markdown(&dom, root),
            "**Listen to this post:** *one two*\n"
        );
    }

    #[test]
    fn replaces_orphan_placeholders_with_described_image_text() {
        let (dom, root) = normalized(
            r#"<img src="grey-placeholder.png"><img src="blank.gif" aria-label="image unavailable"><img src="placeholder.png" alt="A meaningful diagram"><img src="real.jpg" alt="Photo">"#,
        );
        assert_eq!(
            semantic_markdown(&dom, root),
            "A meaningful diagram![Photo](real.jpg)\n"
        );
    }

    #[test]
    fn figure_caption_does_not_preserve_an_extra_placeholder() {
        let (dom, root) = normalized(
            r#"<figure><img src="blank.gif"><img src="photo.jpg" alt="Photo"><figcaption>A useful caption</figcaption></figure>"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            1
        );
    }

    #[test]
    fn placeholder_tokens_outside_known_basenames_do_not_remove_images() {
        let (dom, root) = normalized(
            r#"<img src="/photos/placeholder-design.jpg"><img src="/image.jpg?placeholder=true"><img src="https://placeholder.example/image.jpg">"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            3
        );
    }

    #[test]
    fn normalizes_highlighted_code_and_language() {
        let (dom, root) = normalized(
            r#"<div class="highlight"><pre class="highlight language-rust"><span>fn main() {</span>
<span>}</span></pre></div>"#,
        );
        assert_eq!(
            semantic_markdown(&dom, root),
            "```rust\nfn main() {\n}\n```\n"
        );
    }

    #[test]
    fn keeps_presentation_code_tables_for_semantic_compilation() {
        let (dom, root) = normalized(
            r#"<table role="presentation" class="highlighttable"><tr><td class="linenos"><pre>1</pre></td><td><pre class="language-rust"><code>fn main() {}</code></pre></td></tr></table>"#,
        );
        assert_eq!(
            semantic_markdown(&dom, root),
            "```rust\nfn main() {}\n```\n"
        );
    }

    #[test]
    fn keeps_language_classes_on_inline_code_inline() {
        let (dom, root) = normalized(
            r#"<p>Use <code class="language-plaintext"><span>cargo test</span></code> now.</p><table><tr><th>Call</th></tr><tr><td><code class="language-rust">run()</code></td></tr></table>"#,
        );
        let markdown = semantic_markdown(&dom, root);
        assert!(markdown.contains("Use `cargo test` now."));
        assert!(
            markdown.contains("| Call |\n| --- |\n| `run()` |"),
            "{markdown}"
        );
        assert!(!markdown.contains("```plaintext"));
    }

    #[test]
    fn promotes_multiline_orphan_code_and_finds_wrapper_language() {
        let (dom, root) = normalized(
            r#"<code><span>first
second</span></code><div class="language-rust"><div class="highlight"><pre><code><span>fn main() {}</span></code></pre></div></div>"#,
        );
        let markdown = semantic_markdown(&dom, root);
        assert!(markdown.contains("```\nfirst\nsecond\n```"), "{markdown}");
        assert!(
            markdown.contains("```rust\nfn main() {}\n```"),
            "{markdown}"
        );
    }

    #[test]
    fn removes_headings_without_visible_content() {
        let (dom, root) = normalized(
            "<h2> </h2><h2>&nbsp;<br></h2><h2>\u{200b}\u{2060}\u{feff}</h2><h2>Visible\u{200b}</h2><h2><img src='x' alt='Diagram'></h2><p>Text.</p>",
        );
        let markdown = semantic_markdown(&dom, root);
        assert_eq!(
            markdown,
            "## Visible\u{200b}\n\n## ![Diagram](x)\n\nText.\n"
        );
    }

    #[test]
    fn keeps_an_image_only_heading_without_alt_text() {
        let (dom, root) = normalized(r#"<h2><img src="diagram.png"></h2><p>Text.</p>"#);
        assert!(
            dom.descendants(root)
                .any(|node| dom.tag(node) == Some(Tag::H2))
        );
        assert_eq!(semantic_markdown(&dom, root), "![](diagram.png)\n\nText.\n");
    }

    #[test]
    fn does_not_create_nested_figures() {
        for html in [
            r#"<div class="image-with-caption"><figure><img src="plot.png"><figcaption>Plot</figcaption></figure></div>"#,
            r#"<div class="image-with-caption"><div class="image-with-caption"><img src="plot.png"><p class="caption">Plot</p></div></div>"#,
        ] {
            let (dom, root) = normalized(html);
            let document = crate::document::compile_document(
                &dom,
                root,
                &crate::document::CompileContext::default(),
            )
            .unwrap();
            assert_eq!(
                document
                    .debug_tape()
                    .lines()
                    .filter(|line| line.trim() == "Figure")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn normalizes_captioned_image_wrapper() {
        let (dom, root) = normalized(
            r#"<div class="image-with-caption"><div><img src="plot.png" alt="Plot"><p class="caption">Result plot</p></div></div>"#,
        );
        let document = crate::document::compile_document(
            &dom,
            root,
            &crate::document::CompileContext::default(),
        )
        .unwrap();
        assert!(document.debug_tape().starts_with("Figure\n"));
        assert!(document.debug_tape().contains("  Figcaption\n"));
        assert_eq!(
            semantic_markdown(&dom, root),
            "![Plot](plot.png)\n\nResult plot\n"
        );
    }

    #[test]
    fn uses_srcset_when_src_is_missing() {
        let (dom, root) = normalized(r#"<img srcset="small.jpg 1x, large.jpg 2x" alt="Photo">"#);
        assert_eq!(semantic_markdown(&dom, root), "![Photo](large.jpg)\n");
    }

    #[test]
    fn uses_a_lazy_picture_source_for_markdown() {
        let (dom, root) = normalized(
            r#"<picture><source><source data-srcset="hero.webp 1x, hero-large.webp 2x"><img src="blank.gif" alt="Hero"></picture>"#,
        );
        assert_eq!(semantic_markdown(&dom, root), "![Hero](hero-large.webp)\n");

        let (dom, root) = normalized(
            r#"<picture><source data-src="null"><source data-src="hero.jpg"><img src="placeholder.gif" alt="Hero"></picture>"#,
        );
        assert_eq!(semantic_markdown(&dom, root), "![Hero](hero.jpg)\n");

        let (dom, root) = normalized(
            r#"<picture data-src="parent.jpg"><img width="1" src="blank.gif" alt="Parent"></picture>"#,
        );
        assert_eq!(semantic_markdown(&dom, root), "![Parent](parent.jpg)\n");
    }

    #[test]
    fn converts_ranked_listing_tables_but_keeps_data_tables() {
        let (dom, root) = normalized(
            r#"<table><tr><td>31.</td><td><a href='vote?how=up'></a></td><td><a href='/one'>First result</a> <a href='/one'><img src='one.jpg' alt='Preview'></a></td></tr><tr><td></td><td></td><td>10 points | <a href='hide?id=1'>hide</a> | <a href='/one/comments'>2 comments</a></td></tr><tr><td colspan='3'></td></tr><tr><td>32.</td><td><a href='vote?how=up'></a></td><td><a href='/two'>Second result</a></td></tr><tr><td></td><td></td><td>20 points | <a href='hide?id=2'>hide</a></td></tr><tr><td colspan='3'></td></tr><tr><td>33.</td><td></td><td><a href='/three'>Third result</a></td></tr><tr><td></td><td></td><td>30 points</td></tr><tr><td colspan='3'></td></tr></table><table><thead><tr><th>Name</th><th>Value</th></tr></thead><tbody><tr><td>A</td><td>1</td></tr></tbody></table><table><tr><td>1.</td><td><a href='/team/a'>Team A</a></td><td>30</td></tr><tr><td>2.</td><td><a href='/team/b'>Team B</a></td><td>28</td></tr><tr><td>3.</td><td><a href='/team/c'>Team C</a></td><td>25</td></tr><tr><td>4.</td><td><a href='/team/d'>Team D</a></td><td>22</td></tr><tr><td>5.</td><td><a href='/team/e'>Team E</a></td><td>20</td></tr><tr><td>6.</td><td><a href='/team/f'>Team F</a></td><td>18</td></tr></table>"#,
        );
        let document = crate::document::compile_document(
            &dom,
            root,
            &crate::document::CompileContext::default(),
        )
        .unwrap();
        let tree = document.debug_tape();
        assert_eq!(
            tree.lines()
                .filter(|line| line.starts_with("List("))
                .count(),
            1
        );
        assert_eq!(
            tree.lines()
                .filter(|line| line.trim() == "ListItem")
                .count(),
            3
        );
        assert_eq!(
            tree.lines()
                .filter(|line| line.starts_with("Table("))
                .count(),
            2
        );
        assert_eq!(
            tree.lines()
                .filter(|line| line.trim_start().starts_with("Image("))
                .count(),
            1
        );
        assert!(tree.contains("List(kind=Ordered, start=Some(31))"));
        let markdown = semantic_markdown(&dom, root);
        assert!(markdown.starts_with("31. "));
        assert!(!markdown.contains("hide"));
        assert!(!markdown.contains(" |  | "));
        assert!(markdown.find("First result").unwrap() < markdown.find("Third result").unwrap());
    }

    #[test]
    fn normalizes_an_outer_listing_with_a_nested_data_table() {
        let (dom, root) = normalized(
            r#"<table><tr><td>1.</td><td><a href='/one'>One</a><table><thead><tr><th>Field</th><th>Value</th></tr></thead><tbody><tr><td>A</td><td>B</td></tr></tbody></table></td></tr><tr><td></td><td>First metadata</td></tr><tr><td>2.</td><td><a href='/two'>Two</a></td></tr><tr><td></td><td>Second metadata</td></tr><tr><td>3.</td><td><a href='/three'>Three</a></td></tr><tr><td></td><td>Third metadata</td></tr><tr><td>4.</td><td><a href='/four'>Four</a></td></tr><tr><td></td><td>Fourth metadata</td></tr><tr><td>5.</td><td><a href='/five'>Five</a></td></tr><tr><td></td><td>Fifth metadata</td></tr><tr><td>6.</td><td><a href='/six'>Six</a></td></tr><tr><td></td><td>Sixth metadata</td></tr></table>"#,
        );
        let document = crate::document::compile_document(
            &dom,
            root,
            &crate::document::CompileContext::default(),
        )
        .unwrap();
        let tree = document.debug_tape();
        assert_eq!(
            tree.lines()
                .filter(|line| line.starts_with("List("))
                .count(),
            1
        );
        assert_eq!(
            tree.lines()
                .filter(|line| line.trim_start().starts_with("Table("))
                .count(),
            1
        );
        let markdown = semantic_markdown(&dom, root);
        assert!(markdown.contains("One"));
        assert!(markdown.contains("| Field | Value |"), "{markdown}");
    }

    #[test]
    fn preserves_heading_levels_and_footnotes() {
        let (dom, root) = normalized(
            r##"<h1>Guide</h1><p>Text<a href="#note">[1]</a></p><aside id="note" role="doc-footnote">A reference.</aside>"##,
        );
        assert_eq!(
            semantic_markdown(&dom, root),
            "# Guide\n\nText[^note]\n\n[^note]: A reference.\n"
        );
    }
}
