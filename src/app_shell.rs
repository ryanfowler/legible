//! Client application shells and their source-present content.
//!
//! A client application can send a visible document that is almost empty and
//! keep the real content in source that a browser shows only in other
//! conditions: `<noscript>` content shows when scripts are off, `<template>`
//! content shows when a script clones it, and React streaming fragments show
//! when an inline script moves them into their placeholders. Extraction does
//! not run scripts. This module recognizes these structures during document
//! preparation and moves the content into the document only when the
//! structure proves where it belongs.
//!
//! The module also owns the loading-placeholder vocabulary that the quality
//! checks use to recognize an unresolved shell result.

use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::{
    get_inner_text_limited, has_static_hidden_marker, prose_stats, symbolic_text_length,
};
use crate::tokens::{any_token_contains, has_token};
use std::collections::{HashMap, HashSet};

/// A progress indicator with at most this much text has no meaningful text.
const INDICATOR_TEXT_CHARS: usize = 8;
/// Hidden fallback content must contain at least this much text.
const FALLBACK_TEXT_CHARS: usize = 150;
/// A text run of this length is prose, not a control label.
const PROSE_RUN_CHARS: usize = 25;
/// Text below this length can be a "JavaScript required" notice.
const NOTICE_TEXT_CHARS: usize = 1_000;

/// Source structures that can hold content that the visible document lacks.
/// The preparation traversal records them so that recovery needs no separate
/// discovery pass.
#[derive(Default)]
pub(crate) struct SourceFallbacks {
    noscripts: Vec<NodeId>,
    templates: Vec<NodeId>,
    placeholders: Vec<NodeId>,
    segments: Vec<NodeId>,
}

impl SourceFallbacks {
    /// Records `node` when it can hold fallback content. Call this for each
    /// node of the preparation traversal, before preparation detaches nodes.
    pub(crate) fn observe(&mut self, dom: &Dom, node: NodeId) {
        match dom.tag(node) {
            Some(Tag::Noscript) => self.noscripts.push(node),
            Some(Tag::Template) => match streamed_id(dom, node) {
                Some((b'B' | b'P', _)) => self.placeholders.push(node),
                _ => self.templates.push(node),
            },
            Some(_)
                if dom.has_attr(node, AttrName::Hidden)
                    && matches!(streamed_id(dom, node), Some((b'S', _))) =>
            {
                self.segments.push(node)
            }
            _ => {}
        }
    }

    /// Moves source-present content into the document.
    ///
    /// Streamed segments always replace their matching placeholders, because
    /// the matching IDs prove the destination. A `<noscript>` or ordinary
    /// `<template>` is revealed only when the visible body is an application
    /// shell and the hidden content is substantial prose. At most one such
    /// fallback is revealed, so a page with several copies does not duplicate
    /// content.
    ///
    /// Revealed template contents were not part of the preparation traversal.
    /// Nodes in them that satisfy `is_inactive` are added to `removals`.
    pub(crate) fn reveal(
        self,
        dom: &mut Dom,
        body: Option<NodeId>,
        removals: &mut Vec<NodeId>,
        is_inactive: impl Fn(&Dom, NodeId) -> bool,
    ) {
        let Some(body) = body else {
            return;
        };
        if !self.placeholders.is_empty() && !self.segments.is_empty() {
            splice_streamed_segments(dom, &self.placeholders, &self.segments);
        }
        if self.noscripts.is_empty() && self.templates.is_empty() {
            return;
        }
        let Some((fallback, _)) = self
            .noscripts
            .iter()
            .chain(&self.templates)
            .copied()
            .filter(|&node| is_body_fallback(dom, body, node))
            .filter_map(|node| {
                let root = if dom.tag(node) == Some(Tag::Template) {
                    dom.template_contents(node)?
                } else {
                    node
                };
                Some((node, substantial_fallback_length(dom, root)?))
            })
            .max_by_key(|&(_, length)| length)
        else {
            return;
        };
        if !is_shell(dom, body) {
            return;
        }
        if dom.tag(fallback) == Some(Tag::Template) {
            let previous = dom.prev_sibling(fallback);
            let parent = dom.parent(fallback);
            let end = dom.next_sibling(fallback);
            if !dom.replace_template_with_contents(fallback) {
                return;
            }
            let mut next = previous.map_or_else(
                || parent.and_then(|parent| dom.first_child(parent)),
                |previous| dom.next_sibling(previous),
            );
            // Collect inactive nodes in the revealed range. The range ends at
            // the next node that was already in the document.
            while let Some(node) = next
                && Some(node) != end
            {
                if is_inactive(dom, node) {
                    removals.push(node);
                } else {
                    removals.extend(dom.descendants(node).filter(|&id| is_inactive(dom, id)));
                }
                next = dom.next_sibling(node);
            }
        } else {
            // Preparation detaches the empty noscript element later.
            dom.move_children_before(fallback);
        }
    }
}

/// True for an individual form control. A control beside a loading
/// placeholder is application evidence. A container such as `<form>` is not
/// a control, so its other text stays visible content.
pub(crate) fn is_control(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(Tag::Button | Tag::Input | Tag::Select | Tag::Textarea)
    )
}

/// A loading placeholder text node.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LoadingPlaceholder {
    /// The text shows ongoing progress with an ellipsis or "while … loads"
    /// wording, such as "Loading…" or "Please wait while the app loads".
    /// Other forms, such as "Loading dock", can also be ordinary labels.
    pub(crate) progressive: bool,
}

