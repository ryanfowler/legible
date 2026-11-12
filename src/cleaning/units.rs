//! Removal of small, self-contained peripheral units inside the selected
//! fragment: pagination controls, disclosed promotion units, terminal
//! recommendation cards, and standalone purchase calls to action.
//!
//! Each rule needs structure and explicit evidence together. A word in prose
//! is never sufficient. A unit must be small, must not contain substantive
//! prose, and must not be most of the fragment.

use super::{
    detach_and_invalidate_stats, is_pagination_label, is_protected_content, name_has_token,
    node_name,
};
use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, Tag};
use crate::scoring::{get_link_density_cached, get_or_compute_stats};
use crate::tokens::{contains_any, has_token};
use smallvec::SmallVec;

/// The maximum number of nodes that a classifier inspects in one unit.
/// Peripheral units are small. A larger subtree is never removed here.
const UNIT_NODE_LIMIT: usize = 160;

/// The maximum visible text length of a removable promotion unit.
const UNIT_TEXT_LIMIT: u32 = 400;

/// The maximum number of bytes that the line collector keeps for one line.
/// A longer line is never a label or a credit.
const LINE_BYTE_LIMIT: usize = 256;

/// Removes pagination controls, disclosed promotion units, terminal
/// recommendation cards, and purchase call-to-action blocks from `root`.
///
/// `snapshot` must be a current preorder element snapshot of `root`. The
/// function decides on every node before it detaches any node. It returns
/// true when it changed the DOM.
pub(super) fn remove_peripheral_units(
    dom: &mut Dom,
    root: NodeId,
    snapshot: &[(NodeId, u32)],
    store: &mut NodeStateStore,
    evidence: &crate::document::SourceEvidence,
) -> bool {
    let root_length = get_or_compute_stats(dom, root, store).text_length;
    if root_length == 0 {
        return false;
    }
    let mut removals = Vec::new();
    let mut removed_depth = None;
    let mut label = String::new();
    for &(node, depth) in snapshot {
        if removed_depth.is_some_and(|removed| depth > removed) {
            continue;
        }
        removed_depth = None;
        if node == root || !is_candidate_tag(dom, node) {
            continue;
        }
        let stats = get_or_compute_stats(dom, node, store);
        if stats.text_length == 0 || stats.text_length.saturating_mul(2) >= root_length {
            continue;
        }
        // Every removal path keeps pricing content. Each classifier accepts
        // only short units, so the limited text is the complete unit text.
        // Page numbers are the content of pagination, so pagination keeps
        // only recognized prices. A promotion or purchase unit keeps any
        // number, because a price can use a format that price detection
        // does not know, such as "Rs. 299" or "299 kr".
        // Media can be a described photo in a sponsored update, so a
        // promotion or purchase unit with media is kept.
        let removal = if is_pagination_control(dom, node, stats.text_length) {
            unit_text(dom, node, &mut label)
                .is_some_and(|text| !is_price_text(text))
                .then_some(Removal::Pagination)
        } else if is_disclosed_unit(dom, node, store, &mut label)
            || is_purchase_call_to_action(dom, node, store)
        {
            (unit_text(dom, node, &mut label)
                .is_some_and(|text| !text.chars().any(char::is_numeric))
                && !has_media(dom, node))
            .then_some(Removal::Promotion)
        } else {
            None
        };
        if let Some(removal) = removal
            && !has_protected_content(dom, node, evidence)
        {
            removals.push((node, removal));
            removed_depth = Some(depth);
        }
    }
    keep_promotion_majorities(dom, &mut removals, &mut label, store);
    for &(node, _) in &removals {
        detach_and_invalidate_stats(dom, node, store);
    }
    !removals.is_empty()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Removal {
    Pagination,
    Promotion,
}

/// Smallest number of text-bearing siblings that forms a list for
/// [`keep_promotion_majorities`]. A lone wrapper around one call to action is
/// not a list.
const MIN_LIST_ITEMS: usize = 3;

/// Keeps promotion units when disclosed items are more than half of a list
/// of at least [`MIN_LIST_ITEMS`] text-bearing items. The items of a list are
/// the siblings with the tag of the promotion unit. A list in which
/// most items are sponsored, such as a program of sponsored talks, is the
/// page content, not a promotion inside it. A sibling counts when it carries
/// a disclosure, whatever its size, media, or other content.
fn keep_promotion_majorities(
    dom: &Dom,
    removals: &mut Vec<(NodeId, Removal)>,
    buffer: &mut String,
    store: &mut NodeStateStore,
) {
    // A cohort is a parent and the tag of its repeated items. Headings and
    // other children of a different tag are not list items.
    let mut cohorts: Vec<(NodeId, Option<Tag>)> = removals
        .iter()
        .filter(|&&(_, removal)| removal == Removal::Promotion)
        .filter_map(|&(node, _)| dom.parent(node).map(|parent| (parent, dom.tag(node))))
        .collect();
    cohorts.sort_unstable_by_key(|&(node, _)| node.index());
    cohorts.dedup();
    let mut majorities = Vec::new();
    for (parent, tag) in cohorts {
        let mut siblings = 0_usize;
        let mut disclosed = 0_usize;
        for child in dom.element_children(parent) {
            if dom.tag(child) != tag || get_or_compute_stats(dom, child, store).text_length == 0 {
                continue;
            }
            siblings += 1;
            disclosed += usize::from(carries_disclosure(dom, child, buffer));
        }
        if siblings >= MIN_LIST_ITEMS && disclosed.saturating_mul(2) > siblings {
            majorities.push((parent, tag));
        }
    }
    if majorities.is_empty() {
        return;
    }
    removals.retain(|&(node, removal)| {
        removal != Removal::Promotion
            || dom
                .parent(node)
                .is_none_or(|parent| !majorities.contains(&(parent, dom.tag(node))))
    });
}

/// True when `node` carries a disclosure attribute or starts with a
/// disclosure line other than an advertisement slot label. Unlike [`is_disclosed_unit`], it does not check size,
/// prose, or structure, so the list majority rule counts every disclosed
/// sibling.
fn carries_disclosure(dom: &Dom, node: NodeId, buffer: &mut String) -> bool {
    if has_disclosure_attribute(dom, node) {
        return true;
    }
    buffer.clear();
    let mut lines = 0_usize;
    let mut fits = true;
    walk_lines(dom, node, |_, text, new_line| {
        lines += usize::from(new_line);
        if lines == 1 {
            fits &= push_limited(buffer, text, DISCLOSURE_LINE_BYTES);
        }
    });
    let line = buffer.split_whitespace().collect::<Vec<_>>().join(" ");
    // An advertisement slot label is never page content, so it does not
    // count toward a list of sponsored content.
    fits && !line.eq_ignore_ascii_case("advertisement")
        && disclosure_label(&line) != Disclosure::None
}

/// Longest first line, in bytes, that [`carries_disclosure`] reads.
const DISCLOSURE_LINE_BYTES: usize = 64;

/// True when `node` contains a media element.
fn has_media(dom: &Dom, node: NodeId) -> bool {
    dom.descendants(node)
        .take(UNIT_NODE_LIMIT)
        .any(|descendant| {
            matches!(
                dom.tag(descendant),
                Some(
                    Tag::Img
                        | Tag::Picture
                        | Tag::Video
                        | Tag::Audio
                        | Tag::Iframe
                        | Tag::Canvas
                        | Tag::Object
                        | Tag::Embed
                )
            )
        })
}

fn is_candidate_tag(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(
            Tag::Article
                | Tag::Aside
                | Tag::Div
                | Tag::Li
                | Tag::Nav
                | Tag::Ol
                | Tag::P
                | Tag::Section
                | Tag::Ul
        )
    )
}

