#![allow(clippy::collapsible_if)]

use super::{Dom, DomError, ElementData, NodeData, NodeId, NodeLink, Tag};
use crate::budget::ParseBudget;
use html5ever::tokenizer::TokenizerOpts;
use html5ever::tree_builder::{ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{Attribute as HtmlAttribute, ParseOpts, QualName, parse_document, parse_fragment};
use smallvec::SmallVec;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use tendril::{StrTendril, TendrilSink};

#[derive(Debug, Clone)]
struct OwnedElemName(QualName);
impl ElemName for OwnedElemName {
    fn ns(&self) -> &html5ever::Namespace {
        &self.0.ns
    }
    fn local_name(&self) -> &html5ever::LocalName {
        &self.0.local
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseLimitKind {
    Nodes,
    Elements,
    TotalAttributes,
    AttributesPerElement,
    TextBytes,
    Depth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParseLimit {
    pub(crate) kind: ParseLimitKind,
    pub(crate) observed: usize,
    pub(crate) limit: usize,
}

#[derive(Debug)]
pub(crate) enum ParseError {
    Dom(DomError),
    Limit(ParseLimit),
}

struct DomSink {
    dom: RefCell<Dom>,
    quirks: Cell<QuirksMode>,
    budget: ParseBudget,
    error: RefCell<Option<ParseError>>,
    // Parsing checks this flag for every token. Keep the hot poisoned check
    // out of RefCell borrow bookkeeping; the error value is only read at the
    // end of parsing.
    poisoned: Cell<bool>,
    elements: Cell<usize>,
    total_attributes: Cell<usize>,
    text_bytes: Cell<usize>,
    depths: RefCell<Vec<u32>>,
}
impl DomSink {
    fn new(fragment: bool, capacity: usize, budget: ParseBudget) -> Self {
        let mut depths = Vec::with_capacity(capacity.max(1));
        depths.push(0);
        Self {
            dom: RefCell::new(Dom::with_capacity(
                if fragment {
                    NodeData::Fragment
                } else {
                    NodeData::Document
                },
                capacity,
            )),
            quirks: Cell::new(html5ever::tree_builder::NoQuirks),
            budget,
            error: RefCell::new(None),
            poisoned: Cell::new(false),
            elements: Cell::new(0),
            total_attributes: Cell::new(0),
            text_bytes: Cell::new(0),
            depths: RefCell::new(depths),
        }
    }

    #[inline]
    fn is_poisoned(&self) -> bool {
        self.poisoned.get()
    }

    fn poison(&self, error: ParseError) {
        if !self.poisoned.replace(true) {
            *self.error.borrow_mut() = Some(error);
        }
    }

    fn limit(&self, kind: ParseLimitKind, observed: usize, limit: usize) -> bool {
        if limit > 0 && observed > limit {
            self.poison(ParseError::Limit(ParseLimit {
                kind,
                observed,
                limit,
            }));
            false
        } else {
            true
        }
    }

    #[inline]
    fn reserve_node(&self) -> bool {
        if self.is_poisoned() {
            return false;
        }
        if self.budget.max_nodes == 0 {
            return true;
        }
        let observed = self.dom.borrow().len().saturating_add(1);
        self.limit(ParseLimitKind::Nodes, observed, self.budget.max_nodes)
    }

    #[inline]
    fn create_node(&self, data: NodeData) -> Option<NodeId> {
        if !self.reserve_node() {
            return None;
        }
        let result = self.dom.borrow_mut().create(data);
        match result {
            Ok(id) => {
                if self.budget.max_depth != 0 {
                    self.depths.borrow_mut().push(0);
                }
                Some(id)
            }
            Err(error) => {
                self.poison(ParseError::Dom(error));
                None
            }
        }
    }

    fn inert_handle(&self) -> NodeId {
        self.dom.borrow().root()
    }

    #[inline]
    fn add_text_bytes(&self, bytes: usize) -> bool {
        if self.budget.max_text_bytes == 0 {
            return true;
        }
        let observed = self.text_bytes.get().saturating_add(bytes);
        if !self.limit(
            ParseLimitKind::TextBytes,
            observed,
            self.budget.max_text_bytes,
        ) {
            return false;
        }
        self.text_bytes.set(observed);
        true
    }

    #[inline]
    fn check_attributes(&self, existing: usize, count: usize) -> bool {
        if self.budget.max_attributes_per_element == 0 && self.budget.max_total_attributes == 0 {
            return true;
        }
        if !self.limit(
            ParseLimitKind::AttributesPerElement,
            existing.saturating_add(count),
            self.budget.max_attributes_per_element,
        ) {
            return false;
        }
        let observed = self.total_attributes.get().saturating_add(count);
        if !self.limit(
            ParseLimitKind::TotalAttributes,
            observed,
            self.budget.max_total_attributes,
        ) {
            return false;
        }
        self.total_attributes.set(observed);
        true
    }

    fn check_attachment(&self, parent: NodeId, child: NodeId) -> bool {
        if self.is_poisoned() {
            return false;
        }
        if self.budget.max_depth == 0 {
            return true;
        }
        let dom = self.dom.borrow();
        let parent_depth = self.depths.borrow()[parent.index()];
        let child_is_element = dom.is_element(child);
        let child_depth = parent_depth + if child_is_element { 1 } else { 0 };
        let mut pending = SmallVec::<[(NodeId, u32); 16]>::new();
        pending.push((child, child_depth));
        while let Some((node, depth)) = pending.pop() {
            if dom.is_element(node)
                && !self.limit(ParseLimitKind::Depth, depth as usize, self.budget.max_depth)
            {
                return false;
            }
            for child in dom.children(node) {
                pending.push((child, depth + if dom.is_element(child) { 1 } else { 0 }));
            }
            if let NodeData::Element(element) = &dom.node(node).data
                && let Some(template) = element.template_contents.get()
            {
                pending.push((template, depth));
            }
        }
        true
    }

    fn update_attachment_depths(&self, parent: NodeId, child: NodeId) {
        if self.budget.max_depth == 0 {
            return;
        }
        let dom = self.dom.borrow();
        let parent_depth = self.depths.borrow()[parent.index()];
        let child_depth = parent_depth + if dom.is_element(child) { 1 } else { 0 };
        let mut pending = SmallVec::<[(NodeId, u32); 16]>::new();
        pending.push((child, child_depth));
        let mut depths = self.depths.borrow_mut();
        while let Some((node, depth)) = pending.pop() {
            depths[node.index()] = depth;
            for child in dom.children(node) {
                pending.push((child, depth + if dom.is_element(child) { 1 } else { 0 }));
            }
            if let NodeData::Element(element) = &dom.node(node).data
                && let Some(template) = element.template_contents.get()
            {
                pending.push((template, depth));
            }
        }
    }
}
fn node_capacity_hint(html: &str) -> usize {
    let markup = memchr::memchr_iter(b'<', html.as_bytes()).count();
    if markup == 0 || markup > html.len() / 10 {
        return 1;
    }
    markup
        .saturating_add(markup / 3)
        .saturating_add(64)
        .min(32_768)
}
fn opts(drop_doctype: bool) -> ParseOpts {
    ParseOpts {
        tokenizer: TokenizerOpts::default(),
        tree_builder: html5ever::tree_builder::TreeBuilderOpts {
            scripting_enabled: false,
            drop_doctype,
            ..Default::default()
        },
    }
}
impl Dom {
    pub(crate) fn parse_document(html: &str) -> Result<Self, DomError> {
        Self::parse_document_with_budget(html, &ParseBudget::default()).map_err(|error| match error
        {
            ParseError::Dom(error) => error,
            ParseError::Limit(limit) => DomError(format!(
                "HTML parse limit exceeded: {:?} {} (max {})",
                limit.kind, limit.observed, limit.limit
            )),
        })
    }

    pub(crate) fn parse_document_with_budget(
        html: &str,
        budget: &ParseBudget,
    ) -> Result<Self, ParseError> {
        let _phase = crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Parse);
        crate::instrumentation::record_parse_call();
        parse_document(
            DomSink::new(false, node_capacity_hint(html), budget.clone()),
            opts(false),
        )
        .one(html)
    }
    pub(crate) fn parse_fragment(html: &str, context: Tag) -> Result<Self, DomError> {
        let _phase = crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Parse);
        crate::instrumentation::record_parse_call();
        let sink = DomSink::new(true, node_capacity_hint(html), ParseBudget::default());
        let context = QualName::new(
            None,
            html5ever::ns!(html),
            html5ever::LocalName::from(context.as_lowercase_str()),
        );
        let mut dom = parse_fragment(sink, opts(true), context, Vec::new(), false)
            .one(html)
            .map_err(|error| match error {
                ParseError::Dom(error) => error,
                ParseError::Limit(limit) => DomError(format!(
                    "HTML parse limit exceeded: {:?} {} (max {})",
                    limit.kind, limit.observed, limit.limit
                )),
            })?;

        // html5ever follows the HTML fragment parsing algorithm and inserts a
        // synthetic <html> element below the sink's document node. The DOM
        // represents fragments with that node directly, so expose the
        // synthetic element's children as the fragment roots instead.
        let root = dom.root();
        if let Some(fragment_root) = dom
            .children(root)
            .find(|&id| dom.tag(id) == Some(Tag::Html))
        {
            let children: Vec<_> = dom.children(fragment_root).collect();
            for child in children {
                dom.insert_before(fragment_root, child)
            }
            dom.detach(fragment_root);
        }
        Ok(dom)
    }
}
impl TreeSink for DomSink {
    type Handle = NodeId;
    type Output = Result<Dom, ParseError>;
    type ElemName<'a>
        = OwnedElemName
    where
        Self: 'a;
    fn finish(self) -> Result<Dom, ParseError> {
        if let Some(error) = self.error.into_inner() {
            return Err(error);
        }
        let mut dom = self.dom.into_inner();
        dom.quirks_mode = self.quirks.get();
        Ok(dom)
    }
    fn parse_error(&self, _msg: Cow<'static, str>) {}
    fn get_document(&self) -> NodeId {
        self.dom.borrow().root()
    }
    fn elem_name<'a>(&'a self, target: &'a NodeId) -> OwnedElemName {
        let name = self
            .dom
            .borrow()
            .qual_name(*target)
            .unwrap_or_else(|| QualName::new(None, html5ever::ns!(html), "div".into()));
        OwnedElemName(name)
    }
    #[inline]
    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<HtmlAttribute>,
        flags: ElementFlags,
    ) -> NodeId {
        if self.is_poisoned() || !self.check_attributes(0, attrs.len()) {
            return self.inert_handle();
        }
        if self.budget.max_elements != 0 {
            if !self.limit(
                ParseLimitKind::Elements,
                self.elements.get().saturating_add(1),
                self.budget.max_elements,
            ) {
                return self.inert_handle();
            }
        }
        let tag = Tag::from_qual_name(&name);
        let (compact_name, compact_attrs) = {
            let mut dom = self.dom.borrow_mut();
            let compact_name = dom.compact_element_name(&name, tag);
            let compact_attrs = attrs
                .into_iter()
                .map(|attribute| dom.compact_attribute(attribute))
                .collect();
            (compact_name, compact_attrs)
        };
        let Some(id) = self.create_node(NodeData::Element(ElementData {
            name: compact_name,
            local: name.local.clone(),
            tag,
            attrs: compact_attrs,
            template_contents: NodeLink::NONE,
            mathml_annotation_xml_integration_point: flags.mathml_annotation_xml_integration_point,
        })) else {
            return self.inert_handle();
        };
        if self.budget.max_elements != 0 {
            self.elements.set(self.elements.get() + 1);
        }
        if flags.template {
            let Some(f) = self.create_node(NodeData::Fragment) else {
                return self.inert_handle();
            };
            let mut dom = self.dom.borrow_mut();
            if let NodeData::Element(e) = &mut dom.node_mut(id).data {
                e.template_contents = NodeLink::from_option(Some(f))
            }
        }
        id
    }
    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.create_node(NodeData::Comment(text))
            .unwrap_or_else(|| self.inert_handle())
    }
    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        self.create_node(NodeData::ProcessingInstruction {
            target,
            contents: data,
        })
        .unwrap_or_else(|| self.inert_handle())
    }
    #[inline]
    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        if self.is_poisoned() {
            return;
        }
        let mut d = self.dom.borrow_mut();
        match child {
            NodeOrText::AppendNode(id) => {
                if self.budget.max_depth == 0 {
                    d.append_child(*parent, id);
                    return;
                }
                drop(d);
                if !self.check_attachment(*parent, id) {
                    return;
                }
                let mut d = self.dom.borrow_mut();
                d.append_child(*parent, id);
                drop(d);
                self.update_attachment_depths(*parent, id);
            }
            NodeOrText::AppendText(text) => {
                if !self.add_text_bytes(text.len()) {
                    return;
                }
                if let Some(last) = d.last_child(*parent) {
                    if let NodeData::Text(existing) = &mut d.node_mut(last).data {
                        existing.push_tendril(&text);
                        return;
                    }
                }
                drop(d);
                let Some(id) = self.create_node(NodeData::Text(text)) else {
                    return;
                };
                let mut d = self.dom.borrow_mut();
                d.append_child(*parent, id)
            }
        }
    }
    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        if self.dom.borrow().parent(*element).is_some() {
            self.append_before_sibling(element, child)
        } else {
            self.append(prev, child)
        }
    }
    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        if self.is_poisoned() {
            return;
        }
        let Some(id) = self.create_node(NodeData::Doctype {
            name,
            _public_id: public_id,
            _system_id: system_id,
        }) else {
            return;
        };
        let mut d = self.dom.borrow_mut();
        let root = d.root();
        d.append_child(root, id)
    }
    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        if self.is_poisoned() {
            return self.inert_handle();
        }
        self.dom
            .borrow()
            .node(*target)
            .data
            .element_template()
            .expect("template")
    }
    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }
    fn set_quirks_mode(&self, mode: QuirksMode) {
        if self.is_poisoned() {
            return;
        }
        self.quirks.set(mode)
    }
    #[inline]
    fn append_before_sibling(&self, sibling: &NodeId, child: NodeOrText<NodeId>) {
        if self.is_poisoned() {
            return;
        }
        let mut d = self.dom.borrow_mut();
        match child {
            NodeOrText::AppendNode(id) => {
                let Some(parent) = d.parent(*sibling) else {
                    drop(d);
                    self.poison(ParseError::Dom(DomError("reference is detached".into())));
                    return;
                };
                if self.budget.max_depth == 0 {
                    d.insert_before(*sibling, id);
                    return;
                }
                drop(d);
                if !self.check_attachment(parent, id) {
                    return;
                }
                let mut d = self.dom.borrow_mut();
                d.insert_before(*sibling, id);
                drop(d);
                self.update_attachment_depths(parent, id);
            }
            NodeOrText::AppendText(text) => {
                if !self.add_text_bytes(text.len()) {
                    return;
                }
                if let Some(prev) = d.prev_sibling(*sibling) {
                    if let NodeData::Text(existing) = &mut d.node_mut(prev).data {
                        existing.push_tendril(&text);
                        return;
                    }
                }
                drop(d);
                let Some(id) = self.create_node(NodeData::Text(text)) else {
                    return;
                };
                let mut d = self.dom.borrow_mut();
                d.insert_before(*sibling, id)
            }
        }
    }
    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<HtmlAttribute>) {
        if self.is_poisoned() {
            return;
        }
        let mut d = self.dom.borrow_mut();
        let existing = match &d.node(*target).data {
            NodeData::Element(e) => e.attrs.clone(),
            _ => return,
        };
        let additions = attrs
            .iter()
            .filter(|a| !existing.iter().any(|x| d.attribute_matches(x, &a.name)))
            .count();
        if !self.check_attributes(existing.len(), additions) {
            return;
        }
        let additions = attrs
            .into_iter()
            .filter(|a| !existing.iter().any(|x| d.attribute_matches(x, &a.name)))
            .collect::<Vec<_>>();
        let compacted = additions
            .into_iter()
            .map(|a| d.compact_attribute(a))
            .collect::<Vec<_>>();
        if let NodeData::Element(e) = &mut d.node_mut(*target).data {
            e.attrs.extend(compacted);
        }
    }
    fn remove_from_parent(&self, target: &NodeId) {
        if self.is_poisoned() {
            return;
        }
        self.dom.borrow_mut().detach(*target)
    }
    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        if self.is_poisoned() {
            return;
        }
        let children: Vec<_> = self.dom.borrow().children(*node).collect();
        if self.budget.max_depth == 0 {
            self.dom.borrow_mut().move_children(*node, *new_parent);
            return;
        }
        for child in &children {
            if !self.check_attachment(*new_parent, *child) {
                return;
            }
        }
        let mut d = self.dom.borrow_mut();
        d.move_children(*node, *new_parent);
        drop(d);
        for child in children {
            self.update_attachment_depths(*new_parent, child);
        }
    }
    fn is_mathml_annotation_xml_integration_point(&self, handle: &NodeId) -> bool {
        if self.is_poisoned() {
            return false;
        }
        match &self.dom.borrow().node(*handle).data {
            NodeData::Element(e) => e.mathml_annotation_xml_integration_point,
            _ => false,
        }
    }
}
trait ElementTemplate {
    fn element_template(&self) -> Option<NodeId>;
}
impl ElementTemplate for NodeData {
    fn element_template(&self) -> Option<NodeId> {
        match self {
            NodeData::Element(e) => e.template_contents.get(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_capacity_hint_avoids_sparse_and_markup_dense_overallocation() {
        assert_eq!(node_capacity_hint("plain text"), 1);
        assert_eq!(node_capacity_hint("<<<<<<<<<<<<<<<<<<<<"), 1);
        assert!(
            node_capacity_hint(
                "<article><p>This document has enough text to make its markup sparse.</p></article>"
            ) > 1
        );
    }

    #[test]
    fn poisoned_sink_counts_existing_attributes_on_repair() {
        let sink = DomSink::new(
            false,
            4,
            ParseBudget {
                max_attributes_per_element: 1,
                ..ParseBudget::default()
            },
        );
        let element = sink.create_element(
            QualName::new(None, html5ever::ns!(html), "div".into()),
            vec![HtmlAttribute {
                name: QualName::new(None, html5ever::ns!(), "id".into()),
                value: StrTendril::from("one"),
            }],
            ElementFlags::default(),
        );
        sink.add_attrs_if_missing(
            &element,
            vec![HtmlAttribute {
                name: QualName::new(None, html5ever::ns!(), "class".into()),
                value: StrTendril::from("two"),
            }],
        );

        let result = sink.finish();
        assert!(matches!(
            result,
            Err(ParseError::Limit(ParseLimit {
                kind: ParseLimitKind::AttributesPerElement,
                limit: 1,
                ..
            }))
        ));
    }
}