/// Returns the placeholder when the text node `node` has short loading
/// wording and is the whole text of its element, apart from whitespace and
/// controls. A spinner label "Loading workspace" qualifies. A loading phrase
/// inside running prose, such as `<p>Loading <b>dock closed</b></p>`, does
/// not. This is the one definition of a placeholder for preparation and for
/// result quality checks.
pub(crate) fn loading_placeholder(dom: &Dom, node: NodeId) -> Option<LoadingPlaceholder> {
    let text = dom.text_node(node)?;
    let progressive = loading_placeholder_form(text)?;
    dom.parent(node)
        .is_some_and(|parent| {
            dom.children(parent).all(|child| {
                child == node
                    || is_control(dom, child)
                    || dom
                        .text_node(child)
                        .is_some_and(|text| text.trim().is_empty())
            })
        })
        .then_some(LoadingPlaceholder { progressive })
}

/// True when `text` is a short loading, waiting, or start-up message.
/// Returns whether the form is progressive, or `None` for other text.
/// The match uses the wording, not the punctuation:
///
/// - A progress verb ("Loading", "Fetching", "Initializing") is followed by
///   at most two words: "Loading workspace", "Loading recovery guide".
/// - A waiting phrase ("Please wait", "One moment") stands alone or
///   continues with "while …" wording: "Please wait while the app loads".
/// - A short subject ends with "loading" or "starting": "Workspace loading".
///
/// A sentence with other words, such as "Please wait outside." or "Loading
/// dock closed today.", is content.
fn loading_placeholder_form(text: &str) -> Option<bool> {
    let trimmed = text.trim();
    let ellipsis = trimmed.ends_with('…') || trimmed.ends_with("...");
    let text = trimmed.trim_end_matches(['.', '…', '!', ' ']).trim_end();
    if text.is_empty() || text.len() > 60 {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|word| !word.is_empty())
        .collect();
    if words.is_empty()
        || ["complete", "done", "finished", "failed", "error"]
            .iter()
            .any(|outcome| lower.contains(outcome))
    {
        return None;
    }
    const VERBS: &[&str] = &["loading", "fetching", "initializing", "initialising"];
    const WAITS: &[&[&str]] = &[
        &["please", "wait"],
        &["one", "moment"],
        &["just", "a", "moment"],
        &["hang", "tight"],
        &["starting", "up"],
    ];
    let matched = if VERBS.contains(&words[0]) {
        words.len() <= 3
    } else if let Some(wait) = WAITS.iter().find(|wait| words.starts_with(wait)) {
        let rest = &words[wait.len()..];
        if rest.is_empty() {
            true
        } else if rest[0] == "while"
            && rest.len() <= 6
            && rest
                .last()
                .is_some_and(|last| matches!(*last, "loads" | "loading" | "starts" | "starting"))
        {
            return Some(true);
        } else {
            false
        }
    } else {
        words.len() <= 3 && matches!(*words.last().unwrap_or(&""), "loading" | "starting")
    };
    matched.then_some(ellipsis)
}

/// True when an element is a progress indicator: a progress bar, or an
/// element whose class or ID has a whole progress-name part. Parts are split
/// on whitespace, `-`, and `_`, so `loading-spinner` matches but
/// `file-uploader` does not.
pub(crate) fn is_progress_indicator(dom: &Dom, node: NodeId) -> bool {
    const NAMES: &[&str] = &[
        "spinner",
        "loader",
        "loading",
        "skeleton",
        "progress",
        "progressbar",
        "throbber",
    ];
    dom.attr(node, AttrName::Role)
        .is_some_and(|role| has_token(role, "progressbar"))
        || [AttrName::Class, AttrName::Id]
            .into_iter()
            .filter_map(|name| dom.attr(node, name))
            .any(|value| {
                value
                    .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
                    .any(|part| NAMES.iter().any(|name| part.eq_ignore_ascii_case(name)))
            })
}

/// True when a progress indicator has no meaningful text. Its text may be
/// empty, punctuation, or a progress value such as "40%" or "3/10", up to
/// `INDICATOR_TEXT_CHARS` characters. A letter, a heading, or a label makes
/// the text visible content. The scan stops at the first such node.
fn is_textless_indicator(dom: &Dom, node: NodeId) -> bool {
    let mut length = 0;
    std::iter::once(node)
        .chain(dom.descendants(node))
        .all(|child| {
            if matches!(
                dom.tag(child),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::Label)
            ) {
                return false;
            }
            dom.text_node(child).is_none_or(|text| {
                symbolic_text_length(text).is_some_and(|chars| {
                    length += chars;
                    length <= INDICATOR_TEXT_CHARS
                })
            })
        })
}