fn has_protected_content(
    dom: &Dom,
    node: NodeId,
    evidence: &crate::document::SourceEvidence,
) -> bool {
    is_protected_content(dom, node, evidence)
        || dom
            .descendants(node)
            .take(UNIT_NODE_LIMIT)
            .any(|descendant| super::is_directly_protected(dom, descendant, evidence))
}

/// Recognizes a pagination control by structure and vocabulary.
///
/// The node must be navigation, carry a pagination label or name, or contain
/// a `rel=prev` or `rel=next` link. Every text item must consist only of
/// pagination words, page numbers, and separators. The control must also show
/// a direction word, a page counter, a pagination label, or a `rel` link.
fn is_pagination_control(dom: &Dom, node: NodeId, text_length: u32) -> bool {
    if text_length > 120 {
        return false;
    }
    let navigation = dom.tag(node) == Some(Tag::Nav)
        || dom
            .attr(node, AttrName::Role)
            .is_some_and(|role| has_token(role, "navigation"));
    let labelled = dom
        .attr(node, AttrName::AriaLabel)
        .is_some_and(is_pagination_label)
        || is_pagination_label(&node_name(dom, node));
    let mut directional = false;
    let mut counter = false;
    let mut relation = false;
    for (index, descendant) in dom.descendants(node).enumerate() {
        if index >= UNIT_NODE_LIMIT {
            return false;
        }
        if let Some(text) = dom.text_node(descendant) {
            match classify_pagination_text(text) {
                Some(PaginationText { direction, count }) => {
                    directional |= direction;
                    counter |= count;
                }
                None => return false,
            }
            continue;
        }
        match dom.tag(descendant) {
            Some(Tag::A) => {
                relation |= dom.attr(descendant, AttrName::Rel).is_some_and(|rel| {
                    has_token(rel, "prev") || has_token(rel, "next") || has_token(rel, "previous")
                });
            }
            Some(
                Tag::Span | Tag::Li | Tag::Ol | Tag::Ul | Tag::Div | Tag::P | Tag::Strong | Tag::B,
            )
            | Some(Tag::Em | Tag::Small | Tag::I | Tag::Button | Tag::Img | Tag::Br)
            | None => {}
            _ => return false,
        }
    }
    (navigation || labelled || relation) && (directional || counter || labelled || relation)
}

#[derive(Clone, Copy)]
struct PaginationText {
    direction: bool,
    count: bool,
}

/// Classifies one text node of a possible pagination control. Returns `None`
/// when the text contains a word outside the pagination vocabulary.
fn classify_pagination_text(text: &str) -> Option<PaginationText> {
    let mut result = PaginationText {
        direction: false,
        count: false,
    };
    let mut previous_page_word = false;
    for word in text
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let number = word.bytes().all(|byte| byte.is_ascii_digit());
        if number {
            result.count |= previous_page_word;
            previous_page_word = false;
            continue;
        }
        let lower = word.to_ascii_lowercase();
        previous_page_word = matches!(lower.as_str(), "page" | "of");
        match lower.as_str() {
            "previous" | "prev" | "next" | "newer" | "older" => result.direction = true,
            "page" | "pages" | "of" | "first" | "last" | "posts" | "entries" | "comments"
            | "replies" | "results" => {}
            _ => return None,
        }
    }
    Some(result)
}

/// Recognizes a self-contained unit that discloses promotion, or a terminal
/// recommendation card.
///
/// The unit must be a list item with a text-bearing sibling item, or a card
/// that shares a class token with a same-tag sibling. It must be short and must not contain substantive prose. A
/// promotion unit also needs an explicit disclosure: a short standalone lead
/// label, or an `aria-label` or `data-*` value on the unit. A `rel=sponsored`
/// link marks only that link, so it is not a unit disclosure. A
/// recommendation card needs a recommendation lead label, a related class or
/// ID, a terminal position among its siblings, and at least two links.
fn is_disclosed_unit(
    dom: &Dom,
    node: NodeId,
    store: &mut NodeStateStore,
    label: &mut String,
) -> bool {
    if !matches!(
        dom.tag(node),
        Some(Tag::Article | Tag::Div | Tag::Li | Tag::Section)
    ) {
        return false;
    }
    // Reject a large unit from cached statistics before any text copy.
    let stats = get_or_compute_stats(dom, node, store);
    if stats.text_length > UNIT_TEXT_LIMIT || stats.sentence_end_count > 3 {
        return false;
    }
    let name = node_name(dom, node);
    let Some(lines) = UnitLines::collect(dom, node, label) else {
        return false;
    };
    let lead = lines.label();
    // A heading names an authored section, for example "<h2>Sponsored</h2>"
    // above prose about sponsorship. It is not a disclosure line.
    let labelled = !lines.first_is_heading
        && lead.is_some_and(|text| match disclosure_label(text) {
            Disclosure::Explicit => true,
            Disclosure::Ambiguous => ["ad", "ads", "promo", "promoted", "sponsored"]
                .iter()
                .any(|token| name_has_token(&name, token)),
            Disclosure::None => false,
        });
    // A disclosure attribute is on the unit itself. A disclosure line counts
    // only when the unit owns it, so a label on a nested card does not
    // disclose a whole row.
    let attributed = has_disclosure_attribute(dom, node);
    let labelled = labelled && (attributed || owns_first_line(dom, node));
    let promotion = labelled || attributed;
    let recommendation = !promotion
        && lead.is_some_and(is_recommendation_label)
        && contains_any(&name, &["related", "recommend", "suggest"])
        && is_terminal_sibling(dom, node)
        && is_link_list(dom, node);
    // Check the sibling structure last. The label checks above are bounded
    // and reject almost every node.
    if !(promotion || recommendation) || !is_repeated_unit(dom, node) {
        return false;
    }
    // A disclosure with only a sponsor credit after it is authored text, for
    // example "Sponsored by Acme" or "Sponsored" and a linked "Acme Press"
    // above a sponsored article. Only an advertisement slot label is
    // removable without promoted content.
    let advertisement = lead.is_some_and(|text| text.eq_ignore_ascii_case("advertisement"));
    if promotion && !advertisement && lines.is_credit_only(labelled) {
        return false;
    }
    !has_substantive_prose(dom, node)
}

