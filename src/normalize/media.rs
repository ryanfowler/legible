use crate::dom::{Dom, NodeId, Tag};
use crate::scoring::{has_hidden_utility_class, has_static_hidden_marker};

/// Removes source media that must not reach cleanup or semantic compilation.
///
/// Meaningful media stays in source form. The document compiler interprets it
/// after content selection. This pass only applies visibility and active
/// object/embed policy while the source evidence is available.
#[cfg(test)]
pub(super) fn prepare(dom: &mut Dom, root: NodeId) {
    let nodes: Vec<_> = std::iter::once(root).chain(dom.descendants(root)).collect();
    let mut scratch = crate::cleaning::FragmentScratch {
        u32_values: Vec::new(),
        bytes: Vec::new(),
        bits: Vec::new(),
    };
    prepare_with_source_nodes(dom, root, &nodes, &mut scratch);
}

pub(super) fn prepare_with_source_nodes(
    dom: &mut Dom,
    _root: NodeId,
    nodes: &[NodeId],
    scratch: &mut crate::cleaning::FragmentScratch,
) {
    scratch.bits.resize(dom.len(), false);
    scratch.bits.fill(false);
    scratch.u32_values.resize(dom.len(), u32::MAX);
    scratch.u32_values.fill(u32::MAX);
    crate::document::media_cleanup_evidence_into(
        dom,
        nodes,
        &mut scratch.bits,
        &mut scratch.u32_values,
    );
    for &node in nodes.iter().rev() {
        if dom.parent(node).is_none() || !dom.is_element(node) {
            continue;
        }
        match dom.tag(node) {
            Some(Tag::Object | Tag::Embed) => dom.detach(node),
            Some(Tag::Iframe | Tag::Video | Tag::Audio) if is_statically_hidden(dom, node) => {
                dom.detach(node)
            }
            Some(Tag::Iframe | Tag::Video | Tag::Audio) if !scratch.bits[node.index()] => {
                let fallback = scratch.u32_values[node.index()];
                if fallback != u32::MAX {
                    dom.insert_before(node, NodeId(fallback));
                }
                dom.detach(node);
            }
            _ => {}
        }
    }
}

fn is_statically_hidden(dom: &Dom, node: NodeId) -> bool {
    has_static_hidden_marker(dom, node)
        || has_hidden_utility_class(dom, node)
        || dom.attr(node, crate::dom::AttrName::AriaHidden) == Some("true")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{CompileContext, CompileInputs, compile_document};
    use crate::render::markdown::{MarkdownConfig, render_markdown};

    fn markdown(html: &str) -> String {
        let mut dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        prepare(&mut dom, root);
        let document = compile_document(
            &dom,
            root,
            &CompileContext::default(),
            &CompileInputs::default(),
        )
        .unwrap();
        render_markdown(&document, 0, MarkdownConfig::default())
    }

    #[test]
    fn compiler_converts_source_media_directly() {
        assert_eq!(
            markdown(
                r#"<iframe src="https://www.youtube-nocookie.com/embed/abc?rel=0"></iframe><span><video><source src="movie.mp4"></video></span><span><audio src="sound.mp3"></audio></span><iframe src="/frame"></iframe>"#
            ),
            "[YouTube video](https://www.youtube.com/watch?v=abc) [Video](movie.mp4) [Audio](sound.mp3) [Embedded content](/frame)\n"
        );
    }

    #[test]
    fn compiler_preserves_word_boundaries_around_media() {
        assert_eq!(
            markdown(r#"<p>Watch<span><video src="movie.mp4"></video></span>now</p>"#),
            "Watch [Video](movie.mp4) now\n"
        );
    }

    #[test]
    fn compiler_uses_labels_and_keeps_safe_fallback_links() {
        let output = markdown(
            r#"<figure><iframe src="https://video.example.test/embed/42" title="Interview with the bridge engineer">Opaque player chrome</iframe><figcaption><a href="/transcript">Read the complete interview transcript</a></figcaption></figure><video src="movie.mp4"><a href="/notes">Read the notes</a></video>"#,
        );
        assert!(
            output.contains(
                "[Interview with the bridge engineer](https://video.example.test/embed/42)"
            )
        );
        assert!(output.contains("[Read the complete interview transcript](/transcript)"));
        assert!(output.contains("[Video](movie.mp4) [Read the notes](/notes)"));
        assert!(!output.contains("Opaque player chrome"));
    }

    #[test]
    fn removes_hidden_active_media_and_omits_unsafe_sources() {
        assert_eq!(
            markdown(
                r#"<iframe hidden src="https://example.test/frame"></iframe><video aria-hidden="true" src="movie.mp4"></video><iframe src="javascript:bad()"></iframe><audio src="data:text/html,bad"></audio><object data="movie.swf"></object>"#
            ),
            ""
        );
    }

    #[test]
    fn removes_object_and_embed_without_other_media() {
        assert_eq!(
            markdown(r#"<object data="movie.swf"></object><embed src="movie.swf">"#),
            ""
        );
    }

    #[test]
    fn compiler_skips_unsafe_sources_and_deceptive_youtube_hosts() {
        assert_eq!(
            markdown(
                r#"<iframe src="javascript:bad" data-src="/frame"></iframe><video src="javascript:bad"><source src="javascript:bad" data-src="movie.mp4"></video><audio src="null" data-src="sound.mp3"></audio><iframe src="https://notyoutube.com/embed/id"></iframe>"#
            ),
            "[Embedded content](/frame) [Video](movie.mp4) [Audio](sound.mp3) [Embedded content](https://notyoutube.com/embed/id)\n"
        );
    }
}