/// True for a region that declares that its content is still loading.
/// This is evidence of an application, but the region's text is still
/// visible content.
pub(crate) fn is_busy_region(dom: &Dom, node: NodeId) -> bool {
    dom.attr_by_local_name(node, "aria-busy")
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

/// Parses a React streaming ID such as `B:0`, `P:1a`, or `prefixS:2`.
/// Returns the kind byte and the ID with the kind removed, so the matching
/// placeholder and segment have the same key.
fn streamed_id(dom: &Dom, node: NodeId) -> Option<(u8, String)> {
    let id = dom.attr(node, AttrName::Id)?;
    let colon = id.rfind(':')?;
    let number = &id[colon + 1..];
    if colon == 0 || number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let kind = id.as_bytes()[colon - 1];
    if !matches!(kind, b'B' | b'P' | b'S') {
        return None;
    }
    Some((kind, format!("{}{}", &id[..colon - 1], number)))
}

/// Moves each hidden streamed segment to its placeholder, as React's inline
/// completion scripts do. A boundary placeholder (`B:`) also removes the
/// fallback that follows it up to the closing `<!--/$-->` comment.
fn splice_streamed_segments(dom: &mut Dom, placeholders: &[NodeId], segments: &[NodeId]) {
    let mut segment_keys: HashMap<String, NodeId> = HashMap::with_capacity(segments.len());
    for &segment in segments {
        if let Some((_, key)) = streamed_id(dom, segment) {
            segment_keys.entry(key).or_insert(segment);
        }
    }
    let complete = complete_segments(dom, &segment_keys);
    for &placeholder in placeholders {
        let Some((kind, key)) = streamed_id(dom, placeholder) else {
            continue;
        };
        let Some(&segment) = segment_keys.get(&key) else {
            continue;
        };
        if dom.parent(placeholder).is_none()
            || dom.parent(segment).is_none()
            || dom
                .template_contents(placeholder)
                .is_some_and(|contents| dom.has_non_whitespace_text(contents))
            || dom
                .ancestors(placeholder)
                .any(|ancestor| ancestor == segment)
        {
            continue;
        }
        if kind == b'B' {
            // Completing a boundary removes its readable fallback. Do it only
            // when the segment's whole content is available.
            if !complete.contains(&segment) {
                continue;
            }
            remove_boundary_fallback(dom, placeholder);
        }
        while let Some(child) = dom.first_child(segment) {
            dom.insert_before(placeholder, child);
        }
        dom.detach(segment);
        dom.detach(placeholder);
    }
}

/// Returns the segments whose content is complete: every `B:` or `P:`
/// placeholder inside the segment has an empty template and a matching
/// segment that is also complete. A cycle makes its segments incomplete.
/// The depth-first search uses an explicit stack, so long segment chains
/// cannot overflow the call stack. Each segment is scanned once.
fn complete_segments(dom: &Dom, segment_keys: &HashMap<String, NodeId>) -> HashSet<NodeId> {
    enum State {
        Visiting,
        Done(bool),
    }
    struct Frame {
        segment: NodeId,
        references: Vec<Option<NodeId>>,
        next: usize,
        complete: bool,
    }
    let frame = |segment: NodeId| Frame {
        segment,
        references: dom
            .descendants(segment)
            .filter(|&node| dom.tag(node) == Some(Tag::Template))
            .filter_map(|placeholder| match streamed_id(dom, placeholder)? {
                (b'B' | b'P', key) => {
                    let empty = dom
                        .template_contents(placeholder)
                        .is_none_or(|contents| !dom.has_non_whitespace_text(contents));
                    Some(empty.then(|| segment_keys.get(&key).copied()).flatten())
                }
                _ => None,
            })
            .collect(),
        next: 0,
        complete: true,
    };
    let mut states: HashMap<NodeId, State> = HashMap::with_capacity(segment_keys.len());
    for &start in segment_keys.values() {
        if states.contains_key(&start) {
            continue;
        }
        states.insert(start, State::Visiting);
        let mut stack = vec![frame(start)];
        while let Some(top) = stack.last_mut() {
            if let Some(&reference) = top.references.get(top.next) {
                top.next += 1;
                match reference {
                    None => top.complete = false,
                    Some(segment) => match states.get(&segment) {
                        Some(State::Visiting) => top.complete = false,
                        Some(State::Done(complete)) => top.complete &= *complete,
                        None => {
                            states.insert(segment, State::Visiting);
                            stack.push(frame(segment));
                        }
                    },
                }
                continue;
            }
            let Some(Frame {
                segment, complete, ..
            }) = stack.pop()
            else {
                break;
            };
            states.insert(segment, State::Done(complete));
            if let Some(parent) = stack.last_mut() {
                parent.complete &= complete;
            }
        }
    }
    states
        .into_iter()
        .filter_map(|(segment, state)| matches!(state, State::Done(true)).then_some(segment))
        .collect()
}

/// Removes the siblings after a boundary placeholder up to the comment that
/// closes the Suspense boundary. Nothing is removed when the closing comment
/// is absent, because the fallback extent is then unknown.
fn remove_boundary_fallback(dom: &mut Dom, placeholder: NodeId) {
    let mut depth = 0usize;
    let mut end = None;
    let mut next = dom.next_sibling(placeholder);
    while let Some(node) = next {
        match dom.comment_text(node) {
            Some("/$") if depth == 0 => {
                end = Some(node);
                break;
            }
            Some("/$") => depth -= 1,
            Some(text) if text.starts_with('$') => depth += 1,
            _ => {}
        }
        next = dom.next_sibling(node);
    }
    let Some(end) = end else {
        return;
    };
    while let Some(node) = dom.next_sibling(placeholder)
        && node != end
    {
        dom.detach(node);
    }
}

/// True when a fallback is in the body and is not nested in another inert
/// noscript element. Template contents are not in the document tree, so a
/// template cannot be nested in another template here.
fn is_body_fallback(dom: &Dom, body: NodeId, node: NodeId) -> bool {
    let mut in_body = false;
    for ancestor in dom.ancestors(node) {
        if dom.tag(ancestor) == Some(Tag::Noscript) {
            return false;
        }
        if ancestor == body {
            in_body = true;
        }
    }
    in_body
}

/// Measures a candidate fallback and returns its text length when it is
/// article-like prose: enough text in several prose runs, with a heading or
/// more prose, few links, and no "JavaScript required" notice.
fn substantial_fallback_length(dom: &Dom, root: NodeId) -> Option<usize> {
    let prose = prose_stats(dom, root, PROSE_RUN_CHARS);
    let substantial = prose.text_length >= FALLBACK_TEXT_CHARS
        && prose.prose_runs >= 2
        && (prose.headings > 0 || prose.prose_runs >= 3)
        && prose.link_text_length * 2 < prose.text_length;
    if !substantial {
        return None;
    }
    if prose.text_length < NOTICE_TEXT_CHARS {
        let mut text = String::new();
        get_inner_text_limited(dom, root, &mut text, NOTICE_TEXT_CHARS * 2);
        if is_javascript_notice(&text) {
            return None;
        }
    }
    Some(prose.text_length)
}

/// One item of a [`ChromeWalk`].
pub(crate) enum ChromeItem<'a> {
    /// An element that is not set aside. Its descendants follow.
    Element(NodeId),
    /// An individual control. Its subtree is set aside.
    Control,
    /// A progress indicator without meaningful text. Its subtree is set aside.
    Indicator,
    /// A loading placeholder text node.
    Placeholder,
    /// Any other text node, including whitespace.
    Text(&'a str),
    /// A visible media element, even inside a control or an indicator. Its
    /// subtree is not walked. Media is content.
    Media,
}

/// Preorder walk that separates application chrome from other content.
///
/// The text of controls and progress indicators without meaningful text is
/// set aside, but the walk still enters them to report media, such as an
/// image inside a button. A progress indicator with text, and a busy region,
/// are ordinary elements: their text is still content. Each indicator's text
/// is measured with a bounded scan. An indicator inside an indicator that has
/// text inherits that result and is not measured again, so the walk stays
/// linear for nested indicators.
pub(crate) struct ChromeWalk<'a> {
    dom: &'a Dom,
    root: NodeId,
    next: Option<NodeId>,
    visible_page: bool,
    /// The node after the subtree of the outermost indicator with text.
    texted_indicator_end: Option<Option<NodeId>>,
    /// The node after the subtree of the outermost set-aside control or
    /// indicator. Inside it, the walk reports only media.
    set_aside_end: Option<Option<NodeId>>,
}