/// True when `unit` is a label line followed by at least two lines that hold
/// only link text, as in a recommendation card. Prose with inline links is an
/// authored answer, even under a "Related questions" label, because its
/// lines also hold text outside links.
fn is_link_list(dom: &Dom, unit: NodeId) -> bool {
    let mut lines = 0_usize;
    let mut link_lines = 0_usize;
    let mut current_is_link_only = false;
    let mut mixed = false;
    let complete = walk_lines(dom, unit, |node, text, new_line| {
        if new_line {
            if lines > 1 && current_is_link_only {
                link_lines += 1;
            }
            lines += 1;
            current_is_link_only = true;
        }
        let in_link = dom
            .ancestors(node)
            .take_while(|&ancestor| ancestor != unit)
            .any(|ancestor| dom.tag(ancestor) == Some(Tag::A));
        if !in_link && !text.trim().is_empty() {
            current_is_link_only = false;
            mixed |= lines > 1;
        }
    });
    if lines > 1 && current_is_link_only {
        link_lines += 1;
    }
    complete && !mixed && link_lines >= 2
}

/// True for a self-contained repeated unit: a list item with a text-bearing
/// sibling item, or a card that shares a class token with a same-tag sibling.
/// Plain `<section>` or `<article>` siblings without a shared class are
/// authored structure. Asides belong to the general aside cleanup.
fn is_repeated_unit(dom: &Dom, node: NodeId) -> bool {
    let tag = dom.tag(node);
    let Some(parent) = dom.parent(node) else {
        return false;
    };
    let peer = |sibling: NodeId| {
        sibling != node && dom.tag(sibling) == tag && dom.has_non_whitespace_text(sibling)
    };
    if tag == Some(Tag::Li) {
        return dom.element_children(parent).take(64).any(peer);
    }
    let Some(class) = dom.attr(node, AttrName::Class) else {
        return false;
    };
    dom.element_children(parent).take(64).any(|sibling| {
        peer(sibling)
            && dom.attr(sibling, AttrName::Class).is_some_and(|other| {
                class
                    .split_ascii_whitespace()
                    .any(|token| has_token(other, token))
            })
    })
}

/// True when the first text line of `unit` belongs to `unit` itself. Each
/// element from the line's block up to `unit` must be the only text-bearing
/// child of its parent. A leaf block directly inside `unit`, such as
/// `<li><p>Sponsored</p><h3>…</h3></li>`, is also the unit's own line. So a
/// disclosure on one card in a schedule row does not disclose the row, while
/// a card wrapper or empty advertisement slot wrappers do not hide the label.
fn owns_first_line(dom: &Dom, unit: NodeId) -> bool {
    let mut first = None;
    walk_lines(dom, unit, |node, _, _| {
        first.get_or_insert(node);
    });
    let Some(text) = first else {
        return false;
    };
    let block = block_ancestor(dom, text, unit);
    let only_text_child = |node: NodeId| {
        dom.parent(node).is_some_and(|parent| {
            // Text nodes count too: direct text beside the path is content.
            dom.children(parent)
                .take(64)
                .all(|sibling| sibling == node || !dom.has_non_whitespace_text(sibling))
        })
    };
    let is_leaf = |node: NodeId| {
        !dom.descendants(node)
            .take(UNIT_NODE_LIMIT)
            .any(|descendant| dom.tag(descendant).is_some_and(is_line_boundary))
    };
    std::iter::once(block)
        .chain(dom.ancestors(block))
        .take_while(|&node| node != unit)
        .all(|node| only_text_child(node) || dom.parent(node) == Some(unit) && is_leaf(node))
}

fn is_terminal_sibling(dom: &Dom, node: NodeId) -> bool {
    let mut sibling = dom.next_sibling(node);
    while let Some(next) = sibling {
        if dom.has_non_whitespace_text(next) {
            return false;
        }
        sibling = dom.next_sibling(next);
    }
    true
}

/// Visits the visible text of a small unit line by line.
///
/// A line is the inline text between two block boundaries. A block element
/// start, a block element end, and `<br>` are boundaries. Inline elements such
/// as `<span>`, `<strong>`, and `<a>` do not split a line. `visit` receives
/// each non-blank text node, its text, and true when the text starts a new
/// line. Returns false, after a partial visit, for a large unit.
fn walk_lines(dom: &Dom, unit: NodeId, mut visit: impl FnMut(NodeId, &str, bool)) -> bool {
    let mut boundary = 0_u32;
    let mut current = None;
    for (index, node) in dom.descendants(unit).enumerate() {
        if index >= UNIT_NODE_LIMIT {
            return false;
        }
        if let Some(tag) = dom.tag(node) {
            if is_line_boundary(tag) {
                boundary += 1;
            }
            continue;
        }
        let Some(text) = dom.text_node(node) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let key = (block_ancestor(dom, node, unit), boundary);
        let new_line = current != Some(key);
        current = Some(key);
        visit(node, text, new_line);
    }
    true
}

/// Appends `text` up to `limit` bytes in total. Returns false when it cuts
/// the text. A cut unit is not classified, so that removal decisions never
/// depend on incomplete text.
fn push_limited(buffer: &mut String, text: &str, limit: usize) -> bool {
    let mut end = limit.saturating_sub(buffer.len()).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    buffer.push_str(&text[..end]);
    end == text.len()
}

/// The link that holds a run of text: no text yet, one link element, or
/// text outside one common link.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinkRun {
    Empty,
    One(NodeId),
    Mixed,
}

impl LinkRun {
    fn add(&mut self, link: Option<NodeId>) {
        *self = match (*self, link) {
            (Self::Empty, Some(link)) => Self::One(link),
            (Self::One(current), Some(link)) if current == link => Self::One(link),
            _ => Self::Mixed,
        };
    }
}

/// The line structure of a small unit.
struct UnitLines {
    /// The normalized first line, or empty when it is longer than a label.
    first: String,
    /// True when a heading element holds the first line.
    first_is_heading: bool,
    /// The normalized second line, when the unit has one.
    second: String,
    /// The number of non-empty lines, saturated at 3.
    count: u8,
    /// The link that holds all text, and the link that holds all text after
    /// the first line.
    links: [LinkRun; 2],
}

