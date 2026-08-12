use crate::dom::{AttrName, Dom, NodeId, Tag};

pub(super) fn normalize(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    for (node, _) in nodes {
        if dom.parent(node).is_none() || dom.attr(node, AttrName::DataMath).is_some() {
            continue;
        }
        let Some(latex) = explicit_latex(dom, node) else {
            continue;
        };
        let block = is_block_math(dom, node);
        let fallback = math_fallback(dom, node).unwrap_or_else(|| latex.clone());
        let Ok(canonical) = dom.create_html_element(if block { Tag::Div } else { Tag::Span })
        else {
            continue;
        };
        dom.set_attr(
            canonical,
            AttrName::DataMath,
            if block { "block" } else { "inline" },
        );
        dom.set_attr(canonical, AttrName::DataLatex, latex.trim());
        if let Ok(text) = dom.create_text(&fallback) {
            dom.append_child(canonical, text);
        }
        dom.replace_with(node, canonical);
    }
}

fn math_fallback(dom: &Dom, node: NodeId) -> Option<String> {
    let math = if dom.tag(node) == Some(Tag::Math) {
        node
    } else {
        dom.descendants(node)
            .find(|&descendant| dom.tag(descendant) == Some(Tag::Math))?
    };
    let mut fallback = String::new();
    for descendant in std::iter::once(math).chain(dom.descendants(math)) {
        if dom.text_node(descendant).is_none()
            || dom.ancestors(descendant).any(|ancestor| {
                dom.qual_name(ancestor)
                    .is_some_and(|name| is_annotation_element(name.local.as_ref()))
            })
        {
            continue;
        }
        let text = dom.text_node(descendant).unwrap_or_default().trim();
        if text.is_empty() {
            continue;
        }
        if !fallback.is_empty() {
            fallback.push(' ');
        }
        fallback.push_str(text);
    }
    (!fallback.is_empty()).then_some(fallback)
}

fn is_annotation_element(local: &str) -> bool {
    local.eq_ignore_ascii_case("annotation") || local.eq_ignore_ascii_case("annotation-xml")
}

/// Marks elements that contain a usable TeX annotation and are math or inside
/// a known math wrapper. The previous per-element check walked ancestors and
/// descendants repeatedly. This reverse/preorder pair keeps the pass linear.
pub(crate) fn accessible_math_nodes(dom: &Dom, nodes: &[(NodeId, u32)]) -> Vec<bool> {
    let mut has_annotation = vec![false; dom.len()];
    for &(node, _) in nodes.iter().rev() {
        let own_annotation = is_tex_annotation(dom, node);
        let descendant_annotation = dom
            .element_children(node)
            .any(|child| has_annotation[child.index()]);
        has_annotation[node.index()] = own_annotation || descendant_annotation;
    }

    let mut inside_wrapper = vec![false; dom.len()];
    let mut accessible = vec![false; dom.len()];
    for &(node, _) in nodes {
        let inherited = dom
            .parent(node)
            .is_some_and(|parent| inside_wrapper[parent.index()]);
        let wrapper = inherited || has_math_wrapper_class(dom, node);
        inside_wrapper[node.index()] = wrapper;
        accessible[node.index()] =
            has_annotation[node.index()] && (dom.tag(node) == Some(Tag::Math) || wrapper);
    }
    accessible
}

fn is_tex_annotation(dom: &Dom, node: NodeId) -> bool {
    dom.qual_name(node)
        .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("annotation"))
        && dom
            .attr_by_local_name(node, "encoding")
            .is_some_and(is_tex_encoding)
}

fn is_tex_encoding(value: &str) -> bool {
    value.eq_ignore_ascii_case("application/x-tex")
        || value.eq_ignore_ascii_case("application/x-latex")
        || value.eq_ignore_ascii_case("text/tex")
}

fn explicit_latex(dom: &Dom, node: NodeId) -> Option<String> {
    for name in ["data-latex", "data-tex"] {
        if let Some(value) = dom
            .attr_by_local_name(node, name)
            .filter(|value| valid_latex(value))
        {
            return Some(value.trim().to_owned());
        }
    }
    for name in ["data-math", "data-formula"] {
        if let Some(value) = dom
            .attr_by_local_name(node, name)
            .filter(|value| looks_like_latex(value) || has_math_wrapper_class(dom, node))
        {
            return Some(value.trim().to_owned());
        }
    }

    if dom.tag(node) == Some(Tag::Script)
        && dom
            .attr(node, AttrName::Type)
            .is_some_and(is_math_script_type)
    {
        let value = dom.text(node);
        return valid_latex(&value).then(|| value.trim().to_owned());
    }

    if dom.tag(node) == Some(Tag::Img)
        && image_is_equation(dom, node)
        && let Some(value) = dom
            .attr_by_local_name(node, "alt")
            .filter(|value| looks_like_latex(value))
    {
        return Some(value.trim().to_owned());
    }

    let math_root = dom.tag(node) == Some(Tag::Math) || has_math_wrapper_class(dom, node);
    if !math_root {
        return None;
    }
    let annotated = std::iter::once(node)
        .chain(dom.descendants(node))
        .find(|&descendant| {
            dom.qual_name(descendant)
                .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("annotation"))
                && dom
                    .attr_by_local_name(descendant, "encoding")
                    .is_some_and(|encoding| {
                        matches!(
                            encoding.to_ascii_lowercase().as_str(),
                            "application/x-tex" | "application/x-latex" | "text/tex"
                        )
                    })
        })
        .map(|annotation| dom.text(annotation))
        .filter(|value| valid_latex(value))
        .map(|value| value.trim().to_owned());
    annotated.or_else(|| {
        (dom.tag(node) == Some(Tag::Math))
            .then(|| mathml_latex(dom, node))
            .flatten()
    })
}