impl<'a> ChromeWalk<'a> {
    /// Walks the descendants of `root`. With `visible_page`, the walk also
    /// skips statically hidden, script, style, noscript, and template
    /// subtrees, and page-level header, nav, and footer landmarks.
    pub(crate) fn new(dom: &'a Dom, root: NodeId, visible_page: bool) -> Self {
        Self {
            dom,
            root,
            next: dom.first_child(root),
            visible_page,
            texted_indicator_end: None,
            set_aside_end: None,
        }
    }

    fn advance(&mut self, node: NodeId, enter: bool) {
        self.next = self.dom.next_in_preorder(self.root, node, enter);
        if self.texted_indicator_end == Some(self.next) {
            self.texted_indicator_end = None;
        }
        if self.set_aside_end == Some(self.next) {
            self.set_aside_end = None;
        }
    }

    /// Sets aside the text of `node` and enters it to report media.
    fn set_aside(&mut self, node: NodeId) {
        self.set_aside_end = Some(self.dom.next_in_preorder(self.root, node, false));
        self.advance(node, true);
    }
}

impl<'a> Iterator for ChromeWalk<'a> {
    type Item = ChromeItem<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let dom = self.dom;
        loop {
            let node = self.next?;
            if let Some(text) = dom.text_node(node) {
                // Read the state before `advance`, which can leave the
                // set-aside subtree.
                let set_aside = self.set_aside_end.is_some();
                self.advance(node, false);
                if set_aside {
                    continue;
                }
                return Some(match loading_placeholder(dom, node) {
                    Some(_) => ChromeItem::Placeholder,
                    None => ChromeItem::Text(text),
                });
            }
            if !dom.is_element(node) {
                self.advance(node, false);
                continue;
            }
            if self.visible_page
                && (matches!(
                    dom.tag(node),
                    Some(Tag::Script | Tag::Style | Tag::Noscript | Tag::Template)
                ) || has_static_hidden_marker(dom, node)
                    || dom.attr(node, AttrName::AriaHidden) == Some("true")
                    || is_page_chrome(dom, node))
            {
                self.advance(node, false);
                continue;
            }
            if is_media(dom, node, self.set_aside_end.is_some()) {
                self.advance(node, false);
                return Some(ChromeItem::Media);
            }
            if self.set_aside_end.is_some() {
                self.advance(node, true);
                continue;
            }
            if is_control(dom, node) {
                self.set_aside(node);
                return Some(ChromeItem::Control);
            }
            if self.texted_indicator_end.is_none() && is_progress_indicator(dom, node) {
                if is_textless_indicator(dom, node) {
                    self.set_aside(node);
                    return Some(ChromeItem::Indicator);
                }
                self.texted_indicator_end = Some(dom.next_in_preorder(self.root, node, false));
            }
            self.advance(node, true);
            return Some(ChromeItem::Element(node));
        }
    }
}

/// True for a media element that is visible content.
///
/// An `<svg>` is media when it has an accessible name. Outside a set-aside
/// control or indicator, a static width or height of at least 64 pixels
/// also makes it media. A small unnamed icon, such as a spinner, is not.
fn is_media(dom: &Dom, node: NodeId, in_set_aside: bool) -> bool {
    match dom.tag(node) {
        Some(
            Tag::Img
            | Tag::Picture
            | Tag::Video
            | Tag::Audio
            | Tag::Iframe
            | Tag::Canvas
            | Tag::Object
            | Tag::Embed,
        ) => true,
        // `Tag::Svg` covers every SVG-namespace element. Judge only the
        // `<svg>` root; its parts belong to it.
        Some(Tag::Svg) => {
            dom.qual_name(node)
                .is_some_and(|name| name.local.as_str() == "svg")
                && (has_accessible_name(dom, node) || !in_set_aside && !is_declared_icon(dom, node))
        }
        _ => false,
    }
}

/// True when an element has an accessible name or image role: a non-empty
/// `aria-label` or `aria-labelledby`, `role="img"`, or a `<title>` or
/// `<desc>` child.
fn has_accessible_name(dom: &Dom, node: NodeId) -> bool {
    [
        dom.attr(node, AttrName::AriaLabel),
        dom.attr_by_local_name(node, "aria-labelledby"),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.trim().is_empty())
        || dom
            .attr(node, AttrName::Role)
            .is_some_and(|role| has_token(role, "img"))
        || dom.element_children(node).any(|child| {
            dom.qual_name(child)
                .is_some_and(|name| matches!(name.local.as_str(), "title" | "desc"))
                && dom.has_non_whitespace_text(child)
        })
}