impl UnitLines {
    /// Collects the lines of `unit`. Returns `None` for a large unit.
    fn collect(dom: &Dom, unit: NodeId, buffer: &mut String) -> Option<Self> {
        let mut lines = Self {
            first: String::new(),
            first_is_heading: false,
            second: String::new(),
            count: 0,
            links: [LinkRun::Empty; 2],
        };
        buffer.clear();
        let mut cut = false;
        let complete = walk_lines(dom, unit, |node, text, new_line| {
            if new_line {
                lines.finish(buffer);
                if lines.count == 0 {
                    lines.first_is_heading =
                        dom.ancestors(node)
                            .take_while(|&a| a != unit)
                            .any(|ancestor| {
                                matches!(
                                    dom.tag(ancestor),
                                    Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
                                )
                            });
                }
            }
            let link = dom
                .ancestors(node)
                .take_while(|&ancestor| ancestor != unit)
                .find(|&ancestor| dom.tag(ancestor) == Some(Tag::A));
            lines.links[0].add(link);
            if lines.count > 0 {
                lines.links[1].add(link);
            }
            cut |= !push_limited(buffer, text, LINE_BYTE_LIMIT);
        });
        if !complete || cut {
            return None;
        }
        lines.finish(buffer);
        Some(lines)
    }

    fn finish(&mut self, buffer: &mut String) {
        if buffer.trim().is_empty() {
            buffer.clear();
            return;
        }
        let target = match self.count {
            0 => Some(&mut self.first),
            1 => Some(&mut self.second),
            _ => None,
        };
        if let Some(target) = target {
            for word in buffer.split_whitespace() {
                if !target.is_empty() {
                    target.push(' ');
                }
                target.push_str(word);
            }
        }
        self.count = self.count.saturating_add(1).min(3);
        buffer.clear();
    }

    /// The first line when it is short enough to be a label.
    fn label(&self) -> Option<&str> {
        (self.count > 0 && self.first.len() <= 48).then_some(self.first.as_str())
    }

    /// True when the content after a disclosure is at most one sponsor
    /// credit: one link element with any text, or one short text line.
    /// `skip_first` excludes the label line.
    fn is_credit_only(&self, skip_first: bool) -> bool {
        let (count, line, link) = if skip_first {
            (
                self.count.saturating_sub(1),
                self.second.as_str(),
                self.links[1],
            )
        } else {
            (self.count, self.first.as_str(), self.links[0])
        };
        count == 0 || matches!(link, LinkRun::One(_)) || count == 1 && is_credit_line(line)
    }
}

/// True for an element that starts and ends a line. Every other element,
/// including unknown elements, is inline.
fn is_line_boundary(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::Address
            | Tag::Article
            | Tag::Aside
            | Tag::Blockquote
            | Tag::Br
            | Tag::Caption
            | Tag::Dd
            | Tag::Details
            | Tag::Div
            | Tag::Dl
            | Tag::Dt
            | Tag::Fieldset
            | Tag::Figcaption
            | Tag::Figure
            | Tag::Footer
            | Tag::Form
            | Tag::H1
            | Tag::H2
            | Tag::H3
            | Tag::H4
            | Tag::H5
            | Tag::H6
            | Tag::Header
            | Tag::Hr
            | Tag::Li
            | Tag::Main
            | Tag::Nav
            | Tag::Ol
            | Tag::P
            | Tag::Pre
            | Tag::Section
            | Tag::Summary
            | Tag::Table
            | Tag::Tbody
            | Tag::Td
            | Tag::Tfoot
            | Tag::Th
            | Tag::Thead
            | Tag::Tr
            | Tag::Ul
    )
}

fn block_ancestor(dom: &Dom, node: NodeId, unit: NodeId) -> NodeId {
    dom.ancestors(node)
        .find(|&ancestor| ancestor == unit || dom.tag(ancestor).is_some_and(is_line_boundary))
        .unwrap_or(unit)
}

/// True for one short credit line, for example "Acme Inc.", "adidas", or
/// "Bill & Melinda Gates Foundation". The line has at most eight words and no
/// sentence boundary. A period after an initial or a common abbreviation is
/// not a sentence boundary.
fn is_credit_line(line: &str) -> bool {
    let words: SmallVec<[&str; 8]> = line.split_whitespace().take(9).collect();
    if words.is_empty() || words.len() > 8 || line.contains(['!', '?']) {
        return false;
    }
    words[..words.len() - 1].iter().all(|word| {
        let Some(stem) = word.strip_suffix('.') else {
            return true;
        };
        stem.chars().count() == 1
            || [
                "co", "corp", "dr", "inc", "jr", "llc", "ltd", "mr", "mrs", "ms", "sr", "st",
            ]
            .iter()
            .any(|abbreviation| stem.eq_ignore_ascii_case(abbreviation))
    })
}

/// The strength of a disclosure label.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Disclosure {
    /// The label names paid placement without doubt, for example
    /// "Sponsored", "Promoted post", or "Promoted by DataCo".
    Explicit,
    /// The label is also an ordinary word, for example "Promoted" in release
    /// notes. It needs a matching class or ID.
    Ambiguous,
    None,
}

/// Classifies a short standalone label.
fn disclosure_label(label: &str) -> Disclosure {
    let label = label.trim_end_matches([':', '|', '-', '·', ' ']).trim();
    if label.len() > 48 || label.contains(['.', '!', '?', ',']) {
        return Disclosure::None;
    }
    let lower = label.to_ascii_lowercase();
    match lower.as_str() {
        "ad" | "ads" | "promoted" => return Disclosure::Ambiguous,
        "advertisement" | "advertiser content" | "paid content" | "paid partnership"
        | "paid post" | "partner content" | "promoted content" | "promoted listing"
        | "promoted post" | "sponsored" | "sponsored content" | "sponsored link"
        | "sponsored links" | "sponsored post" => return Disclosure::Explicit,
        _ => {}
    }
    let sponsor_credit = ["sponsored by ", "promoted by ", "paid for by "]
        .iter()
        .any(|prefix| {
            lower
                .strip_prefix(prefix)
                .is_some_and(|sponsor| (1..=4).contains(&sponsor.split_whitespace().count()))
        });
    if sponsor_credit {
        Disclosure::Explicit
    } else {
        Disclosure::None
    }
}

fn is_recommendation_label(label: &str) -> bool {
    matches!(
        label.to_ascii_lowercase().as_str(),
        "related threads"
            | "related posts"
            | "related discussions"
            | "related questions"
            | "recommended for you"
            | "suggested threads"
            | "suggested posts"
    )
}

/// True when the unit carries a disclosure label in `aria-label` or in a
/// `data-*` attribute value.
fn has_disclosure_attribute(dom: &Dom, node: NodeId) -> bool {
    dom.attr(node, AttrName::AriaLabel)
        .is_some_and(|label| disclosure_label(label) == Disclosure::Explicit)
        || dom.attrs(node).iter().any(|attribute| {
            attribute.name.local.as_str().starts_with("data-")
                && disclosure_label(&attribute.value) == Disclosure::Explicit
        })
}

fn has_sponsored_link(dom: &Dom, node: NodeId) -> bool {
    dom.descendants(node)
        .take(UNIT_NODE_LIMIT)
        .any(|descendant| super::is_sponsored_anchor(dom, descendant))
}

