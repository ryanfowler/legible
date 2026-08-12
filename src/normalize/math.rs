use crate::dom::{AttrName, Dom, NodeId, Tag};

pub(super) fn normalize(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    let mut script_for_rendered = vec![None; dom.len()];
    let mut rendered_for_script = vec![None; dom.len()];
    for &(node, _) in &nodes {
        if dom.tag(node) != Some(Tag::Script) {
            continue;
        }
        let Some(latex) = explicit_latex(dom, node) else {
            continue;
        };
        let Some(rendered) = adjacent_rendered_math(dom, node).filter(|&rendered| {
            authoritative_rendered_latex(dom, rendered)
                .is_none_or(|rendered_latex| rendered_latex.trim() == latex.trim())
        }) else {
            continue;
        };
        script_for_rendered[rendered.index()] = Some(node);
        rendered_for_script[node.index()] = Some(rendered);
    }

    for (node, _) in nodes {
        if dom.parent(node).is_none() || dom.attr(node, AttrName::DataMath).is_some() {
            continue;
        }
        let paired_script = script_for_rendered[node.index()];
        let Some(latex) = paired_script
            .and_then(|script| explicit_latex(dom, script))
            .or_else(|| explicit_latex(dom, node))
        else {
            continue;
        };
        let rendered_sibling = rendered_for_script[node.index()];
        let source = rendered_sibling.unwrap_or(node);
        let block = is_block_math(dom, source) || is_block_math(dom, node);
        let fallback = math_fallback(dom, source).unwrap_or_else(|| latex.clone());
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
        dom.replace_with(source, canonical);
        let source_script = paired_script.or((source != node).then_some(node));
        if let Some(script) = source_script
            && script != source
        {
            dom.detach(script);
        }
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

fn adjacent_rendered_math(dom: &Dom, node: NodeId) -> Option<NodeId> {
    for forward in [true, false] {
        let mut sibling = if forward {
            dom.next_sibling(node)
        } else {
            dom.prev_sibling(node)
        };
        while sibling.is_some_and(|sibling| {
            dom.text_node(sibling)
                .is_some_and(|text| text.trim().is_empty())
        }) {
            sibling = sibling.and_then(|sibling| {
                if forward {
                    dom.next_sibling(sibling)
                } else {
                    dom.prev_sibling(sibling)
                }
            });
        }
        if sibling.is_some_and(|sibling| has_math_wrapper_class(dom, sibling)) {
            return sibling;
        }
    }
    None
}

fn authoritative_rendered_latex(dom: &Dom, node: NodeId) -> Option<String> {
    for name in ["data-latex", "data-tex"] {
        if let Some(value) = dom
            .attr_by_local_name(node, name)
            .filter(|value| valid_latex(value))
        {
            return Some(value.trim().to_owned());
        }
    }
    std::iter::once(node)
        .chain(dom.descendants(node))
        .find(|&descendant| is_tex_annotation(dom, descendant))
        .map(|annotation| dom.text(annotation))
        .filter(|value| valid_latex(value))
        .map(|value| value.trim().to_owned())
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
    if annotated.is_some() {
        return annotated;
    }
    for name in ["alttext", "aria-label"] {
        if let Some(value) = dom
            .attr_by_local_name(node, name)
            .filter(|value| valid_latex(value))
        {
            return Some(value.trim().to_owned());
        }
    }
    (dom.tag(node) == Some(Tag::Math))
        .then(|| mathml_latex(dom, node))
        .flatten()
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

enum MathMlTask {
    Node(NodeId),
    Literal(String),
}

fn literal(value: &str) -> MathMlTask {
    MathMlTask::Literal(value.to_owned())
}

fn push_joined_children(tasks: &mut Vec<MathMlTask>, children: &[NodeId], separators: &[String]) {
    for (index, &child) in children.iter().enumerate().rev() {
        tasks.push(MathMlTask::Node(child));
        if index > 0 && !separators.is_empty() {
            tasks.push(MathMlTask::Literal(
                separators[(index - 1).min(separators.len() - 1)].clone(),
            ));
        }
    }
}

fn escape_tex_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.trim().chars() {
        match character {
            '\\' => escaped.push_str("\\textbackslash{}"),
            '{' => escaped.push_str("\\{"),
            '}' => escaped.push_str("\\}"),
            '%' | '$' | '#' | '&' | '_' => {
                escaped.push('\\');
                escaped.push(character);
            }
            '^' => escaped.push_str("\\^{}"),
            '~' => escaped.push_str("\\~{}"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn mathml_latex(dom: &Dom, root: NodeId) -> Option<String> {
    let mut output = String::new();
    let mut tasks = vec![MathMlTask::Node(root)];
    while let Some(task) = tasks.pop() {
        match task {
            MathMlTask::Literal(value) => output.push_str(&value),
            MathMlTask::Node(node) => {
                if let Some(text) = dom.text_node(node) {
                    output.push_str(text.trim());
                    continue;
                }
                let local = dom
                    .qual_name(node)
                    .map(|name| name.local.as_ref())
                    .unwrap_or("");
                let children: Vec<NodeId> = dom
                    .children(node)
                    .filter(|&child| {
                        !dom.text_node(child)
                            .is_some_and(|text| text.trim().is_empty())
                    })
                    .collect();
                match (local, children.as_slice()) {
                    ("semantics", [presentation, ..]) => {
                        tasks.push(MathMlTask::Node(*presentation));
                    }
                    ("annotation" | "annotation-xml", _) => {}
                    ("mtext", _) => {
                        output.push_str("\\text{");
                        output.push_str(&escape_tex_text(&dom.text(node)));
                        output.push('}');
                    }
                    ("msup", [base, exponent, ..]) => {
                        tasks.push(literal("}"));
                        tasks.push(MathMlTask::Node(*exponent));
                        tasks.push(literal("^{"));
                        tasks.push(MathMlTask::Node(*base));
                    }
                    ("msub", [base, subscript, ..]) => {
                        tasks.push(literal("}"));
                        tasks.push(MathMlTask::Node(*subscript));
                        tasks.push(literal("_{"));
                        tasks.push(MathMlTask::Node(*base));
                    }
                    ("mfrac", [numerator, denominator, ..]) => {
                        tasks.push(literal("}"));
                        tasks.push(MathMlTask::Node(*denominator));
                        tasks.push(literal("}{"));
                        tasks.push(MathMlTask::Node(*numerator));
                        tasks.push(literal("\\frac{"));
                    }
                    ("msqrt", _) => {
                        tasks.push(literal("}"));
                        push_joined_children(&mut tasks, &children, &[]);
                        tasks.push(literal("\\sqrt{"));
                    }
                    ("mroot", [radicand, index, ..]) => {
                        tasks.push(literal("}"));
                        tasks.push(MathMlTask::Node(*radicand));
                        tasks.push(literal("]{"));
                        tasks.push(MathMlTask::Node(*index));
                        tasks.push(literal("\\sqrt["));
                    }
                    ("munder", [base, under, ..]) => {
                        tasks.push(literal("}"));
                        tasks.push(MathMlTask::Node(*base));
                        tasks.push(literal("}{"));
                        tasks.push(MathMlTask::Node(*under));
                        tasks.push(literal("\\underset{"));
                    }
                    ("mover", [base, over, ..]) => {
                        tasks.push(literal("}"));
                        tasks.push(MathMlTask::Node(*base));
                        tasks.push(literal("}{"));
                        tasks.push(MathMlTask::Node(*over));
                        tasks.push(literal("\\overset{"));
                    }
                    ("munderover", [base, under, over, ..]) => {
                        tasks.push(literal("}}"));
                        tasks.push(MathMlTask::Node(*base));
                        tasks.push(literal("}{"));
                        tasks.push(MathMlTask::Node(*under));
                        tasks.push(literal("\\underset{"));
                        tasks.push(literal("}{"));
                        tasks.push(MathMlTask::Node(*over));
                        tasks.push(literal("\\overset{"));
                    }
                    ("mfenced", _) => {
                        let open = dom.attr_by_local_name(node, "open").unwrap_or("(");
                        let close = dom.attr_by_local_name(node, "close").unwrap_or(")");
                        let separators = dom
                            .attr_by_local_name(node, "separators")
                            .unwrap_or(",")
                            .chars()
                            .filter(|character| !character.is_whitespace())
                            .map(|character| character.to_string())
                            .collect::<Vec<_>>();
                        tasks.push(MathMlTask::Literal(close.to_owned()));
                        push_joined_children(&mut tasks, &children, &separators);
                        tasks.push(MathMlTask::Literal(open.to_owned()));
                    }
                    ("mtable", _) => {
                        tasks.push(literal("\\end{aligned}"));
                        push_joined_children(&mut tasks, &children, &[" \\\\ ".to_owned()]);
                        tasks.push(literal("\\begin{aligned}"));
                    }
                    ("mtr", _) => {
                        push_joined_children(&mut tasks, &children, &[" & ".to_owned()]);
                    }
                    ("mlabeledtr", [label, cells @ ..]) => {
                        let label = dom
                            .text(*label)
                            .trim()
                            .trim_start_matches('(')
                            .trim_end_matches(')')
                            .to_owned();
                        if !label.is_empty() {
                            tasks.push(MathMlTask::Literal(format!(
                                "\\tag{{{}}}",
                                escape_tex_text(&label)
                            )));
                        }
                        push_joined_children(&mut tasks, cells, &[" & ".to_owned()]);
                    }
                    _ => push_joined_children(&mut tasks, &children, &[]),
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
    dom.qual_name(node)
        .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("mjx-container"))
        || dom.attr(node, AttrName::Class).is_some_and(|classes| {
            classes.split_whitespace().any(|class| {
                let class = class.to_ascii_lowercase();
                matches!(
                    class.as_str(),
                    "katex" | "katex-display" | "mathjax" | "mathjax-display" | "tex2jax_process"
                ) || class.starts_with("mathjax_")
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
    if dom
        .attr_by_local_name(node, "display")
        .is_some_and(|value| {
            value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("block")
        })
        || dom.attr(node, AttrName::Class).is_some_and(|classes| {
            classes.split_whitespace().any(|class| {
                matches!(
                    class.to_ascii_lowercase().as_str(),
                    "katex-display" | "math-display" | "mathjax-display"
                )
            })
        })
    {
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
        let mut dom = Dom::parse_fragment(r#"<p>A <span class="katex" aria-label="E equals m c squared"><math><semantics><annotation encoding="application/x-tex">E=mc^2</annotation></semantics></math><span class="katex-html">duplicate</span></span>.</p>"#, Tag::Div).unwrap();
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
    fn converts_extended_mathml_structures() {
        let cases = [
            (
                "<math><mroot>\n  <mi>x</mi>\n  <mn>3</mn>\n</mroot></math>",
                "$\\sqrt[3]{x}$\n",
            ),
            (
                "<math><munder><mo>∑</mo><mi>i</mi></munder><mover><mi>x</mi><mo>¯</mo></mover><munderover><mo>∫</mo><mn>0</mn><mn>1</mn></munderover></math>",
                "$\\underset{i}{∑}\\overset{¯}{x}\\overset{1}{\\underset{0}{∫}}$\n",
            ),
            (
                r#"<math><mfenced open="[" close="]" separators=";,"><mi>a</mi><mi>b</mi><mi>c</mi></mfenced><mtext>speed &amp; time</mtext></math>"#,
                "$[a;b,c]\\text{speed \\& time}$\n",
            ),
        ];
        for (source, expected) in cases {
            let mut dom = Dom::parse_fragment(source, Tag::Div).unwrap();
            let root = dom.root();
            normalize(&mut dom, root);
            assert_eq!(dom_to_markdown(&dom, root, 0), expected, "{source}");
        }
    }

    #[test]
    fn converts_labeled_mathml_equations_without_duplicate_labels() {
        let mut dom = Dom::parse_fragment(
            r#"<math display="block"><mtable><mlabeledtr><mtd><mtext>(1)</mtext></mtd><mtd><mi>x</mi><mo>=</mo><mn>1</mn></mtd></mlabeledtr></mtable></math>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "$$\n\\begin{aligned}x=1\\tag{1}\\end{aligned}\n$$\n"
        );
    }

    #[test]
    fn normalizes_rendered_mathjax_from_accessible_labels() {
        let mut dom = Dom::parse_fragment(
            r#"<mjx-container class="MathJax" jax="SVG" display="true" aria-label="\int_0^1 x dx"><svg><path d="glyph"></path></svg></mjx-container><mjx-container class="MathJax" jax="CHTML" aria-label="a+b"><mjx-math><mjx-mi></mjx-mi></mjx-math></mjx-container>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "$$\n\\int_0^1 x dx\n$$\n$a+b$\n"
        );
    }

    #[test]
    fn uses_adjacent_tex_source_for_rendered_mathjax_once() {
        let mut dom = Dom::parse_fragment(
            r#"<script type="math/tex; mode=display">x=1</script>
<mjx-container class="MathJax" jax="CHTML" display="true"><mjx-math><mjx-mi></mjx-mi></mjx-math></mjx-container>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "$$\nx=1\n$$\n");
    }

    #[test]
    fn keeps_cells_from_an_empty_mathml_equation_label() {
        let mut dom = Dom::parse_fragment(
            r#"<math><mtable><mlabeledtr><mtd></mtd><mtd><mi>x</mi><mo>=</mo><mn>2</mn></mtd></mlabeledtr></mtable></math>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "$\\begin{aligned}x=2\\end{aligned}$\n"
        );
    }

    #[test]
    fn pairs_tex_source_after_rendered_mathjax() {
        let mut dom = Dom::parse_fragment(
            r#"<mjx-container class="MathJax" jax="CHTML" aria-label="y equals two"><mjx-math></mjx-math></mjx-container>
<script type="math/tex">y=2</script>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "$y=2$\n");
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