/// True when both static `width` and `height` attributes declare an icon
/// size of at most 32 pixels. An SVG with any other sizing, including CSS
/// or no size at all, can be a chart or diagram, so it counts as media. This
/// default keeps the reveal gate conservative.
fn is_declared_icon(dom: &Dom, node: NodeId) -> bool {
    [AttrName::Width, AttrName::Height].into_iter().all(|name| {
        dom.attr(node, name).is_some_and(|value| {
            let value = value.trim();
            let value = value.strip_suffix("px").unwrap_or(value);
            value.parse::<u32>().is_ok_and(|size| size <= 32)
        })
    })
}

/// True for a page-level header, nav, or footer landmark: one that is not
/// inside main content. Its text, such as a "Home" link, is page chrome.
fn is_page_chrome(dom: &Dom, node: NodeId) -> bool {
    let landmark = matches!(dom.tag(node), Some(Tag::Header | Tag::Nav | Tag::Footer))
        || dom.attr(node, AttrName::Role).is_some_and(|role| {
            ["banner", "navigation", "contentinfo"]
                .iter()
                .any(|name| has_token(role, name))
        });
    landmark
        && !dom.ancestors(node).any(|ancestor| {
            matches!(
                dom.tag(ancestor),
                Some(Tag::Main | Tag::Article | Tag::Section)
            ) || dom
                .attr(ancestor, AttrName::Role)
                .is_some_and(|role| has_token(role, "main"))
        })
}

/// True when the visible body is an application shell. Apart from loading
/// placeholders, textless progress indicators, individual controls, and
/// page-level header, nav, and footer landmarks, the visible body has no
/// text and no visible media. Any other visible text or media makes it a
/// normal page, whatever other evidence it has. A shell must also show
/// application evidence: progress, a loading placeholder with a control, or
/// a framework mount node. An empty page without such evidence is not a
/// shell, so an unrelated template on an empty or image-only page stays
/// inert. The scan stops at the first other text or media.
fn is_shell(dom: &Dom, body: NodeId) -> bool {
    let mut placeholder = false;
    let mut progress = false;
    let mut control = false;
    let mut mount = false;
    for item in ChromeWalk::new(dom, body, true) {
        match item {
            ChromeItem::Media => return false,
            ChromeItem::Element(node) => {
                progress |= is_busy_region(dom, node) || is_progress_indicator(dom, node);
                mount |= is_mount_node(dom, node);
            }
            ChromeItem::Control => control = true,
            ChromeItem::Indicator => progress = true,
            ChromeItem::Placeholder => placeholder = true,
            ChromeItem::Text(text) => {
                if !text.trim().is_empty() {
                    return false;
                }
            }
        }
    }
    progress || placeholder && control || mount
}

/// True for the conventional mount element of a client framework, or for a
/// container whose ID has an `app` or `mount` part, such as `app-container`
/// or `mount_point`. The caller decides whether the mount holds content.
fn is_mount_node(dom: &Dom, node: NodeId) -> bool {
    const MOUNT_IDS: &[&str] = &[
        "root",
        "app",
        "app-root",
        "application",
        "__next",
        "__nuxt",
        "___gatsby",
        "svelte",
        "react-root",
    ];
    let id = dom.attr(node, AttrName::Id);
    let class = dom.attr(node, AttrName::Class);
    let named = |needle| {
        id.is_some_and(|id| any_token_contains(id, needle))
            || class.is_some_and(|class| any_token_contains(class, needle))
    };
    id.is_some_and(|id| MOUNT_IDS.iter().any(|mount| id.eq_ignore_ascii_case(mount)))
        || named("app-shell")
        || named("application-shell")
        || class.is_some_and(|class| has_token(class, "ember-application"))
        || matches!(dom.tag(node), Some(Tag::Div | Tag::Main | Tag::Section))
            && id.is_some_and(|id| {
                id.split(['-', '_']).any(|part| {
                    ["app", "mount"]
                        .iter()
                        .any(|name| part.eq_ignore_ascii_case(name))
                })
            })
}