fn image_is_equation(dom: &Dom, node: NodeId) -> bool {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|name| dom.attr(node, name))
        .flat_map(str::split_whitespace)
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "math" | "equation" | "formula" | "latex"
            )
        })
        || dom.attr(node, AttrName::Src).is_some_and(|source| {
            let name = source
                .split(['?', '#'])
                .next()
                .unwrap_or(source)
                .rsplit('/')
                .next()
                .unwrap_or(source)
                .to_ascii_lowercase();
            name.contains("equation") || name.contains("formula")
        })
}

fn looks_like_latex(value: &str) -> bool {
    valid_latex(value)
        && value
            .bytes()
            .any(|byte| matches!(byte, b'\\' | b'^' | b'_' | b'=' | b'{' | b'}'))
}

#[derive(Clone, Copy)]
enum MathMlTask {
    Node(NodeId),
    Literal(&'static str),
}

fn mathml_latex(dom: &Dom, root: NodeId) -> Option<String> {
    let mut output = String::new();
    let mut tasks = vec![MathMlTask::Node(root)];
    while let Some(task) = tasks.pop() {
        match task {
            MathMlTask::Literal(value) => output.push_str(value),
            MathMlTask::Node(node) => {
                if let Some(text) = dom.text_node(node) {
                    output.push_str(text.trim());
                    continue;
                }
                let local = dom
                    .qual_name(node)
                    .map(|name| name.local.as_ref())
                    .unwrap_or("");
                let children: Vec<NodeId> = dom.children(node).collect();
                match (local, children.as_slice()) {
                    ("semantics", [presentation, ..]) => {
                        tasks.push(MathMlTask::Node(*presentation));
                    }
                    ("annotation" | "annotation-xml", _) => {}
                    ("msup", [base, exponent, ..]) => {
                        tasks.push(MathMlTask::Literal("}"));
                        tasks.push(MathMlTask::Node(*exponent));
                        tasks.push(MathMlTask::Literal("^{"));
                        tasks.push(MathMlTask::Node(*base));
                    }
                    ("msub", [base, subscript, ..]) => {
                        tasks.push(MathMlTask::Literal("}"));
                        tasks.push(MathMlTask::Node(*subscript));
                        tasks.push(MathMlTask::Literal("_{"));
                        tasks.push(MathMlTask::Node(*base));
                    }
                    ("mfrac", [numerator, denominator, ..]) => {
                        tasks.push(MathMlTask::Literal("}"));
                        tasks.push(MathMlTask::Node(*denominator));
                        tasks.push(MathMlTask::Literal("}{"));
                        tasks.push(MathMlTask::Node(*numerator));
                        tasks.push(MathMlTask::Literal("\\frac{"));
                    }
                    ("msqrt", _) => {
                        tasks.push(MathMlTask::Literal("}"));
                        for child in children.into_iter().rev() {
                            tasks.push(MathMlTask::Node(child));
                        }
                        tasks.push(MathMlTask::Literal("\\sqrt{"));
                    }
                    _ => {
                        for child in children.into_iter().rev() {
                            tasks.push(MathMlTask::Node(child));
                        }
                    }
                }
            }
        }
        if output.len() > 64 * 1024 {
            return None;
        }
    }
    valid_latex(&output).then_some(output)
}

fn valid_latex(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 64 * 1024
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

fn is_math_script_type(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value == "math/tex" || value.starts_with("math/tex;") || value == "text/tex"
}

fn has_math_wrapper_class(dom: &Dom, node: NodeId) -> bool {
    dom.attr(node, AttrName::Class).is_some_and(|classes| {
        classes.split_whitespace().any(|class| {
            matches!(
                class.to_ascii_lowercase().as_str(),
                "katex" | "katex-display" | "mathjax" | "mathjax-display" | "tex2jax_process"
            )
        })
    })
}

fn is_block_math(dom: &Dom, node: NodeId) -> bool {
    if std::iter::once(node)
        .chain(dom.descendants(node))
        .any(|math| {
            dom.tag(math) == Some(Tag::Math)
                && dom
                    .attr_by_local_name(math, "display")
                    .is_some_and(|value| value.eq_ignore_ascii_case("block"))
        })
    {
        return true;
    }
    if dom.tag(node) == Some(Tag::Script)
        && dom
            .attr(node, AttrName::Type)
            .is_some_and(|value| value.to_ascii_lowercase().contains("mode=display"))
    {
        return true;
    }
    if dom.attr(node, AttrName::Class).is_some_and(|classes| {
        classes.split_whitespace().any(|class| {
            matches!(
                class.to_ascii_lowercase().as_str(),
                "katex-display" | "math-display" | "mathjax-display"
            )
        })
    }) {
        return true;
    }
    matches!(dom.tag(node), Some(Tag::Div | Tag::P))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::dom_to_markdown;
    use crate::text::{TextOptions, render_text};

    #[test]
    fn extracts_katex_annotation_once() {
        let mut dom = Dom::parse_fragment(r#"<p>A <span class="katex"><math><semantics><annotation encoding="application/x-tex">E=mc^2</annotation></semantics></math><span class="katex-html">duplicate</span></span>.</p>"#, Tag::Div).unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "A $E=mc^2$.\n");
    }

    #[test]
    fn converts_common_mathml_without_a_dependency() {
        let mut dom = Dom::parse_fragment(
            "<math><mi>E</mi><mo>=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></math>",
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "$E=mc^{2}$\n");
    }

    #[test]
    fn converts_only_images_with_equation_evidence() {
        let mut dom = Dom::parse_fragment(
            r#"<img class="equation" src="eq.png" alt="E=mc^2"><img src="photo.png" alt="x=y">"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "$E=mc^2$![x=y](photo.png)\n"
        );
    }

    #[test]
    fn canonical_math_has_one_html_and_text_fallback() {
        let mut dom = Dom::parse_fragment(
            r#"<script type="math/tex">x=1</script><span data-latex="y=2"></span><span class="katex"><math><semantics><mrow><mi>z</mi><mo>=</mo><mn>3</mn></mrow><annotation encoding="application/x-tex">z=3</annotation></semantics></math><span class="katex-html">duplicate</span></span>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        let html = crate::dom::render_html(&dom, root, 0);
        assert!(html.contains(">x=1</span>"), "{html}");
        assert!(html.contains(">y=2</span>"), "{html}");
        assert!(html.contains(">z = 3</span>"), "{html}");
        assert!(!html.contains("duplicate"), "{html}");
        assert_eq!(
            render_text(&dom, root, 0, &TextOptions::default()),
            "x=1y=2z = 3"
        );
    }

    #[test]
    fn ignores_ambiguous_application_data_attributes() {
        let mut dom = Dom::parse_fragment(
            r#"<div data-math="true"><p>Article content.</p></div><div data-formula="enabled">More content.</div>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "Article content.\n\nMore content.\n"
        );
    }

    #[test]
    fn honors_block_mathml_display() {
        let mut dom = Dom::parse_fragment(
            r#"<math display="block"><mi>x</mi><mo>=</mo><mn>1</mn></math>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "$$\nx=1\n$$\n");
    }

    #[test]
    fn honors_block_mathml_inside_a_wrapper() {
        let mut dom = Dom::parse_fragment(
            r#"<span class="katex"><math display="block"><semantics><mi>x</mi><annotation encoding="application/x-tex">x=1</annotation></semantics></math></span>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "$$\nx=1\n$$\n");
    }

    #[test]
    fn ignores_non_tex_mathml_semantic_alternatives() {
        let mut dom = Dom::parse_fragment(
            r#"<math><semantics><mi>x</mi><annotation encoding="text/plain">duplicate x</annotation><annotation-xml encoding="application/xhtml+xml"><span>duplicate x</span></annotation-xml></semantics></math>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "$x$\n");
        assert_eq!(render_text(&dom, root, 0, &TextOptions::default()), "x");
    }

    #[test]
    fn preserves_multiline_display_tex() {
        let mut dom = Dom::parse_fragment(
            r#"<script type="math/tex; mode=display">
\begin{align}
x &= 1 \\
y &= 2
\end{align}
</script><span class="katex katex-display"><math><semantics><mi>z</mi><annotation encoding="application/x-tex">
\begin{aligned}
z &amp;= 3
\end{aligned}
</annotation></semantics></math></span>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        let markdown = dom_to_markdown(&dom, root, 0);
        assert!(
            markdown.contains("$$\n\\begin{align}\nx &= 1 \\\\\ny &= 2\n\\end{align}\n$$"),
            "{markdown}"
        );
        assert!(
            markdown.contains("$$\n\\begin{aligned}\nz &= 3\n\\end{aligned}\n$$"),
            "{markdown}"
        );
    }
}