fn has_substantive_prose(dom: &Dom, node: NodeId) -> bool {
    let structure = dom
        .descendants(node)
        .enumerate()
        .any(|(index, descendant)| {
            index >= UNIT_NODE_LIMIT
                || matches!(
                    dom.tag(descendant),
                    Some(Tag::H1 | Tag::Pre | Tag::Table | Tag::Blockquote)
                )
        });
    // Measure prose by line, not by paragraph element, so that text directly
    // in a list item or in a `<span>` counts the same as a `<p>`.
    let mut line = 0_usize;
    let mut long_line = false;
    let complete = walk_lines(dom, node, |_, text, new_line| {
        if new_line {
            line = 0;
        }
        line = line.saturating_add(crate::scoring::stats_for_text(text).text_length as usize);
        long_line |= line >= SUBSTANTIVE_LINE_CHARS;
    });
    structure || long_line || !complete
}

/// Number of text characters that make one line authored prose.
const SUBSTANTIVE_LINE_CHARS: usize = 160;

/// Recognizes a standalone purchase call to action, for example a block
/// that holds only "Buy now with our partner code".
///
/// The block must hold only link or button text. The text must start with a
/// purchase verb phrase. The block also needs commercial evidence: an
/// affiliate, partner, sponsor, or promotion class or ID, affiliate wording,
/// or a `rel=sponsored` link.
fn is_purchase_call_to_action(dom: &Dom, node: NodeId, store: &mut NodeStateStore) -> bool {
    if !matches!(
        dom.tag(node),
        Some(Tag::Aside | Tag::Div | Tag::P | Tag::Section)
    ) {
        return false;
    }
    let stats = get_or_compute_stats(dom, node, store);
    if stats.text_length > 80 || stats.sentence_end_count > 1 {
        return false;
    }
    let mut actions = 0_usize;
    for (index, descendant) in dom.descendants(node).enumerate() {
        if index >= 32 {
            return false;
        }
        match dom.tag(descendant) {
            Some(Tag::A | Tag::Button) => actions += 1,
            // Media can be a product image, so a block with media is not a
            // bare call to action.
            Some(Tag::Span | Tag::Strong | Tag::B) | None => {}
            _ => return false,
        }
    }
    let link_text = get_link_density_cached(dom, node, stats.text_length, store) >= 0.9
        || dom
            .descendants(node)
            .any(|descendant| dom.tag(descendant) == Some(Tag::Button));
    if !(1..=2).contains(&actions) || !link_text {
        return false;
    }
    let mut text = String::new();
    dom.append_normalized_text_limited(node, &mut text, 96);
    text.make_ascii_lowercase();
    let text = text.trim();
    let call_to_action = [
        "buy now",
        "buy it ",
        "buy on ",
        "buy from ",
        "buy at ",
        "shop now",
        "check price",
        "see price",
        "get the deal",
        "order now",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix));
    if !call_to_action {
        return false;
    }
    let name = node_name(dom, node);
    [
        "affiliate",
        "partner",
        "sponsor",
        "sponsored",
        "promo",
        "promotion",
    ]
    .iter()
    .any(|token| name_has_token(&name, token))
        || contains_any(
            text,
            &["partner code", "affiliate", "promo code", "discount code"],
        )
        || has_sponsored_link(dom, node)
}

/// Collects the text of a short unit into `buffer` and returns it. Line
/// boundaries become spaces, so "<h3>Kit</h3><p>USD 29.99</p>" reads as
/// "Kit USD 29.99". Returns `None` when the text does not fit, so the caller
/// keeps the unit.
fn unit_text<'a>(dom: &Dom, node: NodeId, buffer: &'a mut String) -> Option<&'a str> {
    buffer.clear();
    let limit = 4 * UNIT_TEXT_LIMIT as usize;
    let mut complete = true;
    walk_lines(dom, node, |_, text, new_line| {
        if new_line {
            complete &= push_limited(buffer, " ", limit);
        }
        complete &= push_limited(buffer, text, limit);
    });
    complete.then_some(buffer.as_str())
}

/// Maximum number of characters between a number and a currency symbol in
/// a price.
const PRICE_SYMBOL_WINDOW: usize = 4;

/// True when `text` shows a price: a number in any decimal script near a
/// currency symbol, or next to a three-letter uppercase currency code such
/// as "CHF". So "$29.99", "29,– €", "29.99 CHF", "29.99EUR", and
/// "CHF 29.99" are all prices.
fn is_price_text(text: &str) -> bool {
    let characters: SmallVec<[char; 128]> = text.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        if !characters[index].is_numeric() {
            index += 1;
            continue;
        }
        let start = index;
        while index < characters.len()
            && (characters[index].is_numeric()
                || matches!(characters[index], '.' | ',' | '\u{66B}' | '\u{66C}'))
        {
            index += 1;
        }
        let mut before = start;
        while before > 0 && characters[before - 1].is_whitespace() {
            before -= 1;
        }
        let mut after = index;
        while after < characters.len() && characters[after].is_whitespace() {
            after += 1;
        }
        // A currency symbol near the number is a price. The window is
        // deliberately broad: formats such as "29,– €" or "€ 29.-" put
        // marks between the number and the symbol, and a false price only
        // keeps content.
        let symbol_before = characters[start.saturating_sub(PRICE_SYMBOL_WINDOW)..start]
            .iter()
            .copied()
            .any(is_currency_symbol);
        let symbol_after = characters[index..characters.len().min(index + PRICE_SYMBOL_WINDOW)]
            .iter()
            .copied()
            .any(is_currency_symbol);
        let code_before = before >= 3
            && characters[before - 3..before]
                .iter()
                .all(char::is_ascii_uppercase)
            && (before == 3 || !characters[before - 4].is_alphanumeric());
        let code_after = characters.len() >= after + 3
            && characters[after..after + 3]
                .iter()
                .all(char::is_ascii_uppercase)
            && characters
                .get(after + 3)
                .is_none_or(|character| !character.is_alphanumeric());
        if symbol_before || symbol_after || code_before || code_after {
            return true;
        }
    }
    false
}