fn is_javascript_notice(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("javascript")
        && [
            "enable",
            "disabled",
            "required",
            "requires",
            "turn on",
            "turned off",
            "support",
            "activate",
        ]
        .iter()
        .any(|word| lower.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepare(html: &str) -> Dom {
        let mut dom = Dom::parse_document(html).unwrap();
        let body = dom.body();
        crate::cleaning::prep_document_with_body(&mut dom, body);
        dom
    }

    fn body_text(dom: &Dom) -> String {
        let mut text = String::new();
        dom.append_normalized_text(dom.body().unwrap(), &mut text);
        text
    }

    const ARTICLE: &str = "<main><h1>Recovery guide</h1>\
        <p>Stop writes before the snapshot is attached to the new volume.</p>\
        <p>Verify the restored checksum before traffic resumes on the replica.</p>\
        <p>Keep the old volume for one week before you delete it.</p></main>";

    #[test]
    fn reveals_noscript_article_in_an_empty_shell() {
        let dom = prepare(&format!(
            r#"<body><div id="root">Loading…</div><noscript>{ARTICLE}</noscript></body>"#
        ));
        assert!(body_text(&dom).contains("Verify the restored checksum"));
        assert!(
            dom.first_descendant_by_tag(dom.root(), Tag::Noscript)
                .is_none()
        );
    }

    #[test]
    fn reveals_template_article_and_removes_its_scripts() {
        let dom = prepare(&format!(
            r#"<body><div id="app"><p>Loading guide</p></div><template>{ARTICLE}<script>run()</script></template></body>"#
        ));
        let text = body_text(&dom);
        assert!(text.contains("Stop writes before the snapshot"));
        assert!(!text.contains("run()"));
        assert!(
            dom.first_descendant_by_tag(dom.root(), Tag::Template)
                .is_none()
        );
        assert!(
            dom.first_descendant_by_tag(dom.root(), Tag::Script)
                .is_none()
        );
    }

    #[test]
    fn keeps_fallbacks_hidden_on_normal_pages() {
        let visible = "<article><h1>Visible article</h1><p>The visible article has enough text to be the page content on its own, with several complete sentences.</p><p>A second visible paragraph explains the details of the subject for the reader.</p></article>";
        for fallback in [
            format!("<noscript>{ARTICLE}</noscript>"),
            format!("<template>{ARTICLE}</template>"),
        ] {
            let dom = prepare(&format!("<body>{visible}{fallback}</body>"));
            assert!(!body_text(&dom).contains("Stop writes before the snapshot"));
        }
    }

    #[test]
    fn rejects_notices_widgets_and_pixels_in_a_shell() {
        for fallback in [
            r#"<noscript><img src="https://tracker.example/pixel.gif"></noscript>"#,
            "<noscript><h2>JavaScript required</h2><p>Please enable JavaScript in your browser settings to use this application.</p><p>After you enable it, reload the page to continue to your workspace.</p></noscript>",
            r#"<template><div class="toast"><button>Undo</button><span>Item moved</span></div></template>"#,
            r#"<template><ul><li><a href="/a">Account settings and preferences</a></li><li><a href="/b">Billing history and invoices</a></li><li><a href="/c">Team members and their roles</a></li><li><a href="/d">Security keys and sessions</a></li></ul><h2>Menu</h2></template>"#,
        ] {
            let dom = prepare(&format!(
                r#"<body><div id="root">Loading…</div>{fallback}</body>"#
            ));
            let text = body_text(&dom);
            assert!(!text.contains("enable JavaScript"), "{fallback}");
            assert!(!text.contains("Item moved"), "{fallback}");
            assert!(!text.contains("Billing history"), "{fallback}");
        }
    }

    #[test]
    fn short_pages_without_application_evidence_keep_templates_inert() {
        let dom = prepare(&format!(
            "<body><main><p>Open weekdays, 9 AM to 5 PM.</p></main><template>{ARTICLE}</template></body>"
        ));
        assert!(!body_text(&dom).contains("Stop writes before the snapshot"));
        for visible in [
            "<p>Loading <b>dock closed today.</b></p>",
            "<p>Loading dock</p>",
        ] {
            let dom = prepare(&format!(
                "<body>{visible}<template>{ARTICLE}</template></body>"
            ));
            assert!(
                !body_text(&dom).contains("Stop writes before the snapshot"),
                "{visible}"
            );
        }
        for visible in [
            r#"<div id="__next"><main><p>Open weekdays, 9 AM to 5 PM.</p></main></div>"#,
            r#"<div id="root"><p>Please wait outside.</p><button>Check in</button></div>"#,
            r#"<div id="__next" aria-busy="true"><main><p>Open weekdays, 9 AM to 5 PM.</p></main></div>"#,
            r#"<div id="root"><div class="skeleton-article"><p>Open weekdays, 9 AM to 5 PM.</p></div></div>"#,
        ] {
            let dom = prepare(&format!(
                "<body>{visible}<template>{ARTICLE}</template></body>"
            ));
            assert!(
                !body_text(&dom).contains("Stop writes before the snapshot"),
                "{visible}"
            );
        }
        let dom = prepare(&format!(
            r#"<body><nav>Home</nav><div id="__next"><div class="spinner"></div></div><template>{ARTICLE}</template></body>"#
        ));
        assert!(body_text(&dom).contains("Stop writes before the snapshot"));
        let dom = prepare(&format!(
            "<body><p>Loading guide</p><button>Retry</button><template>{ARTICLE}</template></body>"
        ));
        assert!(body_text(&dom).contains("Stop writes before the snapshot"));
        let dom = prepare(&format!(
            r#"<body><div id="fb-root"></div><div id="whatsapp-widget"></div><p>Short note.</p><template>{ARTICLE}</template></body>"#
        ));
        assert!(!body_text(&dom).contains("Stop writes before the snapshot"));
        // Any other visible text is a normal page, whatever the evidence.
        for visible in [
            r#"<div id="app-mount"></div><p>Short note.</p>"#,
            r#"<div id="__next" aria-busy="true"><p>Closed today.</p></div>"#,
            r#"<div id="root"><div class="spinner"></div><span>Hi</span></div>"#,
        ] {
            let dom = prepare(&format!(
                "<body>{visible}<template>{ARTICLE}</template></body>"
            ));
            assert!(
                !body_text(&dom).contains("Stop writes before the snapshot"),
                "{visible}"
            );
        }
        // Page-level chrome does not count as other text.
        let dom = prepare(&format!(
            r#"<body><header><a href="/">Home</a></header><div id="app-mount"></div><footer>© Example</footer><template>{ARTICLE}</template></body>"#
        ));
        assert!(body_text(&dom).contains("Stop writes before the snapshot"));
    }

    #[test]
    fn forms_and_texted_indicators_keep_their_text_visible() {
        for visible in [
            r#"<div id="__next"><form><main><p>Open weekdays, 9 AM to 5 PM.</p></main></form></div>"#,
            r#"<div id="root"><div class="spinner"><div class="loader"><p>Open weekdays, 9 AM to 5 PM.</p></div></div></div>"#,
            r#"<div id="root"><div role="progressbar"><span class="spinner">Ready</span><p>Open weekdays, 9 AM to 5 PM.</p></div></div>"#,
        ] {
            let dom = prepare(&format!(
                "<body>{visible}<template>{ARTICLE}</template></body>"
            ));
            assert!(
                !body_text(&dom).contains("Stop writes before the snapshot"),
                "{visible}"
            );
        }
        // A short indicator is set aside, so the empty mount is a shell.
        let dom = prepare(&format!(
            r#"<body><div id="root"><div class="spinner"><span class="loader">40%</span></div></div><template>{ARTICLE}</template></body>"#
        ));
        assert!(body_text(&dom).contains("Stop writes before the snapshot"));
    }

    #[test]
    fn indicators_match_whole_name_parts_and_symbolic_text() {
        let dom = Dom::parse_document(
            r#"<body><div id="a" class="file-uploader"></div><div id="b" class="loading-spinner"></div><div id="c" class="page_loader x"></div><div id="d" class="progressive"></div><div id="e" class="spinner">40%</div><div id="f" class="spinner">3/10 …</div><div id="g" class="spinner">Ready</div><div id="h" class="spinner"><label>%</label></div><h2 id="i" class="progress">2 / 3</h2><label id="j" class="spinner">%</label></body>"#,
        )
        .unwrap();
        let node = |id: &str| {
            dom.descendants(dom.root())
                .find(|&node| dom.attr(node, AttrName::Id) == Some(id))
                .unwrap()
        };
        assert!(!is_progress_indicator(&dom, node("a")));
        assert!(is_progress_indicator(&dom, node("b")));
        assert!(is_progress_indicator(&dom, node("c")));
        assert!(!is_progress_indicator(&dom, node("d")));
        assert!(is_textless_indicator(&dom, node("e")));
        assert!(is_textless_indicator(&dom, node("f")));
        assert!(!is_textless_indicator(&dom, node("g")));
        assert!(!is_textless_indicator(&dom, node("h")));
        // The indicator element itself can be a heading or a label.
        assert!(!is_textless_indicator(&dom, node("i")));
        assert!(!is_textless_indicator(&dom, node("j")));
        for visible in [
            r#"<main><div class="file-uploader"><h2>Photos</h2><input type="file"></div></main>"#,
            r#"<div id="root"><div class="spinner"><h2>Photos</h2></div></div>"#,
            r#"<div id="root"><h2 class="progress">2 / 3</h2></div>"#,
        ] {
            let dom = prepare(&format!(
                "<body>{visible}<template>{ARTICLE}</template></body>"
            ));
            assert!(
                !body_text(&dom).contains("Stop writes before the snapshot"),
                "{visible}"
            );
        }
    }

    #[test]
    fn media_inside_set_aside_chrome_is_content() {
        for visible in [
            r#"<main id="root"><button><img src="/photo.jpg" alt=""></button></main>"#,
            r#"<div id="root"><div class="spinner"><img src="/photo.jpg" alt=""></div></div>"#,
            r#"<div id="root"><p>Loading…</p><button><video src="/clip.mp4"></video></button></div>"#,
        ] {
            let dom = prepare(&format!(
                "<body>{visible}<template>{ARTICLE}</template></body>"
            ));
            assert!(
                !body_text(&dom).contains("Stop writes before the snapshot"),
                "{visible}"
            );
        }
        // Text inside a control stays set aside, and a spinner icon is not media.
        let dom = prepare(&format!(
            r#"<body><div id="root"><button>Sign in</button><div class="spinner"><svg viewBox="0 0 1 1"></svg></div></div><template>{ARTICLE}</template></body>"#
        ));
        assert!(body_text(&dom).contains("Stop writes before the snapshot"));
    }

    #[test]
    fn svgs_are_media_unless_declared_icons() {
        for visible in [
            r#"<div id="root"><svg role="img" aria-label="Annual revenue chart"><rect width="1" height="1"/></svg></div>"#,
            r#"<div id="root"><svg style="width:800px;height:600px"><rect width="1" height="1"/></svg></div>"#,
            r#"<div id="root"><svg viewBox="0 0 800 600"><rect width="1" height="1"/></svg></div>"#,
            r#"<div id="root"><svg width="24"><path d="M0 0"/></svg></div>"#,
            r#"<div id="root"><svg><title>Revenue</title><rect width="1" height="1"/></svg></div>"#,
            r#"<div id="root"><svg width="800" height="600"><rect width="1" height="1"/></svg></div>"#,
            r#"<div id="root"><div class="spinner"><svg aria-label="Revenue"></svg></div></div>"#,
        ] {
            let dom = prepare(&format!(
                "<body>{visible}<template>{ARTICLE}</template></body>"
            ));
            assert!(
                !body_text(&dom).contains("Stop writes before the snapshot"),
                "{visible}"
            );
        }
        for visible in [
            r#"<div id="root"><div class="spinner"><svg width="120" height="120"></svg></div></div>"#,
            r#"<div id="root"><svg width="24" height="24"><path d="M0 0"/></svg></div>"#,
            r#"<div id="root"><svg width="16px" height="16px"><path d="M0 0"/></svg></div>"#,
        ] {
            let dom = prepare(&format!(
                "<body>{visible}<template>{ARTICLE}</template></body>"
            ));
            assert!(
                body_text(&dom).contains("Stop writes before the snapshot"),
                "{visible}"
            );
        }
    }

    #[test]
    fn deeply_nested_indicators_are_measured_once() {
        // Build the tree directly. The HTML parser is not linear in depth.
        const DEPTH: usize = 50_000;
        let mut dom = Dom::parse_document("<body></body>").unwrap();
        let body = dom.body().unwrap();
        let mut parent = body;
        for _ in 0..DEPTH {
            let spinner = dom.create_html_element(Tag::Div).unwrap();
            dom.set_attr(spinner, AttrName::Class, "spinner");
            dom.append_child(parent, spinner);
            parent = spinner;
        }
        let text = dom.create_text("Open weekdays, 9 AM to 5 PM.").unwrap();
        dom.append_child(parent, text);
        let mut elements = 0;
        let mut texts = 0;
        for item in ChromeWalk::new(&dom, body, true) {
            match item {
                ChromeItem::Element(_) => elements += 1,
                ChromeItem::Text(_) => texts += 1,
                _ => panic!("a texted indicator must not be set aside"),
            }
        }
        assert_eq!((elements, texts), (DEPTH, 1));
        assert!(!is_shell(&dom, body));
    }

    #[test]
    fn reveals_only_the_largest_fallback() {
        let dom = prepare(&format!(
            r#"<body><div id="root"></div><noscript>{ARTICLE}</noscript><template>{ARTICLE}</template></body>"#
        ));
        assert_eq!(body_text(&dom).matches("Stop writes").count(), 1);
    }

    #[test]
    fn splices_streamed_segments_into_placeholders() {
        let dom = prepare(
            r#"<body><main><p>Intro text.</p><!--$?--><template id="B:0"></template><p>Loading steps</p><!--/$--><p>Outro text.</p></main><div hidden id="S:0"><p>Streamed step text.</p></div></body>"#,
        );
        assert_eq!(body_text(&dom), "Intro text.Streamed step text.Outro text.");
        assert!(
            dom.first_descendant_by_tag(dom.root(), Tag::Template)
                .is_none()
        );
    }

    #[test]
    fn splices_many_streamed_segments_by_key() {
        const COUNT: usize = 5_000;
        let mut html = String::from("<body><main>");
        for index in 0..COUNT {
            html.push_str(&format!(
                r#"<!--$?--><template id="B:{index:x}"></template><p>Wait {index}</p><!--/$-->"#
            ));
        }
        html.push_str("</main>");
        // Emit segments in reverse order so each match is far from the start.
        for index in (0..COUNT).rev() {
            html.push_str(&format!(
                r#"<div hidden id="S:{index:x}"><p>Part {index}.</p></div>"#
            ));
        }
        html.push_str("</body>");
        let dom = prepare(&html);
        let main = dom.first_descendant_by_tag(dom.root(), Tag::Main).unwrap();
        let parts: Vec<String> = dom
            .children(main)
            .filter(|&node| dom.tag(node) == Some(Tag::P))
            .map(|node| dom.text(node))
            .collect();
        assert_eq!(parts.len(), COUNT);
        for (index, part) in parts.iter().enumerate() {
            assert_eq!(part, &format!("Part {index}."));
        }
        assert!(
            dom.first_descendant_by_tag(dom.root(), Tag::Template)
                .is_none()
        );
    }

    #[test]
    fn keeps_fallbacks_of_incomplete_boundaries() {
        // S:0 refers to P:1, which never arrived.
        let dom = prepare(
            r#"<body><main><!--$?--><template id="B:0"></template><p>Fallback text.</p><!--/$--></main><div hidden id="S:0"><p>Partial.</p><template id="P:1"></template></div></body>"#,
        );
        let main = dom.first_descendant_by_tag(dom.root(), Tag::Main).unwrap();
        let mut text = String::new();
        dom.append_normalized_text(main, &mut text);
        assert_eq!(text, "Fallback text.");
        assert!(dom.first_descendant_by_tag(main, Tag::Template).is_some());
        // S:0 refers to S:1, which refers back to S:0.
        let dom = prepare(
            r#"<body><main><!--$?--><template id="B:0"></template><p>Fallback text.</p><!--/$--></main><div hidden id="S:0"><p>Part.</p><template id="P:1"></template></div><div hidden id="S:1"><template id="P:0"></template></div></body>"#,
        );
        assert!(body_text(&dom).contains("Fallback text."));
        // A complete nested chain replaces the fallback.
        let dom = prepare(
            r#"<body><main><!--$?--><template id="B:0"></template><p>Fallback text.</p><!--/$--></main><div hidden id="S:0"><p>Part.</p><template id="P:1"></template></div><div hidden id="S:1"><p>Nested.</p></div></body>"#,
        );
        assert_eq!(body_text(&dom), "Part.Nested.");
    }

    #[test]
    fn long_segment_chains_resolve_without_recursion() {
        const COUNT: usize = 20_000;
        let mut html = String::from(
            r#"<body><main><!--$?--><template id="B:0"></template><p>Fallback text.</p><!--/$--></main>"#,
        );
        for index in 0..COUNT {
            html.push_str(&format!(
                r#"<div hidden id="S:{index:x}"><template id="P:{:x}"></template></div>"#,
                index + 1
            ));
        }
        html.push_str(&format!(
            r#"<div hidden id="S:{COUNT:x}"><p>End.</p></div></body>"#
        ));
        let dom = prepare(&html);
        assert_eq!(body_text(&dom), "End.");
    }

    #[test]
    fn keeps_unmatched_or_cyclic_streamed_segments_in_place() {
        let dom = prepare(
            r#"<body><main><template id="B:1"></template><p>Fallback text.</p></main><div hidden id="S:2"><p>Other segment.</p></div><div hidden id="S:3"><template id="P:3"></template></div></body>"#,
        );
        let text = body_text(&dom);
        assert!(text.contains("Fallback text."));
        assert!(
            dom.first_descendant_by_tag(dom.root(), Tag::Template)
                .is_some()
        );
    }

    #[test]
    fn recognizes_loading_placeholders_by_wording() {
        for text in [
            "Loading workspace",
            "Loading…",
            "loading.",
            "Please wait",
            "Workspace loading",
            "Client renderer starting",
            "Loading saved reports...",
            "Please wait while the workspace loads",
            "Loading recovery guide",
        ] {
            assert!(loading_placeholder_form(text).is_some(), "{text}");
        }
        for text in [
            "Loaders and unloaders",
            "Loading dock safety rules for every warehouse shift team",
            "Loading dock closed today.",
            "Please wait outside.",
            "One moment of silence",
            "Loading the dataset now",
            "Download the export",
            "Loading complete",
            ", .",
            "",
        ] {
            assert!(loading_placeholder_form(text).is_none(), "{text}");
        }
    }
}
