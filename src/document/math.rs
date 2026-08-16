use crate::dom::{AttrName, Dom, NodeId, Tag};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub(crate) struct RecognizedMath {
    pub(crate) source: Box<str>,
    pub(crate) fallback: Box<str>,
    pub(crate) block: bool,
}

pub(crate) struct MathAnalysis {
    values: Vec<Option<RecognizedMath>>,
    skipped: Vec<bool>,
}

impl MathAnalysis {
    pub(crate) fn analyze(dom: &Dom, nodes: &[NodeId]) -> Self {
        if !nodes.iter().any(|&node| has_own_math_evidence(dom, node)) {
            return Self::empty();
        }
        Self::analyze_detected(dom, nodes, None)
    }

    pub(crate) fn analyze_with_inventory(
        dom: &Dom,
        nodes: &[NodeId],
        candidates: &[NodeId],
    ) -> Self {
        Self::analyze_with_inventory_and_evidence(dom, nodes, candidates, None)
    }

    pub(crate) fn analyze_with_inventory_and_evidence(
        dom: &Dom,
        nodes: &[NodeId],
        candidates: &[NodeId],
        source_evidence: Option<&super::facts::SourceEvidence>,
    ) -> Self {
        if candidates.is_empty() {
            Self::empty()
        } else {
            Self::analyze_detected(dom, nodes, source_evidence)
        }
    }

    fn empty() -> Self {
        Self {
            values: Vec::new(),
            skipped: Vec::new(),
        }
    }