/// True for a character in the Unicode currency symbol category (Sc).
fn is_currency_symbol(character: char) -> bool {
    matches!(
        character,
        '$' | '\u{A2}'..='\u{A5}'
            | '\u{58F}'
            | '\u{60B}'
            | '\u{7FE}'..='\u{7FF}'
            | '\u{9F2}'..='\u{9F3}'
            | '\u{9FB}'
            | '\u{AF1}'
            | '\u{BF9}'
            | '\u{E3F}'
            | '\u{17DB}'
            | '\u{20A0}'..='\u{20C0}'
            | '\u{A838}'
            | '\u{FDFC}'
            | '\u{FE69}'
            | '\u{FF04}'
            | '\u{FFE0}'..='\u{FFE1}'
            | '\u{FFE5}'..='\u{FFE6}'
            | '\u{11FDD}'..='\u{11FE0}'
            | '\u{1E2FF}'
            | '\u{1ECB0}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(html: &str) -> String {
        let mut dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        let root = dom.element_children(root).next().unwrap();
        let snapshot = dom.element_descendants_snapshot_with_depth(root);
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        let evidence = crate::document::SourceEvidence::default();
        remove_peripheral_units(&mut dom, root, &snapshot, &mut store, &evidence);
        let mut text = String::new();
        dom.append_normalized_text(root, &mut text);
        text
    }

    const LEAD: &str = "<p>The maintainers describe the regression, the investigation, \
        and the repair in enough detail to make the thread useful for later readers \
        who arrive from a search.</p>";

    #[test]
    fn removes_navigation_pagination() {
        let text = run(&format!(
            "<main>{LEAD}<nav aria-label=\"Discussion pages\"><a>Previous page</a>\
             <span>Page 2 of 8</span><a>Next page</a></nav></main>"
        ));
        assert!(!text.contains("Page 2 of 8"), "{text}");
        assert!(text.contains("maintainers"));
    }

    #[test]
    fn keeps_page_counter_in_prose_and_titled_series_navigation() {
        let text = run(&format!(
            "<main>{LEAD}<p>This is page 2 of 8 in the report.</p>\
             <nav><a rel=\"prev\">Previous: Installing the tool</a>\
             <a rel=\"next\">Next: Configuring the tool</a></nav></main>"
        ));
        assert!(text.contains("page 2 of 8"), "{text}");
        assert!(text.contains("Configuring the tool"), "{text}");
    }

    #[test]
    fn keeps_numbered_navigation_without_pagination_evidence() {
        let text = run(&format!(
            "<main>{LEAD}<nav><a>1</a> <a>2</a> <a>3</a></nav></main>"
        ));
        assert!(text.contains("1 2 3"), "{text}");
    }

    #[test]
    fn removes_disclosed_promotion_reply() {
        let text = run(&format!(
            "<main>{LEAD}<ol><li><article><strong>Nia</strong><p>Nia measured the plan.</p>\
             </article></li><li aria-label=\"Sponsored\"><article><strong>DataCo</strong>\
             <p>Database training.</p><a href=\"/c\">Start free course</a></article></li>\
             <li><article><strong>Pavel</strong><p>Pavel removed the join.</p></article></li>\
             </ol></main>"
        ));
        assert!(!text.contains("Start free course"), "{text}");
        assert!(text.contains("Nia measured") && text.contains("Pavel removed"));
    }

    #[test]
    fn removes_promoted_lead_label_card() {
        let text = run(&format!(
            "<main>{LEAD}<ol><li><strong>Lena</strong><p>Lena shared the diagram.</p></li>\
             <li><strong>Promoted post</strong><p>Field software.</p>\
             <a href=\"/trial\">Start a trial</a></li></ol></main>"
        ));
        assert!(!text.contains("Field software"), "{text}");
        // One short line after the label is a credit, so the card stays.
        let text = run(&format!(
            "<main>{LEAD}<ol><li><strong>Lena</strong><p>Lena shared the diagram.</p></li>\
             <li><strong>Promoted post</strong><p>Field software.</p></li></ol></main>"
        ));
        assert!(text.contains("Field software"), "{text}");
    }

    #[test]
    fn keeps_reply_that_mentions_sponsorship() {
        let text = run(&format!(
            "<main>{LEAD}<ol><li><strong>Kim</strong><p>Sponsored by my employer, \
             I tested this.</p></li><li><strong>Promoted</strong><p>Sam was promoted \
             to maintainer.</p></li><li><strong>Promoted</strong> the parser API to \
             stable</li><li><strong>Sponsored</strong> the build farm</li>\
             <li><strong>Ana</strong><p>Thanks.</p></li></ol></main>"
        ));
        assert!(text.contains("Sponsored by my employer"), "{text}");
        assert!(text.contains("Sam was promoted"), "{text}");
        assert!(text.contains("parser API"), "{text}");
        assert!(text.contains("build farm"), "{text}");
    }

    #[test]
    fn keeps_single_disclosed_item_and_long_disclosed_unit() {
        let long = "word ".repeat(40);
        let text = run(&format!(
            "<main>{LEAD}<ul><li><span>Sponsored</span> only item</li></ul>\
             <section><h2>Notes</h2></section><section><span>Sponsored</span>\
             <p>{long}</p></section></main>"
        ));
        assert!(text.contains("only item"), "{text}");
        assert!(text.contains("word word"), "{text}");
    }

    #[test]
    fn keeps_label_inside_an_inline_line_and_linked_sponsor_credit() {
        let text = run(&format!(
            "<main>{LEAD}<ul class=\"sponsored\"><li><strong>Sponsored</strong><span> a public \
             guide about clear labels</span></li><li><strong>Promoted</strong><em> the parser \
             API</em></li><li>Other work</li></ul><ul><li>Peer item</li><li><span>Sponsored</span> \
             <a href=\"/acme\">Acme Press</a></li><li><p>Sponsored</p>\
             <a href=\"/acme\">Acme Press Books</a></li>\
             <li aria-label=\"Sponsored\"><a href=\"/acme\">Acme Foundation</a></li></ul></main>"
        ));
        assert!(text.contains("a public guide about clear labels"), "{text}");
        assert!(text.contains("the parser API"), "{text}");
        assert!(text.contains("Acme Press"), "{text}");
        assert!(text.contains("Acme Press Books"), "{text}");
        assert!(text.contains("Acme Foundation"), "{text}");
    }

    #[test]
    fn keeps_abbreviated_sponsor_credits() {
        let text = run(&format!(
            "<main>{LEAD}<ul><li>Peer item</li><li><p>Sponsored</p><p>Acme Inc.</p></li>\
             <li><p>Sponsored</p><p>J. R. Smith Ltd.</p></li></ul></main>"
        ));
        assert!(text.contains("Acme Inc."), "{text}");
        assert!(text.contains("J. R. Smith Ltd."), "{text}");
        let text = run(&format!(
            "<main>{LEAD}<ul><li>Peer item</li><li>Second peer</li><li>Third peer</li>\
             <li><p>Sponsored</p><p>Try it. Free today</p></li>\
             <li><p>Sponsored</p><p>Field software</p><a href=\"/x\">Start a trial</a></li>\
             </ul></main>"
        ));
        assert!(!text.contains("Free today"), "{text}");
        assert!(!text.contains("Field software"), "{text}");
    }

    #[test]
    fn keeps_lists_that_are_mostly_promotions() {
        let talk = "<li><p>Sponsored by Acme</p><h2>Building accessible maps</h2>\
                    <p>Learn how to make navigation work without sight.</p></li>";
        let text = run(&format!("<main>{LEAD}<ul>{talk}{talk}{talk}</ul></main>"));
        assert_eq!(
            text.matches("Building accessible maps").count(),
            3,
            "{text}"
        );

        // A session with long prose is not removable, but it still counts.
        let long = format!(
            "<li><p>Sponsored by Acme</p><h2>Long session</h2><p>{}</p></li>",
            "Learn how the team rebuilt the transit map for screen readers. ".repeat(3)
        );
        let text = run(&format!(
            "<main>{LEAD}<ul>{long}{talk}<li>Community lunch</li></ul></main>"
        ));
        assert!(text.contains("Building accessible maps"), "{text}");

        // A talk that another rule keeps still counts toward the majority.
        let with_photo = "<li><p>Sponsored by Acme</p><h2>Building accessible maps</h2>\
                          <img src=\"/maps.jpg\" alt=\"Tactile map\">\
                          <p>Learn how to make navigation work without sight.</p></li>";
        let text = run(&format!(
            "<main>{LEAD}<ul>{with_photo}{talk}{talk}<li>Community lunch</li></ul></main>"
        ));
        assert_eq!(
            text.matches("Building accessible maps").count(),
            3,
            "{text}"
        );
    }

    #[test]
    fn keeps_promotion_units_with_media() {
        let text = run(&format!(
            "<main>{LEAD}<ul><li>Peer item</li><li>Second peer</li><li><p>Sponsored by Acme</p>\
             <h3>The greenhouse</h3><img src=\"/greenhouse.jpg\" alt=\"The finished greenhouse\">\
             <p>Our volunteers finished the greenhouse.</p></li></ul></main>"
        ));
        assert!(
            text.contains("Our volunteers finished the greenhouse"),
            "{text}"
        );
    }

    #[test]
    fn keeps_any_unit_that_shows_a_price() {
        let text = run(&format!(
            "<main>{LEAD}<ul><li>Peer item</li><li data-label=\"Sponsored\"><p>Field kit</p>\
             <a href=\"/buy\">Buy now for $29.99</a></li><li data-label=\"Sponsored\">\
             <p>Field kit</p><p>Only 29.99 CHF this week</p></li><li data-label=\"Sponsored\">\
             <p>Field kit</p><p>Now 29.99EUR for members</p></li><li><p>Sponsored</p>\
             <h3>Trail kit</h3><p>USD 29.99</p></li><li><p>Sponsored</p><p>Desert kit</p>\
             <p>\u{FDFC} \u{661}\u{662}\u{669}</p></li></ul></main>"
        ));
        assert!(text.contains("$29.99"), "{text}");
        assert!(text.contains("29.99 CHF"), "{text}");
        assert!(text.contains("29.99EUR"), "{text}");
        assert!(text.contains("USD 29.99"), "{text}");
        assert!(text.contains("Desert kit"), "{text}");
    }

    #[test]
    fn recognizes_prices_by_currency_symbol_or_code() {
        for price in [
            "$29.99",
            "29.99 CHF",
            "29.99EUR",
            "CHF 29.99",
            "₩30000",
            "₺ 45",
            "12 ₽",
            "\u{FDFC} \u{661}\u{662}\u{669}",
            "29,\u{2013} \u{20AC}",
            "29,- \u{20AC}",
            "\u{20AC} 29.-",
        ] {
            assert!(is_price_text(&format!("Only {price} today")), "{price}");
        }
        for text in ["Page 2 of 8", "Chapter 12 ABCD", "Since 2019 we tested it"] {
            assert!(!is_price_text(text), "{text}");
        }
    }

    #[test]
    fn separates_lines_when_collecting_price_text() {
        let dom = Dom::parse_fragment(
            "<li><h3>Field kit</h3><p>USD 29.99</p></li><li><b>Kit</b>USD<br>29</li>",
            Tag::Div,
        )
        .unwrap();
        let first = dom.element_children(dom.root()).next().unwrap();
        let mut buffer = String::new();
        assert!(is_price_text(unit_text(&dom, first, &mut buffer).unwrap()));
        assert!(buffer.contains("Field kit USD 29.99"), "{buffer}");
    }

    #[test]
    fn keeps_lowercase_and_multiline_link_credits() {
        let text = run(&format!(
            "<main>{LEAD}<ul><li>Peer item</li><li><p>Sponsored</p><p>adidas</p></li>\
             <li><p>Sponsored</p><a href=\"/acme\">Acme<br>outdoor gear and more</a></li>\
             </ul></main>"
        ));
        assert!(text.contains("adidas"), "{text}");
        assert!(text.contains("outdoor gear and more"), "{text}");
    }

    #[test]
    fn ignores_asides() {
        let text = run(&format!(
            "<main>{LEAD}<aside><p>Sponsored</p><p>Field software</p>\
             <a href=\"/x\">Start a trial</a></aside></main>"
        ));
        assert!(text.contains("Start a trial"), "{text}");
    }

    #[test]
    fn keeps_camel_case_sponsor_credits() {
        let text = run(&format!(
            "<main>{LEAD}<ul><li>Peer item</li><li><p>Sponsored</p><p>eBay</p></li>\
             <li><p>Sponsored</p><p>iFixit Repair Guides</p></li></ul></main>"
        ));
        assert!(text.contains("eBay"), "{text}");
        assert!(text.contains("iFixit Repair Guides"), "{text}");
    }

    #[test]
    fn keeps_long_name_credit() {
        let text = run(&format!(
            "<main>{LEAD}<ul><li>Peer item</li><li><p>Sponsored</p>\
             <p>Bill & Melinda Gates Foundation</p></li></ul></main>"
        ));
        assert!(text.contains("Bill & Melinda Gates Foundation"), "{text}");
    }

    #[test]
    fn keeps_authored_sections_about_sponsorship() {
        let text = run(&format!(
            "<main>{LEAD}<section><h2>Sponsored</h2><p>A sponsored post is paid content \
             that a blog publishes.</p></section><section><h2>Labels</h2><p>Labels must be \
             clear.</p></section><section class=\"note\"><p>Sponsored</p><p>Paid copy \
             for a sponsor.</p></section><section class=\"tip\"><p>Readers notice \
             labels.</p></section></main>"
        ));
        assert!(text.contains("A sponsored post is paid content"), "{text}");
        assert!(text.contains("Paid copy for a sponsor"), "{text}");
        let text = run(&format!(
            "<main>{LEAD}<div class=\"card\"><h3>Sponsored</h3><p>Seed packs.</p></div>\
             <div class=\"card\"><h3>Probe A</h3><p>Stable readings.</p></div></main>"
        ));
        assert!(text.contains("Seed packs"), "{text}");
        let text = run(&format!(
            "<main>{LEAD}<div class=\"card\"><p>Sponsored</p><p>Seed packs.</p>\
             <a href=\"/seeds\">Shop seeds</a></div>\
             <div class=\"card\"><h3>Probe A</h3><p>Stable readings.</p></div></main>"
        ));
        assert!(!text.contains("Seed packs"), "{text}");
    }

    #[test]
    fn keeps_purchase_call_to_action_with_price() {
        let text = run(&format!(
            "<article>{LEAD}<p><a rel=\"sponsored\" href=\"/buy\">Check price: $29.99</a></p>\
             <p class=\"affiliate\"><a href=\"/buy\">Buy now for 25 EUR</a></p></article>"
        ));
        assert!(text.contains("$29.99"), "{text}");
        assert!(text.contains("25 EUR"), "{text}");
    }

    #[test]
    fn keeps_purchase_blocks_with_numbers_or_media() {
        let text = run(&format!(
            "<article>{LEAD}<p class=\"affiliate\"><a href=\"/buy\">Buy now for Rs. 299</a></p>\
             <p class=\"affiliate\"><a href=\"/buy\">Buy now for 299 kr</a></p>\
             <p class=\"affiliate\"><a href=\"/buy\">Buy now<img src=\"/stove.jpg\" \
             alt=\"Trail stove\"></a> today</p></article>"
        ));
        assert!(text.contains("Rs. 299"), "{text}");
        assert!(text.contains("299 kr"), "{text}");
        assert!(text.contains("today"), "{text}");
    }

    #[test]
    fn keeps_promotion_cards_with_any_number() {
        for offer in ["Buy now for Rs. 299", "Buy now for 299 kr"] {
            let text = run(&format!(
                "<main>{LEAD}<ul><li><p>Sponsored</p><p>GardenCloud plans your beds.</p>\
                 <a href=\"/try\">{offer}</a></li><li>Other work</li></ul></main>"
            ));
            assert!(text.contains(offer), "{text}");
        }
    }

    #[test]
    fn keeps_units_whose_text_does_not_fit_the_collector() {
        let spaces = " ".repeat(1_700);
        let text = run(&format!(
            "<main>{LEAD}<ul><li data-label=\"Sponsored\"><p>Field kit{spaces}</p>\
             <a href=\"/buy\">Buy now for 299 kr</a></li><li>Other work</li></ul></main>"
        ));
        assert!(text.contains("299 kr"), "{text}");

        let spaces = " ".repeat(300);
        let text = run(&format!(
            "<main>{LEAD}<ul><li><span>Sponsored{spaces} the build farm</span>\
             <p>It serves the public.</p><a href=\"/report\">Read the full report</a></li>\
             <li>Other work</li></ul></main>"
        ));
        assert!(text.contains("the build farm"), "{text}");
    }

    #[test]
    fn keeps_label_inside_unlisted_inline_elements() {
        let text = run(&format!(
            "<main>{LEAD}<ul><li><u>Sponsored</u><bdi> a public guide about labels</bdi></li>\
             <li>Other work</li></ul></main>"
        ));
        assert!(text.contains("a public guide about labels"), "{text}");
    }

    /// An "Advertisement" label with an advertiser link is an advertisement
    /// slot, not a sponsor credit. The advertiser link goes with the slot.
    #[test]
    fn removes_advertisement_label_with_advertiser_link() {
        let text = run(&format!(
            "<main><div class=\"block\">{LEAD}</div><div class=\"block\"><p>Advertisement</p>\
             <a href=\"https://ads.example/acme\">Acme Inc.</a></div></main>"
        ));
        assert!(!text.contains("Acme Inc."), "{text}");
        assert!(text.contains("maintainers"), "{text}");
    }

    #[test]
    fn skips_large_nested_units_before_collecting_lines() {
        let long = "word ".repeat(20_000);
        let depth = 120;
        let html = format!(
            "<main><p>{long}</p><p>{long}</p>{}<p>Sponsored</p><p>{long}</p>{}</main>",
            "<section>".repeat(depth),
            "</section>".repeat(depth)
        );
        let started = std::time::Instant::now();
        let text = run(&html);
        assert!(text.contains("Sponsored"));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn keeps_label_only_sponsor_credit_and_removes_advertisement_slot() {
        let text = run(&format!(
            "<main><div class=\"block\"><span>Sponsored by Acme</span></div>\
             <div class=\"block\">{LEAD}</div><div class=\"block\"><p>Advertisement</p>\
             <div class=\"slot\"></div></div></main>"
        ));
        assert!(text.contains("Sponsored by Acme"), "{text}");
        assert!(!text.contains("Advertisement"), "{text}");
    }

    #[test]
    fn removes_terminal_related_card_only_with_name_label_and_links() {
        let text = run(&format!(
            "<main>{LEAD}<ol><li><strong>Rae</strong><p>Rae found it.</p></li>\
             <li class=\"related-card\"><h3>Related threads</h3><ul>\
             <li><a href=\"/t/1\">Cache bug</a></li><li><a href=\"/t/2\">Parser bug</a></li>\
             </ul></li></ol></main>"
        ));
        assert!(!text.contains("Parser bug"), "{text}");
        let text = run(&format!(
            "<main>{LEAD}<ol><li><strong>Rae</strong><p>Rae found it.</p></li>\
             <li><h3>Related threads</h3><ul><li><a href=\"/t/1\">Cache bug</a></li>\
             <li><a href=\"/t/2\">Parser bug</a></li></ul></li></ol></main>"
        ));
        assert!(text.contains("Parser bug"), "{text}");
        let text = run(&format!(
            "<main>{LEAD}<section><h2>Notes</h2><p>Clay needs calibration.</p></section>\
             <section class=\"related-questions\"><h2>Related questions</h2><p>Can this \
             sensor measure frozen soil? No, ice causes a false dry reading.</p></section></main>"
        ));
        assert!(text.contains("frozen soil"), "{text}");
    }

    #[test]
    fn keeps_product_entries_with_sponsored_links() {
        let text = run(&format!(
            "<main>{LEAD}<ul><li><h3>Probe A</h3><p>Probe A gave stable readings in clay \
             after calibration.</p><a rel=\"sponsored\" href=\"/buy\">Check price</a></li>\
             <li><h3>Probe B</h3><p>Probe B needed a new battery.</p>\
             <a rel=\"sponsored\" href=\"/buy-b\">Check price</a></li></ul></main>"
        ));
        assert!(text.contains("Probe A gave stable readings"), "{text}");
        assert!(text.contains("Probe B needed a new battery"), "{text}");
    }

    #[test]
    fn removes_affiliate_purchase_call_to_action() {
        let text = run(&format!(
            "<article>{LEAD}<div class=\"affiliate\"><a href=\"/buy\">Buy now with our \
             partner code</a></div></article>"
        ));
        assert!(!text.contains("partner code"), "{text}");
    }

    #[test]
    fn keeps_affiliate_disclosure_and_plain_purchase_links() {
        let text = run(&format!(
            "<article><p class=\"affiliate-disclosure\">This review contains affiliate \
             links.</p>{LEAD}<div><a href=\"/buy\">Buy now</a></div>\
             <div class=\"affiliate\"><a href=\"/notes\">Read our testing notes</a></div></article>"
        ));
        assert!(text.contains("contains affiliate links"), "{text}");
        assert!(text.contains("Buy now"), "{text}");
        assert!(text.contains("testing notes"), "{text}");
    }
}