    fn analyze_detected(
        dom: &Dom,
        nodes: &[NodeId],
        source_evidence: Option<&super::facts::SourceEvidence>,
    ) -> Self {
        let mut has_annotation = vec![false; dom.len()];
        for &node in nodes.iter().rev() {
            has_annotation[node.index()] = is_tex_annotation(dom, node)
                || dom
                    .children(node)
                    .any(|child| has_annotation[child.index()]);
        }
        let mut inside_container = vec![false; dom.len()];
        let mut candidates = vec![false; dom.len()];
        for &node in nodes {
            let inherited = dom
                .parent(node)
                .is_some_and(|parent| inside_container[parent.index()]);
            let own_evidence = source_evidence
                .map(|evidence| evidence.math(node))
                .unwrap_or_else(|| has_own_math_evidence(dom, node));
            let evidence =
                own_evidence || has_math_wrapper_class(dom, node) && has_annotation[node.index()];
            let container =
                evidence && (is_math_root(dom, node) || has_math_wrapper_class(dom, node));
            inside_container[node.index()] = inherited || container;
            candidates[node.index()] =
                evidence && (!inherited || dom.tag(node) == Some(Tag::Script));
        }

        let mut script_for_rendered = vec![None; dom.len()];
        let mut rendered_for_script = vec![None; dom.len()];
        for &node in nodes {
            if !candidates[node.index()] || dom.tag(node) != Some(Tag::Script) {
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

        let mut values = (0..dom.len()).map(|_| None).collect::<Vec<_>>();
        let mut skipped = vec![false; dom.len()];
        for &node in nodes {
            if !candidates[node.index()] || skipped[node.index()] || values[node.index()].is_some()
            {
                continue;
            }
            let paired_script = script_for_rendered[node.index()];
            let Some(latex) = paired_script
                .and_then(|script| explicit_latex(dom, script))
                .or_else(|| explicit_latex(dom, node))
            else {
                continue;
            };
            let rendered = rendered_for_script[node.index()];
            let semantic_root = rendered.unwrap_or(node);
            let block = is_block_math(dom, semantic_root) || is_block_math(dom, node);
            let fallback = math_fallback(dom, semantic_root)
                .or_else(|| {
                    dom.attr(semantic_root, AttrName::DataMath).and_then(|_| {
                        let fallback = dom.text(semantic_root);
                        (!fallback.trim().is_empty()).then_some(fallback)
                    })
                })
                .unwrap_or_else(|| latex.clone());
            values[semantic_root.index()] = Some(RecognizedMath {
                source: latex.trim().into(),
                fallback: fallback.into(),
                block,
            });
            if semantic_root != node {
                skipped[node.index()] = true;
            }
            if let Some(script) = paired_script
                && script != semantic_root
            {
                skipped[script.index()] = true;
            }
        }
        Self { values, skipped }
    }

    pub(crate) fn value(&self, node: NodeId) -> Option<&RecognizedMath> {
        self.values.get(node.index()).and_then(Option::as_ref)
    }

    pub(crate) fn is_skipped(&self, node: NodeId) -> bool {
        self.skipped.get(node.index()).copied().unwrap_or(false)
    }
}

pub(crate) fn is_source_evidence(dom: &Dom, node: NodeId) -> bool {
    has_own_math_evidence(dom, node)
}

fn has_own_math_evidence(dom: &Dom, node: NodeId) -> bool {
    is_math_root(dom, node)
        || is_tex_annotation(dom, node)
        || ["data-latex", "data-tex"]
            .into_iter()
            .filter_map(|name| dom.attr_by_local_name(node, name))
            .any(valid_latex)
        || ["data-math", "data-formula"]
            .into_iter()
            .filter_map(|name| dom.attr_by_local_name(node, name))
            .any(|value| looks_like_latex(value) || has_math_wrapper_class(dom, node))
        || has_math_wrapper_class(dom, node)
            && ["alttext", "aria-label"]
                .into_iter()
                .filter_map(|name| dom.attr_by_local_name(node, name))
                .any(valid_latex)
        || dom.tag(node) == Some(Tag::Script)
            && dom
                .attr(node, AttrName::Type)
                .is_some_and(is_math_script_type)
            && valid_latex(&dom.text(node))
        || dom.tag(node) == Some(Tag::Img)
            && image_is_equation(dom, node)
            && dom
                .attr_by_local_name(node, "alt")
                .is_some_and(looks_like_latex)
}

pub(crate) fn class_is_semantic_evidence(dom: &Dom, node: NodeId) -> bool {
    has_math_wrapper_class(dom, node) || image_is_equation(dom, node)
}

fn math_fallback(dom: &Dom, node: NodeId) -> Option<String> {
    let math = if is_math_root(dom, node) {
        node
    } else {
        dom.descendants(node)
            .find(|&descendant| is_math_root(dom, descendant))?
    };
    let math_nodes = std::iter::once(math)
        .chain(dom.descendants(math))
        .collect::<Vec<_>>();
    let mut inside_annotation = vec![false; dom.len()];
    let mut fallback = String::new();
    for descendant in math_nodes {
        let inherited = dom
            .parent(descendant)
            .is_some_and(|parent| inside_annotation[parent.index()]);
        let annotation = dom
            .qual_name(descendant)
            .is_some_and(|name| is_annotation_element(name.local.as_ref()));
        inside_annotation[descendant.index()] = inherited || annotation;
        if dom.text_node(descendant).is_none() || inside_annotation[descendant.index()] {
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

fn is_math_root(dom: &Dom, node: NodeId) -> bool {
    dom.qual_name(node)
        .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("math"))
}

fn is_annotation_element(local: &str) -> bool {
    local.eq_ignore_ascii_case("annotation") || local.eq_ignore_ascii_case("annotation-xml")
}

/// Marks elements that contain a usable TeX annotation and are math or inside
/// a known math wrapper.
///
/// The annotation scan is the cheap source gate. Only the ancestor paths of
/// matching annotations enter the targeted pass, so an ordinary page does not
/// allocate semantic sets and a page with one equation does not build sets for
/// every source element.
pub(crate) fn accessible_math_nodes(dom: &Dom, nodes: &[(NodeId, u32)]) -> HashSet<NodeId> {
    let annotations: Vec<_> = nodes
        .iter()
        .map(|&(node, _)| node)
        .filter(|&node| is_tex_annotation(dom, node))
        .collect();
    if annotations.is_empty() {
        return HashSet::new();
    }

    let root = nodes.first().map(|&(node, _)| node);
    let mut relevant = HashSet::new();
    for annotation in annotations {
        let mut node = Some(annotation);
        while let Some(current) = node {
            relevant.insert(current);
            if Some(current) == root {
                break;
            }
            node = dom.parent(current);
        }
    }

    let mut inside_wrapper = HashSet::new();
    let mut accessible = HashSet::new();
    for &(node, _) in nodes {
        if !relevant.contains(&node) {
            continue;
        }
        let inherited = dom
            .parent(node)
            .is_some_and(|parent| inside_wrapper.contains(&parent));
        let wrapper = inherited || has_math_wrapper_class(dom, node);
        if wrapper {
            inside_wrapper.insert(node);
        }
        if is_math_root(dom, node) || wrapper {
            accessible.insert(node);
        }
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

    let math_root = is_math_root(dom, node) || has_math_wrapper_class(dom, node);
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
    is_math_root(dom, node)
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
    if dom
        .attr(node, AttrName::DataMath)
        .is_some_and(|value| value.eq_ignore_ascii_case("block"))
    {
        return true;
    }
    if std::iter::once(node)
        .chain(dom.descendants(node))
        .any(|math| {
            is_math_root(dom, math)
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
