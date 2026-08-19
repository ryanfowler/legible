//! Metadata discovery and structured-data parsing.

use crate::budget::ParseBudget;
use crate::constants::{
    find_last_title_separator_start, has_hierarchical_title_separator, has_title_separator,
    is_json_ld_article_type, is_schema_org_url, normalize_whitespace, remove_title_first_part,
    remove_title_separators, split_word_tokens,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::{
    get_inner_text, get_inner_text_owned, get_normalized_inner_text, has_static_hidden_marker,
};
use serde_json::Value;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use url::Url;

/// Metadata discovered in the source page.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    /// Page title.
    pub title: Option<String>,
    /// Page description or summary.
    pub description: Option<String>,
    /// Author names in source order.
    pub authors: Vec<String>,
    /// Site or publication name.
    pub site_name: Option<String>,
    /// Canonical page URL.
    pub canonical_url: Option<String>,
    /// Representative image URL.
    pub image: Option<String>,
    /// Favicon URL.
    pub favicon: Option<String>,
    /// Publication time in its source format.
    pub published_time: Option<String>,
    /// Modification time in its source format.
    pub modified_time: Option<String>,
    /// Document language, such as `"en"` or `"fr"`.
    pub language: Option<String>,
    /// Text direction, such as `"ltr"` or `"rtl"`.
    pub direction: Option<String>,
    /// Page section or category.
    pub section: Option<String>,
    /// Page tags in source order.
    pub tags: Vec<String>,
    pub(crate) has_source_author: bool,
    pub(crate) title_from_content_heading: bool,
}

/// Parsed schema.org data. It remains available after metadata discovery so a
/// later extraction stage can use `articleBody` and `text` as location hints.
#[derive(Debug, Clone, Default)]
pub(crate) struct StructuredData {
    items: Vec<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructuredDataError {
    Bytes { limit: usize },
    Items { limit: usize },
    Depth { limit: usize },
}

const INTERNAL_MAX_JSON_LD_DEPTH: usize = 512;

impl StructuredData {
    pub(crate) fn parse(dom: &Dom, budget: &ParseBudget) -> Result<Self, StructuredDataError> {
        let scripts: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| {
                dom.tag(id) == Some(Tag::Script)
                    && dom.attr(id, AttrName::Type).is_some_and(|value| {
                        value.split(';').next().is_some_and(|mime| {
                            mime.trim().eq_ignore_ascii_case("application/ld+json")
                        })
                    })
            })
            .collect();
        let mut items = Vec::new();
        let mut buffer = String::new();
        let mut json_ld_bytes = 0usize;
        for id in scripts {
            let (raw_content, raw_bytes) = script_text_bounded(
                dom,
                id,
                &mut buffer,
                budget.max_json_ld_bytes.saturating_sub(json_ld_bytes),
            )
            .map_err(|_| StructuredDataError::Bytes {
                limit: budget.max_json_ld_bytes,
            })?;
            json_ld_bytes = json_ld_bytes.saturating_add(raw_bytes);
            if budget.max_json_ld_bytes > 0 && json_ld_bytes > budget.max_json_ld_bytes {
                return Err(StructuredDataError::Bytes {
                    limit: budget.max_json_ld_bytes,
                });
            }
            let content = raw_content
                .trim()
                .trim_start_matches("<![CDATA[")
                .trim_end_matches("]]>")
                .trim();
            let json_depth_limit = if budget.max_json_ld_depth == 0 {
                INTERNAL_MAX_JSON_LD_DEPTH
            } else {
                budget.max_json_ld_depth.min(INTERNAL_MAX_JSON_LD_DEPTH)
            };
            if exceeds_json_depth(content, json_depth_limit) {
                return Err(StructuredDataError::Depth {
                    limit: json_depth_limit,
                });
            }
            crate::instrumentation::record_json_ld_bytes(content.len());
            let Ok(value) = serde_json::from_str::<Value>(content) else {
                continue;
            };
            #[cfg(feature = "bench-instrumentation")]
            crate::instrumentation::record_json_ld_parsed_bytes(estimated_json_value_bytes(&value));
            collect_structured_items(&value, false, &mut items, budget.max_json_ld_items).map_err(
                |_| StructuredDataError::Items {
                    limit: budget.max_json_ld_items,
                },
            )?;
        }
        Ok(Self { items })
    }

    #[cfg(test)]
    pub(crate) fn article_texts(&self) -> impl Iterator<Item = &str> {
        self.items
            .iter()
            .filter(|item| {
                item.get("@type").is_some_and(|kind| {
                    json_types(kind)
                        .any(|kind| is_article_type(kind) || is_general_content_type(kind))
                })
            })
            .flat_map(article_text_values)
    }

    pub(crate) fn primary_texts<'a>(
        &'a self,
        document_title: &str,
        source_url: Option<&Url>,
    ) -> impl Iterator<Item = &'a str> {
        primary_hint_item(self, document_title, source_url)
            .into_iter()
            .flat_map(article_text_values)
    }

    fn has_article_item(&self) -> bool {
        self.items.iter().any(|item| {
            item.get("@type")
                .is_some_and(|kind| json_types(kind).any(is_article_type))
        })
    }

    fn primary_item_matches(&self, document_title: &str, source_url: Option<&Url>) -> bool {
        let Some(item) = primary_hint_item(self, document_title, source_url) else {
            return false;
        };
        let title_matches = item
            .get("headline")
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .is_some_and(|title| {
                !document_title.is_empty() && text_similarity(title, document_title) >= 0.6
            });
        let url_matches = json_url(item)
            .and_then(|value| Url::parse(value).ok())
            .zip(source_url)
            .is_some_and(|(candidate, source)| {
                candidate.host() == source.host()
                    && candidate.path().trim_end_matches('/') == source.path().trim_end_matches('/')
            });
        title_matches || url_matches
    }

    fn article_items(&self) -> impl Iterator<Item = &Value> {
        self.items.iter().filter(|item| {
            item.get("@type")
                .is_some_and(|kind| json_types(kind).any(is_article_type))
        })
    }

    fn typed_items<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
        self.items.iter().filter(move |item| {
            item.get("@type")
                .is_some_and(|value| json_types(value).any(|value| schema_type(value, kind)))
        })
    }

    pub(crate) fn retained_items(&self) -> Vec<Value> {
        let items = self.items.clone();
        #[cfg(feature = "bench-instrumentation")]
        crate::instrumentation::record_json_ld_retained_bytes(
            items.iter().map(estimated_json_value_bytes).sum(),
        );
        items
    }
}

#[cfg(feature = "bench-instrumentation")]
fn estimated_json_value_bytes(value: &Value) -> usize {
    let mut total = 0usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        total = total.saturating_add(std::mem::size_of::<Value>());
        match value {
            Value::Null | Value::Bool(_) => {}
            Value::Number(number) => {
                total = total.saturating_add(number.to_string().len());
            }
            Value::String(text) => {
                total = total.saturating_add(text.len());
            }
            Value::Array(values) => pending.extend(values.iter().rev()),
            Value::Object(values) => {
                for (key, value) in values.iter().rev() {
                    total = total.saturating_add(key.len());
                    pending.push(value);
                }
            }
        }
    }
    total
}

fn script_text_bounded<'a>(
    dom: &'a Dom,
    id: NodeId,
    buffer: &'a mut String,
    limit: usize,
) -> Result<(&'a str, usize), ()> {
    let mut children = dom.children(id);
    let first = children.next();
    if let Some(node) = first
        && children.next().is_none()
        && let Some(text) = dom.text_node(node)
    {
        if limit > 0 && text.len() > limit {
            return Err(());
        }
        return Ok((text, text.len()));
    }
    let bytes = std::iter::once(id)
        .chain(dom.descendants(id))
        .filter_map(|node| dom.text_node(node).map(str::len))
        .fold(0usize, usize::saturating_add);
    if limit > 0 && bytes > limit {
        return Err(());
    }
    buffer.clear();
    dom.append_text(id, buffer);
    Ok((buffer, bytes))
}

fn collect_structured_items(
    value: &Value,
    inherited_schema: bool,
    out: &mut Vec<Value>,
    max_items: usize,
) -> Result<(), ()> {
    let mut pending = vec![(value, inherited_schema)];
    while let Some((value, inherited_schema)) = pending.pop() {
        match value {
            Value::Array(values) => {
                pending.extend(values.iter().rev().map(|value| (value, inherited_schema)));
            }
            Value::Object(object) => {
                let schema = inherited_schema
                    || object.get("@context").is_some_and(is_schema_context)
                    || object
                        .get("@type")
                        .is_some_and(|kind| json_types(kind).any(is_absolute_schema_type));
                if !schema {
                    continue;
                }
                if object.get("@type").is_some() {
                    if max_items > 0 && out.len() >= max_items {
                        return Err(());
                    }
                    out.push(value.clone());
                }
                for (key, nested) in object.iter().rev() {
                    if !matches!(key.as_str(), "@context" | "@graph" | "@type") {
                        pending.push((nested, true));
                    }
                }
                if let Some(graph) = object.get("@graph") {
                    pending.push((graph, true));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_schema_context(value: &Value) -> bool {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(value) if is_schema_url(value) => return true,
            Value::Array(values) => pending.extend(values.iter()),
            Value::Object(values) => {
                if values
                    .get("@vocab")
                    .and_then(Value::as_str)
                    .is_some_and(is_schema_url)
                {
                    return true;
                }
                pending.extend(values.values());
            }
            _ => {}
        }
    }
    false
}

fn exceeds_json_depth(value: &str, limit: usize) -> bool {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in value.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                if depth > limit {
                    return true;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

fn is_schema_url(value: &str) -> bool {
    is_schema_org_url(value) || value.trim_end_matches('/') == "schema.org"
}

fn json_types(value: &Value) -> impl Iterator<Item = &str> {
    value.as_str().into_iter().chain(
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    )
}

fn schema_type(value: &str, expected: &str) -> bool {
    value
        .rsplit(['/', ':'])
        .next()
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn is_absolute_schema_type(value: &str) -> bool {
    value.starts_with("https://schema.org/") || value.starts_with("http://schema.org/")
}

fn is_article_type(value: &str) -> bool {
    is_json_ld_article_type(value)
}

fn is_general_content_type(value: &str) -> bool {
    [
        "WebPage",
        "AboutPage",
        "CollectionPage",
        "ContactPage",
        "FAQPage",
        "HowTo",
        "ProfilePage",
        "QAPage",
        "Recipe",
    ]
    .iter()
    .any(|kind| schema_type(value, kind))
}

/// The source of a discovered metadata value.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataSource {
    /// Schema.org JSON-LD.
    JsonLd,
    /// Open Graph metadata.
    OpenGraph,
    /// Twitter card metadata.
    Twitter,
    /// Dublin Core metadata.
    DublinCore,
    /// Citation or publishing-system metadata.
    Citation,
    /// A general HTML `meta` element.
    HtmlMeta,
    /// Visible or semantic HTML content.
    HtmlElement,
    /// An HTML `link` or `a` element.
    LinkElement,
    /// A value inferred from page context.
    Inferred,
}

/// One discovered metadata value and its source confidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataValue<T> {
    /// The normalized value.
    pub value: T,
    /// The source that supplied the value.
    pub source: MetadataSource,
    /// The source confidence from `0` through `100`.
    pub confidence: u8,
}

/// Selection details for a metadata field that has one value.
#[derive(Debug, Clone, Default)]
pub struct MetadataFieldDiagnostics<T> {
    /// The selected value.
    pub selected: Option<MetadataValue<T>>,
    /// The normalized values that were not selected.
    pub alternatives: Vec<MetadataValue<T>>,
}

/// Selection details for a metadata field that can have many values.
#[derive(Debug, Clone, Default)]
pub struct MetadataListFieldDiagnostics<T> {
    /// The selected values in source order.
    pub selected: Vec<MetadataValue<T>>,
    /// The normalized values that were not selected.
    pub alternatives: Vec<MetadataValue<T>>,
}

/// Optional provenance for all public metadata fields.
#[derive(Debug, Clone, Default)]
pub struct MetadataDiagnostics {
    /// Title selection details.
    pub title: MetadataFieldDiagnostics<String>,
    /// Description selection details.
    pub description: MetadataFieldDiagnostics<String>,
    /// Author selection details.
    pub authors: MetadataListFieldDiagnostics<String>,
    /// Site-name selection details.
    pub site_name: MetadataFieldDiagnostics<String>,
    /// Canonical-URL selection details.
    pub canonical_url: MetadataFieldDiagnostics<String>,
    /// Representative-image selection details.
    pub image: MetadataFieldDiagnostics<String>,
    /// Favicon selection details.
    pub favicon: MetadataFieldDiagnostics<String>,
    /// Publication-time selection details.
    pub published_time: MetadataFieldDiagnostics<String>,
    /// Modification-time selection details.
    pub modified_time: MetadataFieldDiagnostics<String>,
    /// Language selection details.
    pub language: MetadataFieldDiagnostics<String>,
    /// Text-direction selection details.
    pub direction: MetadataFieldDiagnostics<String>,
    /// Section selection details.
    pub section: MetadataFieldDiagnostics<String>,
    /// Tag selection details.
    pub tags: MetadataListFieldDiagnostics<String>,
}

impl MetadataDiagnostics {
    pub(crate) fn complete_with_fallbacks(&mut self, metadata: &Metadata) {
        complete_scalar(&mut self.title, metadata.title.as_deref());
        complete_scalar(&mut self.description, metadata.description.as_deref());
        complete_list(&mut self.authors, &metadata.authors);
        complete_scalar(&mut self.language, metadata.language.as_deref());
        complete_scalar(&mut self.direction, metadata.direction.as_deref());
    }
}

fn inferred_value(value: &str) -> MetadataValue<String> {
    MetadataValue {
        value: value.to_owned(),
        source: MetadataSource::Inferred,
        confidence: 40,
    }
}

fn complete_scalar(field: &mut MetadataFieldDiagnostics<String>, value: Option<&str>) {
    if field.selected.is_none()
        && let Some(value) = value
    {
        field.selected = Some(inferred_value(value));
    }
}

fn complete_list(field: &mut MetadataListFieldDiagnostics<String>, values: &[String]) {
    for value in values {
        if !field
            .selected
            .iter()
            .any(|selected| metadata_values_equal(&selected.value, value))
        {
            field.selected.push(inferred_value(value));
        }
    }
}

#[derive(Debug, Clone)]
struct MetadataCandidate {
    value: String,
    source: MetadataSource,
    confidence: u8,
    order: usize,
}

#[derive(Default)]
struct CandidateSet {
    title: Vec<MetadataCandidate>,
    description: Vec<MetadataCandidate>,
    authors: Vec<MetadataCandidate>,
    site_name: Vec<MetadataCandidate>,
    canonical_url: Vec<MetadataCandidate>,
    image: Vec<MetadataCandidate>,
    favicon: Vec<MetadataCandidate>,
    published_time: Vec<MetadataCandidate>,
    modified_time: Vec<MetadataCandidate>,
    language: Vec<MetadataCandidate>,
    direction: Vec<MetadataCandidate>,
    section: Vec<MetadataCandidate>,
    tags: Vec<MetadataCandidate>,
    next_order: usize,
}

impl CandidateSet {
    fn add(
        &mut self,
        field: fn(&mut Self) -> &mut Vec<MetadataCandidate>,
        value: impl Into<String>,
        source: MetadataSource,
        confidence: u8,
    ) {
        let order = self.next_order;
        self.next_order += 1;
        field(self).push(MetadataCandidate {
            value: value.into(),
            source,
            confidence,
            order,
        });
    }
}

/// Discovers metadata without changing the source DOM.
pub(crate) fn discover_with_diagnostics(
    dom: &Dom,
    structured: &StructuredData,
    document_title: &str,
    base_url: Option<&Url>,
    source_url: Option<&Url>,
    retain_diagnostics: bool,
) -> (Metadata, Option<MetadataDiagnostics>) {
    let mut candidates = CandidateSet::default();
    let identity_title = metadata_identity_title(dom, document_title);
    collect_structured_candidates(structured, &identity_title, source_url, &mut candidates);
    collect_meta_candidates(dom, &mut candidates);
    collect_link_candidates(dom, &mut candidates);
    collect_element_candidates(dom, document_title, &mut candidates);
    collect_visible_brand_candidate(dom, document_title, source_url, &mut candidates);

    if let Some(url) = source_url {
        candidates.add(
            |set| &mut set.canonical_url,
            url.as_str(),
            MetadataSource::Inferred,
            10,
        );
        if let Some(host) = url.host_str().filter(|host| host.contains('.')) {
            candidates.add(
                |set| &mut set.site_name,
                host.strip_prefix("www.").unwrap_or(host),
                MetadataSource::Inferred,
                10,
            );
        }
    }

    resolve_candidates(candidates, base_url, retain_diagnostics)
}

/// Finds content that is present in reliable page metadata but absent from the
/// visible source tree, as happens on static application shells.
#[cold]
#[inline(never)]
pub(crate) fn metadata_backed_content(
    dom: &Dom,
    structured: &StructuredData,
    metadata: &Metadata,
    document_title: &str,
    base_url: Option<&Url>,
    source_url: Option<&Url>,
) -> Option<String> {
    let has_article_signal =
        source_has_meta_value(dom, "og:type", "article") || structured.has_article_item();
    let has_canonical_signal = source_has_canonical_url(dom, base_url, source_url);
    let has_author_or_publication =
        !metadata.authors.is_empty() || metadata.published_time.is_some();
    if !has_article_signal || !has_canonical_signal || !has_author_or_publication {
        return None;
    }

    // Prefer an articleBody/text value from the selected JSON-LD item. A page
    // description is only a fallback after structured content is unavailable
    // or too generic to use.
    let structured_body = structured
        .primary_item_matches(document_title, source_url)
        .then(|| {
            structured
                .primary_texts(document_title, source_url)
                .filter_map(|value| normalize_metadata_body(value, 20, 3))
                .next()
        })
        .flatten();
    if let Some(body) = structured_body {
        return Some(body);
    }
    if !source_has_application_shell(dom) {
        return None;
    }
    metadata_description(dom, metadata)
}

fn source_has_meta_value(dom: &Dom, key: &str, expected: &str) -> bool {
    dom.descendants(dom.root()).any(|node| {
        dom.tag(node) == Some(Tag::Meta)
            && dom
                .attr(node, AttrName::Property)
                .or_else(|| dom.attr(node, AttrName::Name))
                .is_some_and(|value| value.eq_ignore_ascii_case(key))
            && dom
                .attr(node, AttrName::Content)
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
    })
}

fn source_has_canonical_url(dom: &Dom, base_url: Option<&Url>, source_url: Option<&Url>) -> bool {
    dom.descendants(dom.root()).any(|node| {
        if dom.tag(node) == Some(Tag::Link) {
            return dom.attr(node, AttrName::Rel).is_some_and(|rel| {
                rel.split_ascii_whitespace()
                    .any(|part| part.eq_ignore_ascii_case("canonical"))
            }) && dom
                .attr(node, AttrName::Href)
                .is_some_and(|href| canonical_matches_source(href, base_url, source_url));
        }
        dom.tag(node) == Some(Tag::Meta)
            && dom
                .attr(node, AttrName::Property)
                .or_else(|| dom.attr(node, AttrName::Name))
                .is_some_and(|key| key.eq_ignore_ascii_case("og:url"))
            && dom
                .attr(node, AttrName::Content)
                .is_some_and(|value| canonical_matches_source(value, base_url, source_url))
    })
}

fn canonical_matches_source(value: &str, base_url: Option<&Url>, source_url: Option<&Url>) -> bool {
    let resolved = Url::parse(value)
        .ok()
        .or_else(|| base_url.and_then(|base| base.join(value).ok()));
    let Some(resolved) = resolved else {
        return false;
    };
    if !matches!(resolved.scheme(), "http" | "https") {
        return false;
    }
    source_url.is_none_or(|source| {
        resolved.host() == source.host()
            && resolved.path().trim_end_matches('/') == source.path().trim_end_matches('/')
    })
}

fn source_has_application_shell(dom: &Dom) -> bool {
    let mut controls = 0;
    let mut has_data_structure = false;
    for node in dom.descendants(dom.root()) {
        if matches!(
            dom.tag(node),
            Some(Tag::Button | Tag::Input | Tag::Select | Tag::Textarea | Tag::Form)
        ) {
            controls += 1;
        }
        if matches!(dom.tag(node), Some(Tag::Table | Tag::Pre | Tag::Dl)) {
            has_data_structure = true;
        }
    }
    controls >= 2 && !has_data_structure
}

fn metadata_description(dom: &Dom, metadata: &Metadata) -> Option<String> {
    if let Some(description) = metadata
        .description
        .as_deref()
        .and_then(|value| normalize_metadata_body(value, 40, 5))
    {
        return Some(description);
    }
    [
        "og:description",
        "twitter:description",
        "description",
        "dc:description",
        "dcterm:description",
        "dcterms:description",
    ]
    .into_iter()
    .flat_map(|expected| {
        dom.descendants(dom.root()).filter(move |&node| {
            dom.tag(node) == Some(Tag::Meta)
                && dom
                    .attr(node, AttrName::Property)
                    .or_else(|| dom.attr(node, AttrName::Name))
                    .is_some_and(|key| key.eq_ignore_ascii_case(expected))
        })
    })
    .filter_map(|node| dom.attr(node, AttrName::Content))
    .find_map(|value| normalize_metadata_body(value, 40, 5))
}

fn normalize_metadata_body(value: &str, min_chars: usize, min_words: usize) -> Option<String> {
    let value = normalize_text(value)?;
    let lower = value.to_ascii_lowercase();
    let generic = [
        "welcome to our website",
        "welcome to our site",
        "enable javascript",
        "please enable javascript",
        "javascript is required",
        "you need to enable javascript",
        "please turn on javascript",
        "sign in to continue",
        "log in to continue",
    ];
    if value.chars().count() < min_chars
        || value.split_whitespace().count() < min_words
        || generic.iter().any(|prefix| {
            lower == *prefix
                || lower.strip_prefix(prefix).is_some_and(|rest| {
                    rest.chars().next().is_some_and(|character| {
                        character.is_whitespace() || ".!?".contains(character)
                    })
                })
        })
    {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
fn discover(
    dom: &Dom,
    structured: &StructuredData,
    document_title: &str,
    base_url: Option<&Url>,
    source_url: Option<&Url>,
) -> Metadata {
    discover_with_diagnostics(dom, structured, document_title, base_url, source_url, false).0
}

fn collect_visible_brand_candidate(
    dom: &Dom,
    document_title: &str,
    source_url: Option<&Url>,
    out: &mut CandidateSet,
) {
    let Some(body) = dom.body() else { return };
    let normalized_title = normalize_text(document_title);
    let mut buffer = String::new();
    for node in dom
        .descendants(body)
        .filter(|&node| dom.is_element(node))
        .take(40)
    {
        if dom.tag(node) != Some(Tag::A) {
            continue;
        }
        let href_is_home = dom
            .attr(node, AttrName::Href)
            .zip(source_url)
            .and_then(|(href, source)| source.join(href).ok().map(|target| (source, target)))
            .is_some_and(|(source, target)| {
                source.scheme() == target.scheme()
                    && source.host_str() == target.host_str()
                    && source.port_or_known_default() == target.port_or_known_default()
                    && target.path() == "/"
                    && target.query().is_none()
                    && target.fragment().is_none()
            });
        // Exact agreement with the document title prevents a generic header
        // link or personal greeting from becoming the publication name.
        let value = get_inner_text(dom, node, &mut buffer);
        if !href_is_home || normalized_title.as_deref() != normalize_text(value).as_deref() {
            continue;
        }
        if value.chars().count() <= 60 {
            out.add(
                |set| &mut set.site_name,
                value,
                MetadataSource::Inferred,
                20,
            );
            break;
        }
    }
}

fn collect_structured_candidates(
    data: &StructuredData,
    document_title: &str,
    source_url: Option<&Url>,
    out: &mut CandidateSet,
) {
    let (primary_article, primary_general, use_article, _) =
        primary_structured_items(data, document_title, source_url);
    if let Some(item) = primary_article.filter(|_| use_article) {
        let headline = item.get("headline").and_then(Value::as_str);
        let name = item.get("name").and_then(Value::as_str);
        match (name, headline) {
            (Some(name), Some(headline)) if name != headline => {
                let headline_matches = text_similarity(headline, document_title) > 0.75;
                let name_matches = text_similarity(name, document_title) > 0.75;
                if name_matches && !headline_matches {
                    out.add(|set| &mut set.title, name, MetadataSource::JsonLd, 98);
                    out.add(|set| &mut set.title, headline, MetadataSource::JsonLd, 91);
                } else {
                    out.add(|set| &mut set.title, headline, MetadataSource::JsonLd, 98);
                    out.add(|set| &mut set.title, name, MetadataSource::JsonLd, 91);
                }
            }
            (Some(name), _) => out.add(|set| &mut set.title, name, MetadataSource::JsonLd, 98),
            (_, Some(headline)) => {
                out.add(|set| &mut set.title, headline, MetadataSource::JsonLd, 98)
            }
            _ => {}
        }
        add_json_string(out, |set| &mut set.description, item.get("description"), 92);
        if let Some(authors) = item.get("author") {
            collect_json_names(authors, &mut |name| {
                out.add(|set| &mut set.authors, name, MetadataSource::JsonLd, 96)
            });
        }
        add_json_names(out, |set| &mut set.site_name, item.get("publisher"), 94);
        add_json_names(
            out,
            |set| &mut set.site_name,
            item.get("sourceOrganization"),
            90,
        );
        add_json_names(out, |set| &mut set.site_name, item.get("isPartOf"), 88);
        add_json_string(
            out,
            |set| &mut set.published_time,
            item.get("datePublished"),
            98,
        );
        add_json_string(
            out,
            |set| &mut set.modified_time,
            item.get("dateModified"),
            98,
        );
        add_json_string(out, |set| &mut set.language, item.get("inLanguage"), 90);
        add_json_string(out, |set| &mut set.section, item.get("articleSection"), 92);
        collect_json_keywords(item.get("keywords"), out);
        add_json_url(out, |set| &mut set.image, item.get("image"), 88);
        add_json_url(out, |set| &mut set.canonical_url, item.get("url"), 82);
        add_json_url(
            out,
            |set| &mut set.canonical_url,
            item.get("mainEntityOfPage"),
            90,
        );
    }
    if let Some(item) = primary_general.filter(|_| !use_article) {
        let headline = item.get("headline").and_then(Value::as_str);
        let name = item.get("name").and_then(Value::as_str);
        match (name, headline) {
            (Some(name), Some(headline)) if name != headline => {
                let headline_matches = text_similarity(headline, document_title) > 0.75;
                let name_matches = text_similarity(name, document_title) > 0.75;
                if headline_matches || !name_matches {
                    out.add(|set| &mut set.title, headline, MetadataSource::JsonLd, 94);
                    out.add(|set| &mut set.title, name, MetadataSource::JsonLd, 88);
                } else {
                    out.add(|set| &mut set.title, name, MetadataSource::JsonLd, 90);
                    out.add(|set| &mut set.title, headline, MetadataSource::JsonLd, 88);
                }
            }
            (Some(name), _) => out.add(|set| &mut set.title, name, MetadataSource::JsonLd, 90),
            (_, Some(headline)) => {
                out.add(|set| &mut set.title, headline, MetadataSource::JsonLd, 94)
            }
            _ => {}
        }
        add_json_string(out, |set| &mut set.description, item.get("description"), 88);
        if let Some(authors) = item.get("author") {
            collect_json_names(authors, &mut |name| {
                out.add(|set| &mut set.authors, name, MetadataSource::JsonLd, 90)
            });
        }
        add_json_names(out, |set| &mut set.site_name, item.get("publisher"), 88);
        add_json_names(
            out,
            |set| &mut set.site_name,
            item.get("sourceOrganization"),
            86,
        );
        add_json_names(out, |set| &mut set.site_name, item.get("isPartOf"), 84);
        add_json_url(out, |set| &mut set.image, item.get("image"), 84);
        add_json_url(out, |set| &mut set.canonical_url, item.get("url"), 80);
        add_json_string(
            out,
            |set| &mut set.published_time,
            item.get("datePublished"),
            92,
        );
        add_json_string(
            out,
            |set| &mut set.modified_time,
            item.get("dateModified"),
            92,
        );
        add_json_string(out, |set| &mut set.language, item.get("inLanguage"), 86);
        collect_json_keywords(item.get("keywords"), out);
    }
    for item in data.typed_items("WebSite") {
        add_json_string(out, |set| &mut set.site_name, item.get("name"), 86);
    }
}

pub(crate) fn content_identity_title(dom: &Dom, document_title: &str) -> String {
    if !document_title.is_empty() {
        return document_title.to_owned();
    }
    dom.first_descendant_by_tag(dom.root(), Tag::H1)
        .map(|heading| get_inner_text_owned(dom, heading))
        .unwrap_or_default()
}

fn metadata_identity_title(dom: &Dom, document_title: &str) -> String {
    best_visible_heading(
        dom,
        dom.descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::H1)),
    )
    .filter(|&heading| visible_heading_confidence(dom, heading) > 78)
    .map(|heading| get_inner_text_owned(dom, heading))
    .filter(|title| !title.trim().is_empty())
    .unwrap_or_else(|| document_title.to_owned())
}

fn primary_hint_item<'a>(
    data: &'a StructuredData,
    document_title: &str,
    source_url: Option<&Url>,
) -> Option<&'a Value> {
    primary_structured_items(data, document_title, source_url).3
}

fn primary_structured_items<'a>(
    data: &'a StructuredData,
    document_title: &str,
    source_url: Option<&Url>,
) -> (
    Option<&'a Value>,
    Option<&'a Value>,
    bool,
    Option<&'a Value>,
) {
    let mut primary_article =
        select_unique_structured_item(data.article_items(), document_title, source_url);
    let mut primary_general = select_unique_structured_item(
        data.items.iter().filter(|item| {
            item.get("@type")
                .is_some_and(|kind| json_types(kind).any(is_general_content_type))
        }),
        document_title,
        source_url,
    );
    let use_article = match (primary_article, primary_general) {
        (Some(article), Some(general)) => {
            let article_score = structured_item_score(article, document_title, source_url);
            let general_score = structured_item_score(general, document_title, source_url);
            if article_score == general_score {
                primary_article = None;
                primary_general = None;
            }
            article_score > general_score
        }
        (Some(_), None) => true,
        _ => false,
    };
    let primary = if use_article {
        primary_article
    } else {
        primary_general
    };
    (primary_article, primary_general, use_article, primary)
}

fn select_unique_structured_item<'a>(
    items: impl IntoIterator<Item = &'a Value>,
    document_title: &str,
    source_url: Option<&Url>,
) -> Option<&'a Value> {
    let mut fingerprint_buckets: HashMap<u64, Option<SmallVec<[&Value; 1]>>> = HashMap::new();
    for item in items {
        let bucket = fingerprint_buckets
            .entry(structured_item_fingerprint(item))
            .or_insert_with(|| Some(SmallVec::new()));
        let Some(values) = bucket else {
            continue;
        };
        if values.contains(&item) {
            continue;
        }
        if values.len() == FINGERPRINT_COLLISION_LIMIT {
            // The bounded fingerprint cannot distinguish this value from the
            // stored values. Exclude the whole bucket instead of counting an
            // unstored value again if an exact duplicate appears later.
            *bucket = None;
        } else {
            values.push(item);
        }
    }

    let mut selected = None;
    let mut best_score = 0;
    let mut best_count = 0;
    let mut item_count = 0;
    for item in fingerprint_buckets
        .values()
        .filter_map(Option::as_ref)
        .flatten()
    {
        item_count += 1;
        let score = structured_item_score(item, document_title, source_url);
        if selected.is_none() || score > best_score {
            selected = Some(*item);
            best_score = score;
            best_count = 1;
        } else if score == best_score {
            best_count += 1;
        }
    }
    (item_count == 1 || best_score > 0 && best_count == 1)
        .then_some(selected)
        .flatten()
}

// Bound deep equality checks when many distinct values share a bounded fingerprint.
const FINGERPRINT_COLLISION_LIMIT: usize = 8;
const FINGERPRINT_VALUE_LIMIT: usize = 256;
const FINGERPRINT_BYTE_LIMIT: usize = 16 * 1024;

struct FingerprintBudget {
    values: usize,
    bytes: usize,
}

fn structured_item_fingerprint(item: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    let mut budget = FingerprintBudget {
        values: FINGERPRINT_VALUE_LIMIT,
        bytes: FINGERPRINT_BYTE_LIMIT,
    };
    hash_json_value(item, &mut hasher, &mut budget);
    hasher.finish()
}

enum HashTask<'a> {
    Value(&'a Value),
    ObjectEntry(&'a str, &'a Value),
}

fn hash_json_value(value: &Value, hasher: &mut impl Hasher, budget: &mut FingerprintBudget) {
    if budget.values == 0 {
        0xff_u8.hash(hasher);
        return;
    }

    let mut pending = vec![HashTask::Value(value)];
    while let Some(task) = pending.pop() {
        if budget.values == 0 {
            break;
        }
        match task {
            HashTask::Value(value) => {
                budget.values -= 1;
                match value {
                    Value::Null => 0_u8.hash(hasher),
                    Value::Bool(value) => {
                        1_u8.hash(hasher);
                        value.hash(hasher);
                    }
                    Value::Number(value) => {
                        2_u8.hash(hasher);
                        hash_bounded_bytes(value.to_string().as_bytes(), hasher, budget);
                    }
                    Value::String(value) => {
                        3_u8.hash(hasher);
                        hash_bounded_bytes(value.as_bytes(), hasher, budget);
                    }
                    Value::Array(values) => {
                        4_u8.hash(hasher);
                        values.len().hash(hasher);
                        pending.extend(values.iter().rev().map(HashTask::Value));
                    }
                    Value::Object(values) => {
                        5_u8.hash(hasher);
                        values.len().hash(hasher);
                        pending.extend(
                            values
                                .iter()
                                .rev()
                                .map(|(key, value)| HashTask::ObjectEntry(key, value)),
                        );
                    }
                }
            }
            HashTask::ObjectEntry(key, value) => {
                hash_bounded_bytes(key.as_bytes(), hasher, budget);
                pending.push(HashTask::Value(value));
            }
        }
    }
}

fn hash_bounded_bytes(bytes: &[u8], hasher: &mut impl Hasher, budget: &mut FingerprintBudget) {
    bytes.len().hash(hasher);
    let count = bytes.len().min(budget.bytes);
    bytes[..count].hash(hasher);
    budget.bytes -= count;
}

fn article_text_values(item: &Value) -> impl Iterator<Item = &str> {
    ["articleBody", "text"]
        .into_iter()
        .filter_map(move |key| item.get(key).and_then(Value::as_str))
}

fn structured_item_score(item: &Value, document_title: &str, source_url: Option<&Url>) -> u16 {
    let title = item
        .get("headline")
        .or_else(|| item.get("name"))
        .and_then(Value::as_str);
    let title_score = title
        .map(|title| (text_similarity(title, document_title) * 100.0) as u16)
        .unwrap_or(0);
    let url_score = json_url(item)
        .and_then(|value| Url::parse(value).ok())
        .zip(source_url)
        .filter(|(candidate, source)| {
            candidate.as_str().trim_end_matches('/') == source.as_str().trim_end_matches('/')
        })
        .map_or(0, |_| 150);
    title_score + url_score
}

fn add_json_string(
    out: &mut CandidateSet,
    field: fn(&mut CandidateSet) -> &mut Vec<MetadataCandidate>,
    value: Option<&Value>,
    confidence: u8,
) {
    if let Some(value) = value.and_then(Value::as_str) {
        out.add(field, value, MetadataSource::JsonLd, confidence);
    }
}

fn add_json_names(
    out: &mut CandidateSet,
    field: fn(&mut CandidateSet) -> &mut Vec<MetadataCandidate>,
    value: Option<&Value>,
    confidence: u8,
) {
    if let Some(value) = value {
        collect_json_names(value, &mut |name| {
            out.add(field, name, MetadataSource::JsonLd, confidence)
        });
    }
}

fn add_json_url(
    out: &mut CandidateSet,
    field: fn(&mut CandidateSet) -> &mut Vec<MetadataCandidate>,
    value: Option<&Value>,
    confidence: u8,
) {
    let Some(value) = value else { return };
    if let Some(value) = value.as_str() {
        out.add(field, value, MetadataSource::JsonLd, confidence);
    } else if let Some(values) = value.as_array() {
        if let Some(value) = values.iter().find_map(json_url) {
            out.add(field, value, MetadataSource::JsonLd, confidence);
        }
    } else if let Some(value) = json_url(value) {
        out.add(field, value, MetadataSource::JsonLd, confidence);
    }
}

fn json_url(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("url").and_then(Value::as_str))
        .or_else(|| value.get("@id").and_then(Value::as_str))
        .or_else(|| value.get("contentUrl").and_then(Value::as_str))
}

fn json_name(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("name").and_then(Value::as_str))
}

fn collect_json_names(value: &Value, add: &mut impl FnMut(&str)) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        if let Some(values) = value.as_array() {
            pending.extend(values.iter().rev());
        } else if let Some(name) = json_name(value) {
            add(name);
        }
    }
}

fn collect_json_keywords(value: Option<&Value>, out: &mut CandidateSet) {
    let Some(value) = value else { return };
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        if let Some(values) = value.as_array() {
            pending.extend(values.iter().rev());
        } else if let Some(value) = value.as_str() {
            for tag in value.split(',') {
                out.add(|set| &mut set.tags, tag, MetadataSource::JsonLd, 88);
            }
        }
    }
}

fn collect_meta_candidates(dom: &Dom, out: &mut CandidateSet) {
    for id in dom
        .descendants(dom.root())
        .filter(|&id| dom.tag(id) == Some(Tag::Meta))
    {
        let Some(content) = dom
            .attr(id, AttrName::Content)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let property = dom.attr(id, AttrName::Property);
        let Some(raw_name) = property
            .or_else(|| dom.attr(id, AttrName::Name))
            .or_else(|| dom.attr_by_local_name(id, "http-equiv"))
        else {
            continue;
        };
        for raw_key in raw_name.split_ascii_whitespace() {
            collect_meta_value(out, &normalize_key(raw_key), content, property.is_some());
        }
    }
}

fn collect_meta_value(out: &mut CandidateSet, name: &str, content: &str, is_property: bool) {
    if is_property && !name.contains(':') {
        return;
    }
    let (source, confidence) = metadata_source(name);
    let add = |out: &mut CandidateSet,
               field: fn(&mut CandidateSet) -> &mut Vec<MetadataCandidate>,
               confidence: u8| {
        out.add(field, content, source, confidence);
    };
    match name {
        "og:title" => add(out, |set| &mut set.title, if is_property { 95 } else { 94 }),
        "twitter:title" => add(out, |set| &mut set.title, 86),
        "dc:title" | "dcterm:title" | "dcterms:title" => add(
            out,
            |set| &mut set.title,
            if is_property { 100 } else { 98 },
        ),
        "citation_title" => add(out, |set| &mut set.title, 90),
        "parsely-title" => add(out, |set| &mut set.title, 92),
        "title" => add(out, |set| &mut set.title, confidence),
        "og:description" => add(
            out,
            |set| &mut set.description,
            if is_property { 95 } else { 94 },
        ),
        "twitter:description" => add(out, |set| &mut set.description, 86),
        "dc:description" | "dcterm:description" | "dcterms:description" => add(
            out,
            |set| &mut set.description,
            if is_property { 100 } else { 98 },
        ),
        "description" => add(out, |set| &mut set.description, confidence),
        "author" | "dc:creator" | "dcterm:creator" | "dcterms:creator" | "sailthru:author"
        | "parsely-author" | "citation_author" => add(out, |set| &mut set.authors, confidence),
        "article:author" | "og:article:author" if Url::parse(content).is_err() => {
            add(out, |set| &mut set.authors, 82)
        }
        "og:site_name" | "application-name" => add(out, |set| &mut set.site_name, confidence),
        "og:url" => add(out, |set| &mut set.canonical_url, 88),
        "og:image" | "og:image:url" | "twitter:image" | "twitter:image:src" => {
            add(out, |set| &mut set.image, confidence)
        }
        "article:published_time"
        | "citation_publication_date"
        | "parsely-pub-date"
        | "publishdate"
        | "publish_date"
        | "datepublished" => add(out, |set| &mut set.published_time, confidence),
        "article:modified_time" | "datemodified" | "last-modified" => {
            add(out, |set| &mut set.modified_time, confidence)
        }
        "content-language" | "og:locale" => add(out, |set| &mut set.language, confidence),
        "article:section" => add(out, |set| &mut set.section, confidence),
        "article:tag" => add(out, |set| &mut set.tags, confidence),
        "keywords" | "news_keywords" => {
            for tag in content.split(',') {
                out.add(|set| &mut set.tags, tag, source, confidence);
            }
        }
        "citation_keywords" => {
            for tag in content.split([',', ';']) {
                out.add(|set| &mut set.tags, tag, source, confidence);
            }
        }
        _ => {}
    }
}

fn normalize_key(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|character| !character.is_whitespace())
        .map(|character| if character == '.' { ':' } else { character })
        .flat_map(char::to_lowercase)
        .collect()
}

fn metadata_source(name: &str) -> (MetadataSource, u8) {
    if name.starts_with("og:") || name.starts_with("article:") {
        (MetadataSource::OpenGraph, 90)
    } else if name.starts_with("twitter:") {
        (MetadataSource::Twitter, 82)
    } else if name.starts_with("dc:") || name.starts_with("dcterm:") || name.starts_with("dcterms:")
    {
        (MetadataSource::DublinCore, 96)
    } else if name.starts_with("citation_") || name.starts_with("parsely-") {
        (MetadataSource::Citation, 88)
    } else {
        (MetadataSource::HtmlMeta, 76)
    }
}

fn collect_link_candidates(dom: &Dom, out: &mut CandidateSet) {
    for id in dom
        .descendants(dom.root())
        .filter(|&id| dom.tag(id) == Some(Tag::Link))
    {
        let Some(rel) = dom.attr(id, AttrName::Rel) else {
            continue;
        };
        let Some(href) = dom.attr(id, AttrName::Href) else {
            continue;
        };
        if rel
            .split_ascii_whitespace()
            .any(|part| part.eq_ignore_ascii_case("canonical"))
        {
            out.add(
                |set| &mut set.canonical_url,
                href,
                MetadataSource::LinkElement,
                100,
            );
        }
        if rel.split_ascii_whitespace().any(|part| {
            matches!(
                part.to_ascii_lowercase().as_str(),
                "icon" | "shortcut" | "apple-touch-icon"
            )
        }) {
            out.add(
                |set| &mut set.favicon,
                href,
                MetadataSource::LinkElement,
                90,
            );
        }
    }
}

fn collect_element_candidates(dom: &Dom, document_title: &str, out: &mut CandidateSet) {
    if !document_title.is_empty() {
        out.add(
            |set| &mut set.title,
            document_title,
            MetadataSource::HtmlElement,
            84,
        );
    }
    let elements: Vec<_> = dom
        .descendants(dom.root())
        .filter(|&id| dom.is_element(id))
        .collect();
    let primary_heading = best_visible_heading(
        dom,
        elements
            .iter()
            .copied()
            .filter(|&id| dom.tag(id) == Some(Tag::H1)),
    );
    let mut text_buffer = String::new();
    for id in elements {
        if dom.tag(id) == Some(Tag::Html) {
            if let Some(language) = dom.attr(id, AttrName::Lang) {
                out.add(
                    |set| &mut set.language,
                    language,
                    MetadataSource::HtmlElement,
                    100,
                );
            }
            if let Some(direction) = dom.attr(id, AttrName::Dir) {
                out.add(
                    |set| &mut set.direction,
                    direction,
                    MetadataSource::HtmlElement,
                    100,
                );
            }
        }
        if dom.tag(id) == Some(Tag::H1) {
            let text = get_inner_text(dom, id, &mut text_buffer);
            let confidence = visible_heading_confidence(dom, id);
            if confidence > 0 {
                out.add(
                    |set| &mut set.title,
                    text,
                    MetadataSource::HtmlElement,
                    confidence,
                );
            }
        }
        let itemprop = dom.attr(id, AttrName::ItemProp).unwrap_or("");
        let mut has_author_itemprop = false;
        let mut has_published_itemprop = false;
        for part in itemprop.split_ascii_whitespace() {
            has_author_itemprop |= part.eq_ignore_ascii_case("author");
            has_published_itemprop |= part.eq_ignore_ascii_case("datePublished");
        }
        // Most elements have no relevant itemprop. Do not calculate their
        // distance from the heading because that scans ancestors and siblings.
        let itemprop_is_page_metadata = dom.tag(id) == Some(Tag::Meta)
            || (has_author_itemprop || has_published_itemprop)
                && primary_heading.is_some_and(|heading| {
                    is_near_heading(dom, id, heading)
                        || metadata_container_near_heading(dom, id, heading)
                });
        if itemprop_is_page_metadata && has_author_itemprop {
            let name_node = dom.descendants(id).find(|&child| {
                dom.attr(child, AttrName::ItemProp).is_some_and(|value| {
                    value
                        .split_ascii_whitespace()
                        .any(|part| part.eq_ignore_ascii_case("name"))
                })
            });
            let value = dom
                .attr(id, AttrName::Content)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    name_node.and_then(|node| {
                        dom.attr(node, AttrName::Content)
                            .filter(|value| !value.trim().is_empty())
                            .map(str::to_owned)
                    })
                })
                .or_else(|| byline_name(dom, name_node.unwrap_or(id)))
                .unwrap_or_else(|| {
                    get_inner_text(dom, name_node.unwrap_or(id), &mut text_buffer).to_owned()
                });
            out.add(
                |set| &mut set.authors,
                value,
                MetadataSource::HtmlElement,
                84,
            );
        }
        if itemprop_is_page_metadata && has_published_itemprop {
            let value = dom
                .attr_by_local_name(id, "datetime")
                .or_else(|| dom.attr(id, AttrName::Content))
                .unwrap_or_else(|| get_inner_text(dom, id, &mut text_buffer));
            out.add(
                |set| &mut set.published_time,
                value,
                MetadataSource::HtmlElement,
                82,
            );
        } else if dom.tag(id) == Some(Tag::Time)
            && primary_heading.is_some_and(|heading| is_near_heading(dom, id, heading))
        {
            let value = dom
                .attr_by_local_name(id, "datetime")
                .unwrap_or_else(|| get_inner_text(dom, id, &mut text_buffer));
            out.add(
                |set| &mut set.published_time,
                value,
                MetadataSource::Inferred,
                48,
            );
        }
        if dom.tag(id) == Some(Tag::A) {
            let rel_author = dom.attr(id, AttrName::Rel).is_some_and(|rel| {
                rel.split_ascii_whitespace()
                    .any(|part| part.eq_ignore_ascii_case("author"))
            });
            let profile_author = dom
                .attr(id, AttrName::Href)
                .is_some_and(|href| href.to_ascii_lowercase().contains("/author/"));
            if rel_author
                || (profile_author
                    && primary_heading
                        .is_some_and(|heading| is_profile_author_near_heading(dom, id, heading)))
            {
                let value = byline_name(dom, id)
                    .unwrap_or_else(|| get_inner_text(dom, id, &mut text_buffer).to_owned());
                out.add(
                    |set| &mut set.authors,
                    value,
                    MetadataSource::LinkElement,
                    if rel_author { 82 } else { 68 },
                );
            }
        }
        let author_element = is_author_container(dom, id);
        if author_element
            && primary_heading.is_some_and(|heading| is_byline_near_heading(dom, id, heading))
        {
            let author_node = preferred_author_node(dom, id);
            let value = byline_name(dom, id).unwrap_or_else(|| {
                get_inner_text(dom, author_node.unwrap_or(id), &mut text_buffer).to_owned()
            });
            if value.chars().count() <= 120 {
                out.add(|set| &mut set.authors, value, MetadataSource::Inferred, 62);
            }
            if is_byline_container(dom, id) {
                let visible_date = preferred_byline_date(dom, id);
                if let Some(date) = visible_date.as_deref() {
                    out.add(
                        |set| &mut set.published_time,
                        date,
                        MetadataSource::Inferred,
                        48,
                    );
                }
                if (visible_date.is_some()
                    || dom
                        .descendants(id)
                        .any(|node| dom.tag(node) == Some(Tag::Time)))
                    && let Some(section_node) = preferred_byline_section_node(dom, id, author_node)
                {
                    let section = get_inner_text(dom, section_node, &mut text_buffer);
                    out.add(
                        |set| &mut set.section,
                        section,
                        MetadataSource::Inferred,
                        46,
                    );
                }
            }
        }
    }
}

fn preferred_byline_date(dom: &Dom, container: NodeId) -> Option<String> {
    dom.descendants(container)
        .filter_map(|node| dom.text_node(node))
        .find_map(extract_written_date)
}

fn extract_written_date(text: &str) -> Option<String> {
    written_date(text).map(|(_, date)| date)
}

fn written_date(text: &str) -> Option<(usize, String)> {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let mut earliest = None;
    for month in MONTHS {
        for (start, _) in text.match_indices(month) {
            let value = normalize_whitespace(&text[start..]);
            let mut parts = value.split_ascii_whitespace();
            if parts.next() != Some(month) {
                continue;
            }
            let Some(day) = parts.next().map(|value| value.trim_end_matches(',')) else {
                continue;
            };
            let Some(year) = parts
                .next()
                .map(|value| value.trim_end_matches(|character: char| !character.is_ascii_digit()))
            else {
                continue;
            };
            if day.parse::<u8>().is_ok_and(|day| (1..=31).contains(&day))
                && year.len() == 4
                && year.bytes().all(|byte| byte.is_ascii_digit())
            {
                let date = format!("{month} {day}, {year}");
                if earliest
                    .as_ref()
                    .is_none_or(|(current, _): &(usize, String)| start < *current)
                {
                    earliest = Some((start, date));
                }
            }
        }
    }
    earliest
}

fn is_author_container_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    let parts: Vec<_> = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty()
        || parts.iter().any(|part| {
            matches!(
                *part,
                "avatar"
                    | "bio"
                    | "date"
                    | "dateline"
                    | "description"
                    | "image"
                    | "name"
                    | "photo"
                    | "picture"
                    | "role"
                    | "source"
                    | "time"
                    | "timestamp"
                    | "title"
            )
        })
    {
        return false;
    }
    if matches!(lower.as_str(), "author" | "byline" | "p-author") {
        return true;
    }
    if lower.ends_with("byline") {
        return true;
    }
    let Some(marker) = parts
        .iter()
        .position(|part| matches!(*part, "author" | "byline"))
    else {
        return false;
    };
    if marker == 0 {
        false
    } else if marker + 1 == parts.len() {
        parts[..marker].iter().any(|part| {
            matches!(
                *part,
                "article"
                    | "blog"
                    | "c"
                    | "content"
                    | "entry"
                    | "footer"
                    | "p"
                    | "post"
                    | "sidebar"
            ) || part.contains("meta")
                || part.eq_ignore_ascii_case("byline")
        })
    } else {
        marker + 2 == parts.len()
            && parts[marker + 1] == "item"
            && parts[..marker].iter().any(|part| part.contains("byline"))
    }
}

fn is_author_container(dom: &Dom, node: NodeId) -> bool {
    [
        dom.attr(node, AttrName::Class),
        dom.attr(node, AttrName::Id),
    ]
    .into_iter()
    .flatten()
    .flat_map(|value| value.split_ascii_whitespace())
    .any(is_author_container_token)
}

fn is_byline_container(dom: &Dom, node: NodeId) -> bool {
    [
        dom.attr(node, AttrName::Class),
        dom.attr(node, AttrName::Id),
    ]
    .into_iter()
    .flatten()
    .flat_map(|value| value.split_ascii_whitespace())
    .any(|token| {
        let lower = token.to_ascii_lowercase();
        lower == "author"
            || lower == "byline"
            || lower.ends_with("byline")
            || lower.contains("byline__author")
            || lower.contains("byline-author")
    })
}

fn metadata_container_near_heading(dom: &Dom, node: NodeId, heading: NodeId) -> bool {
    let mut current = dom.parent(node);
    while let Some(ancestor) = current {
        let is_metadata_container = [
            dom.attr(ancestor, AttrName::Class),
            dom.attr(ancestor, AttrName::Id),
        ]
        .into_iter()
        .flatten()
        .flat_map(|value| value.split_ascii_whitespace())
        .any(is_author_container_token);
        if is_metadata_container {
            return is_near_heading(dom, ancestor, heading);
        }
        current = dom.parent(ancestor);
    }
    false
}

fn visible_heading_confidence(dom: &Dom, heading: NodeId) -> u8 {
    if !is_visible_metadata_heading(dom, heading) {
        return 0;
    }
    let text = get_inner_text_owned(dom, heading);
    if normalize_text(&text).is_none() {
        return 0;
    }
    let title_bonus = u8::from(has_class_or_id_token(dom, heading, "p-name")) * 4
        + u8::from(split_word_tokens(&text).take(4).count() == 4) * 2;
    let mut current = Some(heading);
    while let Some(node) = current {
        if dom.tag(node) == Some(Tag::Article) {
            return 82 + title_bonus;
        }
        if dom.tag(node) == Some(Tag::Main) {
            return 80 + title_bonus;
        }
        if [
            dom.attr(node, AttrName::Class),
            dom.attr(node, AttrName::Id),
        ]
        .into_iter()
        .flatten()
        .flat_map(|value| value.split_ascii_whitespace())
        .any(is_primary_content_token)
        {
            return 88 + title_bonus;
        }
        current = dom.parent(node);
    }
    78 + title_bonus
}

fn is_visible_metadata_heading(dom: &Dom, heading: NodeId) -> bool {
    let mut current = Some(heading);
    while let Some(node) = current {
        if has_static_hidden_marker(dom, node)
            || dom
                .attr(node, AttrName::AriaHidden)
                .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        {
            return false;
        }
        current = dom.parent(node);
    }
    true
}

fn has_class_or_id_token(dom: &Dom, node: NodeId, expected: &str) -> bool {
    [
        dom.attr(node, AttrName::Class),
        dom.attr(node, AttrName::Id),
    ]
    .into_iter()
    .flatten()
    .flat_map(|value| value.split_ascii_whitespace())
    .any(|token| token.eq_ignore_ascii_case(expected))
}

fn best_visible_heading(dom: &Dom, headings: impl Iterator<Item = NodeId>) -> Option<NodeId> {
    headings
        .filter(|&heading| visible_heading_confidence(dom, heading) > 0)
        .fold(None, |best, heading| {
            best.filter(|&current| {
                visible_heading_confidence(dom, current) >= visible_heading_confidence(dom, heading)
            })
            .or(Some(heading))
        })
}

fn is_primary_content_token(token: &str) -> bool {
    [
        "article-content",
        "article-body",
        "entry-content",
        "e-content",
        "h-entry",
        "main-content",
        "post-content",
        "post-body",
    ]
    .iter()
    .any(|value| token.eq_ignore_ascii_case(value))
}

fn preferred_author_node(dom: &Dom, container: NodeId) -> Option<NodeId> {
    dom.descendants(container)
        .find(|&node| has_itemprop(dom, node, "name"))
        .or_else(|| {
            dom.descendants(container)
                .find(|&node| is_author_name_node(dom, node))
        })
        .or_else(|| {
            dom.descendants(container).find(|&node| {
                dom.tag(node) == Some(Tag::A)
                    && (dom.attr(node, AttrName::Rel).is_some_and(|rel| {
                        rel.split_ascii_whitespace()
                            .any(|part| part.eq_ignore_ascii_case("author"))
                    }) || dom
                        .attr(node, AttrName::Href)
                        .is_some_and(|href| href.to_ascii_lowercase().contains("/author/")))
            })
        })
        .or_else(|| {
            dom.element_children(container).find(|&node| {
                matches!(dom.tag(node), Some(Tag::B | Tag::Strong))
                    && get_inner_text_owned(dom, node).chars().count() <= 80
            })
        })
}

pub(crate) fn byline_name(dom: &Dom, container: NodeId) -> Option<String> {
    let value = preferred_author_node(dom, container)
        .or_else(|| timestamp_author_link(dom, container))
        .and_then(|node| {
            dom.attr(node, AttrName::Content)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| author_text_segment(dom, node))
        })
        .or_else(|| author_text_segment(dom, container))?;
    normalize_text(&value)
}

fn timestamp_author_link(dom: &Dom, container: NodeId) -> Option<NodeId> {
    let has_timestamp_separator = dom
        .descendants(container)
        .filter_map(|node| dom.text_node(node))
        .any(|text| text.split_whitespace().any(|token| token == "@"));
    if !has_timestamp_separator {
        return None;
    }
    let mut links = dom.descendants(container).filter(|&node| {
        dom.tag(node) == Some(Tag::A)
            && dom.has_non_whitespace_text(node)
            && dom.normalized_char_count(node) < 100
    });
    let link = links.next()?;
    links.next().is_none().then_some(link)
}

fn author_text_segment(dom: &Dom, root: NodeId) -> Option<String> {
    enum Visit {
        Node(NodeId),
        Stop,
    }

    let mut value = String::new();
    let mut stack: Vec<Visit> = dom.children_rev(root).map(Visit::Node).collect();
    while let Some(visit) = stack.pop() {
        let Visit::Node(node) = visit else {
            break;
        };
        if dom.tag(node) == Some(Tag::Br) {
            append_author_text_segment(&mut value, " ");
            continue;
        }
        if let Some(text) = dom.text_node(node) {
            if let Some(boundary) = author_timestamp_boundary(text) {
                append_author_text_segment(&mut value, &text[..boundary]);
                break;
            }
            append_author_text_segment(&mut value, text);
            continue;
        }
        if dom.tag(node) == Some(Tag::Img) {
            continue;
        }
        if is_author_role_node(dom, node) || is_author_timestamp_node(dom, node) {
            break;
        }
        if is_author_name_node(dom, node) {
            stack.push(Visit::Stop);
        }
        stack.extend(dom.children_rev(node).map(Visit::Node));
    }
    (!value.trim().is_empty()).then_some(value)
}

fn author_timestamp_boundary(text: &str) -> Option<usize> {
    written_date(text)
        .map(|(index, _)| index)
        .or_else(|| clock_time_boundary(text))
}

fn is_author_timestamp_node(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Time)
        || dom
            .text_node(node)
            .is_some_and(|text| author_timestamp_boundary(text).is_some())
}

fn append_author_text_segment(out: &mut String, text: &str) {
    let Some(first) = text.chars().find(|character| !character.is_whitespace()) else {
        if !out.is_empty() {
            out.push_str(text);
        }
        return;
    };
    if out.chars().next_back().is_some_and(char::is_alphanumeric) && first.is_alphanumeric() {
        out.push(' ');
    }
    out.push_str(text);
}

fn is_author_name_node(dom: &Dom, node: NodeId) -> bool {
    [
        dom.attr(node, AttrName::Class),
        dom.attr(node, AttrName::Id),
    ]
    .into_iter()
    .flatten()
    .flat_map(|value| value.split_ascii_whitespace())
    .any(|token| {
        let token = token.to_ascii_lowercase();
        token == "author-name"
            || token == "byline-name"
            || token == "byline__author"
            || token == "byline__author-name"
            || token == "p-author"
            || token == "p-name"
    })
}

fn is_author_role_node(dom: &Dom, node: NodeId) -> bool {
    [
        dom.attr(node, AttrName::Class),
        dom.attr(node, AttrName::Id),
    ]
    .into_iter()
    .flatten()
    .flat_map(|value| value.split_ascii_whitespace())
    .any(|token| {
        matches!(
            token.to_ascii_lowercase().as_str(),
            "author-bio"
                | "author-role"
                | "author-title"
                | "author__bio"
                | "author__role"
                | "author__title"
                | "byline-role"
                | "byline-title"
                | "byline__role"
                | "byline__title"
        )
    })
}

fn preferred_byline_section_node(
    dom: &Dom,
    container: NodeId,
    author_node: Option<NodeId>,
) -> Option<NodeId> {
    let mut links = dom.descendants(container).filter(|&node| {
        dom.tag(node) == Some(Tag::A)
            && author_node.is_none_or(|author| node != author)
            && is_section_link(dom, node)
            && !dom.attr(node, AttrName::Rel).is_some_and(|rel| {
                rel.split_ascii_whitespace()
                    .any(|part| part.eq_ignore_ascii_case("author"))
            })
            && !dom
                .attr(node, AttrName::Href)
                .is_some_and(|href| href.to_ascii_lowercase().contains("/author/"))
    });
    let first = links.next()?;
    links.next().is_none().then_some(first)
}

fn is_section_link(dom: &Dom, link: NodeId) -> bool {
    let section_path = dom.attr(link, AttrName::Href).is_some_and(|href| {
        let path = href.to_ascii_lowercase();
        [
            "/section/",
            "/sections/",
            "/category/",
            "/categories/",
            "/topic/",
            "/topics/",
        ]
        .iter()
        .any(|marker| path.contains(marker))
    });
    section_path || previous_element_sibling(dom, link) == Some(Tag::Hr)
}

fn previous_element_sibling(dom: &Dom, node: NodeId) -> Option<Tag> {
    let mut sibling = dom.prev_sibling(node);
    while let Some(current) = sibling {
        if let Some(tag) = dom.tag(current) {
            return Some(tag);
        }
        sibling = dom.prev_sibling(current);
    }
    None
}

fn has_itemprop(dom: &Dom, node: NodeId, expected: &str) -> bool {
    dom.attr(node, AttrName::ItemProp).is_some_and(|value| {
        value
            .split_ascii_whitespace()
            .any(|part| part.eq_ignore_ascii_case(expected))
    })
}

fn is_near_heading(dom: &Dom, node: NodeId, heading: NodeId) -> bool {
    if dom.parent(node) == dom.parent(heading) && sibling_element_distance(dom, node, heading) <= 6
    {
        return true;
    }
    nearest_ancestor_with_tag(dom, node, Tag::Header)
        .zip(nearest_ancestor_with_tag(dom, heading, Tag::Header))
        .is_some_and(|(node_header, heading_header)| node_header == heading_header)
}

fn is_byline_near_heading(dom: &Dom, node: NodeId, heading: NodeId) -> bool {
    is_near_heading(dom, node, heading)
        || (nearest_ancestor_with_tag(dom, node, Tag::Aside).is_none()
            && document_element_distance(dom, node, heading) <= 12)
}

fn is_profile_author_near_heading(dom: &Dom, node: NodeId, heading: NodeId) -> bool {
    if is_near_heading(dom, node, heading) {
        return true;
    }
    let mut current = dom.parent(node);
    while let Some(ancestor) = current {
        if (matches!(dom.tag(ancestor), Some(Tag::Header)) || is_author_container(dom, ancestor))
            && is_near_heading(dom, ancestor, heading)
        {
            return true;
        }
        current = dom.parent(ancestor);
    }
    false
}

fn document_element_distance(dom: &Dom, first: NodeId, second: NodeId) -> usize {
    let mut first_position = None;
    let mut second_position = None;
    for (position, node) in dom
        .descendants(dom.root())
        .filter(|&node| dom.is_element(node))
        .enumerate()
    {
        if node == first {
            first_position = Some(position);
        }
        if node == second {
            second_position = Some(position);
        }
        if first_position.is_some() && second_position.is_some() {
            break;
        }
    }
    first_position
        .zip(second_position)
        .map_or(usize::MAX, |(first, second)| first.abs_diff(second))
}

fn sibling_element_distance(dom: &Dom, first: NodeId, second: NodeId) -> usize {
    let Some(parent) = dom.parent(first) else {
        return usize::MAX;
    };
    let mut first_position = None;
    let mut second_position = None;
    for (position, child) in dom.element_children(parent).enumerate() {
        if child == first {
            first_position = Some(position);
        }
        if child == second {
            second_position = Some(position);
        }
    }
    first_position
        .zip(second_position)
        .map_or(usize::MAX, |(first, second)| first.abs_diff(second))
}

fn nearest_ancestor_with_tag(dom: &Dom, node: NodeId, tag: Tag) -> Option<NodeId> {
    let mut current = dom.parent(node);
    while let Some(id) = current {
        if dom.tag(id) == Some(tag) {
            return Some(id);
        }
        current = dom.parent(id);
    }
    None
}

fn resolve_candidates(
    mut candidates: CandidateSet,
    base_url: Option<&Url>,
    retain_diagnostics: bool,
) -> (Metadata, Option<MetadataDiagnostics>) {
    normalize_all(&mut candidates.title, normalize_title);
    normalize_all(&mut candidates.description, normalize_description);
    normalize_all(&mut candidates.authors, normalize_person);
    if candidates
        .authors
        .iter()
        .any(|candidate| !is_ambiguous_person_placeholder(&candidate.value))
    {
        candidates
            .authors
            .retain(|candidate| !is_ambiguous_person_placeholder(&candidate.value));
    }
    normalize_all(&mut candidates.site_name, normalize_site_name);
    normalize_all(&mut candidates.published_time, normalize_scalar_text);
    normalize_all(&mut candidates.modified_time, normalize_scalar_text);
    normalize_all(&mut candidates.language, normalize_language);
    normalize_all(&mut candidates.direction, normalize_direction);
    normalize_all(&mut candidates.section, normalize_scalar_text);
    normalize_all(&mut candidates.tags, normalize_list_text);
    normalize_urls(&mut candidates.canonical_url, base_url);
    normalize_urls(&mut candidates.image, base_url);
    normalize_urls(&mut candidates.favicon, base_url);

    let has_source_author = candidates.authors.iter().any(|candidate| {
        !matches!(
            candidate.source,
            MetadataSource::HtmlElement | MetadataSource::LinkElement | MetadataSource::Inferred
        )
    });
    let site_name = resolve_one(&candidates.site_name);
    let selected_title = resolve_best(&candidates.title);
    let mut title = selected_title.map(|candidate| candidate.value.clone());
    if let (Some(title_value), Some(site)) = (&title, &site_name) {
        if title_value.eq_ignore_ascii_case(site) {
            if let Some(alternative) = resolve_one_excluding(&candidates.title, title_value) {
                title = Some(alternative);
            }
        } else if let Some(stripped) = strip_site_affix(title_value, site) {
            title = Some(stripped);
        }
    }
    let title_from_content_heading = title.as_deref().is_some_and(|title| {
        candidates.title.iter().any(|candidate| {
            candidate.source == MetadataSource::HtmlElement
                && candidate.confidence >= 90
                && metadata_values_equal(&candidate.value, title)
        })
    });

    let metadata = Metadata {
        title,
        description: resolve_one(&candidates.description),
        authors: resolve_authors(&candidates.authors),
        site_name,
        canonical_url: resolve_one(&candidates.canonical_url),
        image: resolve_one(&candidates.image),
        favicon: resolve_one(&candidates.favicon),
        published_time: resolve_one(&candidates.published_time),
        modified_time: resolve_one(&candidates.modified_time),
        language: resolve_one(&candidates.language),
        direction: resolve_one(&candidates.direction),
        section: resolve_one(&candidates.section),
        tags: resolve_many(&candidates.tags),
        has_source_author,
        title_from_content_heading,
    };
    let diagnostics = retain_diagnostics.then(|| MetadataDiagnostics {
        title: scalar_diagnostics(&candidates.title, metadata.title.as_deref()),
        description: scalar_diagnostics(&candidates.description, metadata.description.as_deref()),
        authors: author_list_diagnostics(&candidates.authors, &metadata.authors),
        site_name: scalar_diagnostics(&candidates.site_name, metadata.site_name.as_deref()),
        canonical_url: scalar_diagnostics(
            &candidates.canonical_url,
            metadata.canonical_url.as_deref(),
        ),
        image: scalar_diagnostics(&candidates.image, metadata.image.as_deref()),
        favicon: scalar_diagnostics(&candidates.favicon, metadata.favicon.as_deref()),
        published_time: scalar_diagnostics(
            &candidates.published_time,
            metadata.published_time.as_deref(),
        ),
        modified_time: scalar_diagnostics(
            &candidates.modified_time,
            metadata.modified_time.as_deref(),
        ),
        language: scalar_diagnostics(&candidates.language, metadata.language.as_deref()),
        direction: scalar_diagnostics(&candidates.direction, metadata.direction.as_deref()),
        section: scalar_diagnostics(&candidates.section, metadata.section.as_deref()),
        tags: list_diagnostics(&candidates.tags, &metadata.tags),
    });
    (metadata, diagnostics)
}

fn public_value(candidate: &MetadataCandidate, value: String) -> MetadataValue<String> {
    MetadataValue {
        value,
        source: candidate.source,
        confidence: candidate.confidence,
    }
}

fn scalar_diagnostics(
    candidates: &[MetadataCandidate],
    selected: Option<&str>,
) -> MetadataFieldDiagnostics<String> {
    let selected_candidate = selected.and_then(|value| {
        resolve_best_matching(candidates, value, &[])
            .or_else(|| resolve_best(candidates))
            .map(|candidate| (candidate, value))
    });
    MetadataFieldDiagnostics {
        selected: selected_candidate
            .map(|(candidate, value)| public_value(candidate, value.to_owned())),
        alternatives: candidates
            .iter()
            .filter(|candidate| {
                selected_candidate.is_none_or(|(selected, _)| candidate.order != selected.order)
            })
            .map(|candidate| public_value(candidate, candidate.value.clone()))
            .collect(),
    }
}

fn author_list_diagnostics(
    candidates: &[MetadataCandidate],
    selected: &[String],
) -> MetadataListFieldDiagnostics<String> {
    let mut selected_orders = Vec::new();
    let selected = selected
        .iter()
        .filter_map(|value| {
            let candidate = resolve_best_exact_matching(candidates, value, &selected_orders)
                .or_else(|| resolve_best_matching(candidates, value, &selected_orders))?;
            selected_orders.push(candidate.order);
            Some(public_value(candidate, value.clone()))
        })
        .collect();
    let alternatives = candidates
        .iter()
        .filter(|candidate| !selected_orders.contains(&candidate.order))
        .map(|candidate| public_value(candidate, candidate.value.clone()))
        .collect();
    MetadataListFieldDiagnostics {
        selected,
        alternatives,
    }
}

fn list_diagnostics(
    candidates: &[MetadataCandidate],
    selected: &[String],
) -> MetadataListFieldDiagnostics<String> {
    let mut selected_orders = Vec::new();
    let selected = selected
        .iter()
        .filter_map(|value| {
            let candidate = resolve_best_matching(candidates, value, &selected_orders)?;
            selected_orders.push(candidate.order);
            Some(public_value(candidate, value.clone()))
        })
        .collect();
    let alternatives = candidates
        .iter()
        .filter(|candidate| !selected_orders.contains(&candidate.order))
        .map(|candidate| public_value(candidate, candidate.value.clone()))
        .collect();
    MetadataListFieldDiagnostics {
        selected,
        alternatives,
    }
}

fn normalize_all(candidates: &mut Vec<MetadataCandidate>, normalize: fn(&str) -> Option<String>) {
    candidates.retain_mut(|candidate| {
        if let Some(value) = normalize(&candidate.value) {
            candidate.value = value;
            true
        } else {
            false
        }
    });
}

fn normalize_urls(candidates: &mut Vec<MetadataCandidate>, base_url: Option<&Url>) {
    candidates.retain_mut(|candidate| {
        let Some(value) = normalize_text(&candidate.value) else {
            return false;
        };
        let Some(resolved) = Url::parse(&value)
            .ok()
            .or_else(|| base_url.and_then(|base| base.join(&value).ok()))
        else {
            return false;
        };
        if !matches!(resolved.scheme(), "http" | "https") {
            return false;
        }
        candidate.value = resolved.into();
        true
    });
}

fn normalize_text(value: &str) -> Option<String> {
    let unescaped = unescape_html_entities(value);
    let value = normalize_whitespace(unescaped.trim());
    if value.is_empty() || is_placeholder(&value) || !value.chars().any(char::is_alphanumeric) {
        None
    } else {
        Some(value)
    }
}

pub(crate) fn normalize_title(value: &str) -> Option<String> {
    normalize_field_text(
        value,
        &[
            "title",
            "page title",
            "default title",
            "your title",
            "n/a",
            "not available",
            "null",
            "undefined",
            "unset",
        ],
    )
}

fn normalize_description(value: &str) -> Option<String> {
    normalize_field_text(
        value,
        &[
            "description",
            "page description",
            "default description",
            "your description",
            "n/a",
            "not available",
            "null",
            "undefined",
            "unset",
        ],
    )
}

fn normalize_site_name(value: &str) -> Option<String> {
    normalize_field_text(
        value,
        &[
            "site name",
            "website name",
            "default site name",
            "your site name",
            "n/a",
            "not available",
            "null",
            "undefined",
            "unset",
        ],
    )
}

fn normalize_field_text(value: &str, placeholders: &[&str]) -> Option<String> {
    let value = normalize_text(value)?;
    (!placeholders
        .iter()
        .any(|placeholder| value.eq_ignore_ascii_case(placeholder)))
    .then_some(value)
}

fn normalize_scalar_text(value: &str) -> Option<String> {
    normalize_field_text(
        value,
        &["n/a", "not available", "none", "null", "undefined", "unset"],
    )
}

fn normalize_list_text(value: &str) -> Option<String> {
    normalize_scalar_text(value)
}

pub(crate) fn normalize_person(value: &str) -> Option<String> {
    let value = normalize_text(value)?;
    if value.eq_ignore_ascii_case("by")
        || ["last updated", "last modified"]
            .iter()
            .any(|prefix| value.to_ascii_lowercase().starts_with(prefix))
    {
        return None;
    }
    let without_label = strip_metadata_label_prefix(&value);
    let without_prefix = strip_by_prefix(without_label);
    let value = normalize_text(person_name_segment(without_prefix))?;
    if [
        "author",
        "authors",
        "author name",
        "default author",
        "your name",
        "n/a",
        "not applicable",
        "not available",
        "none",
        "null",
        "undefined",
        "unset",
    ]
    .iter()
    .any(|placeholder| value.eq_ignore_ascii_case(placeholder))
        || Url::parse(&value).is_ok()
        || value.chars().count() > 120
    {
        None
    } else {
        Some(value)
    }
}

fn strip_metadata_label_prefix(value: &str) -> &str {
    for label in ["posted", "published", "updated"] {
        let Some(prefix) = value.get(..label.len()) else {
            continue;
        };
        let remainder = &value[label.len()..];
        if prefix.eq_ignore_ascii_case(label)
            && remainder
                .chars()
                .next()
                .is_some_and(|character| character.is_whitespace() || character == ':')
        {
            return remainder.trim_start_matches(|character: char| {
                character.is_whitespace() || character == ':'
            });
        }
    }
    value
}

fn person_name_segment(value: &str) -> &str {
    let value = value.trim().trim_start_matches(['|', '·', '—', '–']).trim();
    let separator_boundary = value
        .char_indices()
        .filter_map(|(index, character)| {
            matches!(character, '|' | '·' | '—' | '–').then_some(index)
        })
        .chain(metadata_label_boundary(value))
        .min()
        .unwrap_or(value.len());
    let date_boundary = written_date(value)
        .map(|(index, _)| index)
        .filter(|&index| index > 0)
        .unwrap_or(value.len());
    let boundary = separator_boundary
        .min(date_boundary)
        .min(clock_time_boundary(value).unwrap_or(value.len()));
    let value = value[..boundary].trim();
    if let Some((name, role)) = value.split_once(',')
        && [
            "engineer",
            "editor",
            "writer",
            "reporter",
            "correspondent",
            "director",
            "manager",
            "founder",
            "president",
            "officer",
            "developer",
            "designer",
        ]
        .iter()
        .any(|word| role.to_ascii_lowercase().contains(word))
    {
        name.trim()
    } else {
        value
    }
}

fn clock_time_boundary(value: &str) -> Option<usize> {
    for (colon, _) in value.match_indices(':') {
        let start = value[..colon]
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map_or(0, |(index, character)| index + character.len_utf8());
        let end = value[colon + 1..]
            .find(|character: char| !character.is_ascii_digit())
            .map_or(value.len(), |index| colon + 1 + index);
        let hour = &value[start..colon];
        let minute = &value[colon + 1..end];
        if hour.parse::<u8>().is_ok_and(|hour| hour <= 23)
            && minute.parse::<u8>().is_ok_and(|minute| minute <= 59)
        {
            return Some(start);
        }
    }
    None
}

fn metadata_label_boundary(value: &str) -> Option<usize> {
    let lowercase = value.to_ascii_lowercase();
    ["posted", "published", "updated"]
        .into_iter()
        .flat_map(|label| {
            lowercase
                .match_indices(label)
                .map(move |(index, _)| (index, label))
        })
        .filter_map(|(index, label)| {
            let before = value[..index].chars().next_back();
            let after = value[index + label.len()..].chars().next();
            (index > 0
                && before.is_some_and(|character| {
                    character.is_whitespace() || matches!(character, '|' | '·' | '—' | '–')
                })
                && after.is_none_or(|character| character.is_whitespace() || character == ':'))
            .then_some(index)
        })
        .min()
}

fn strip_by_prefix(value: &str) -> &str {
    let Some(prefix) = value.get(..2) else {
        return value;
    };
    let remainder = &value[2..];
    if prefix.eq_ignore_ascii_case("by")
        && remainder.chars().next().is_some_and(char::is_whitespace)
    {
        remainder.trim_start()
    } else {
        value
    }
}

pub(crate) fn normalize_language(value: &str) -> Option<String> {
    let value = normalize_scalar_text(value)?;
    Some(value.replace('_', "-"))
}

pub(crate) fn normalize_direction(value: &str) -> Option<String> {
    let value = normalize_text(value)?.to_ascii_lowercase();
    matches!(value.as_str(), "ltr" | "rtl" | "auto").then_some(value)
}

fn is_placeholder(value: &str) -> bool {
    (value.contains("{{") && value.contains("}}"))
        || (value.contains("${") && value.contains('}'))
        || (value.contains("<%") && value.contains("%>"))
}

fn resolve_one(candidates: &[MetadataCandidate]) -> Option<String> {
    resolve_best(candidates).map(|candidate| candidate.value.clone())
}

fn resolve_best(candidates: &[MetadataCandidate]) -> Option<&MetadataCandidate> {
    let mut best: Option<(&MetadataCandidate, u16)> = None;
    for candidate in candidates {
        let score = candidate_resolution_score(candidate, candidates);
        if best.is_none_or(|(current, current_score)| {
            score > current_score || (score == current_score && candidate.order < current.order)
        }) {
            best = Some((candidate, score));
        }
    }
    best.map(|(candidate, _)| candidate)
}

fn resolve_best_exact_matching<'a>(
    candidates: &'a [MetadataCandidate],
    value: &str,
    excluded_orders: &[usize],
) -> Option<&'a MetadataCandidate> {
    candidates
        .iter()
        .filter(|candidate| candidate.value == value && !excluded_orders.contains(&candidate.order))
        .max_by(|first, second| {
            candidate_resolution_score(first, candidates)
                .cmp(&candidate_resolution_score(second, candidates))
                .then_with(|| second.order.cmp(&first.order))
        })
}

fn resolve_best_matching<'a>(
    candidates: &'a [MetadataCandidate],
    value: &str,
    excluded_orders: &[usize],
) -> Option<&'a MetadataCandidate> {
    let mut best: Option<(&MetadataCandidate, u16)> = None;
    for candidate in candidates.iter().filter(|candidate| {
        metadata_values_equal(&candidate.value, value)
            && !excluded_orders.contains(&candidate.order)
    }) {
        let score = candidate_resolution_score(candidate, candidates);
        if best.is_none_or(|(current, current_score)| {
            score > current_score || (score == current_score && candidate.order < current.order)
        }) {
            best = Some((candidate, score));
        }
    }
    best.map(|(candidate, _)| candidate)
}

fn candidate_resolution_score(
    candidate: &MetadataCandidate,
    candidates: &[MetadataCandidate],
) -> u16 {
    let agreements = candidates
        .iter()
        .filter(|other| {
            other.source != candidate.source
                && metadata_values_equal(&other.value, &candidate.value)
        })
        .count() as u16;
    u16::from(candidate.confidence) + agreements.min(3) * 5
}

fn resolve_one_excluding(candidates: &[MetadataCandidate], excluded: &str) -> Option<String> {
    let filtered: Vec<_> = candidates
        .iter()
        .filter(|candidate| !metadata_values_equal(&candidate.value, excluded))
        .cloned()
        .collect();
    resolve_one(&filtered)
}

fn resolve_authors(candidates: &[MetadataCandidate]) -> Vec<String> {
    let Some(maximum) = candidates
        .iter()
        .map(|candidate| candidate.confidence)
        .max()
    else {
        return Vec::new();
    };
    let selected: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.confidence.saturating_add(8) >= maximum)
        .collect();
    let mut authors = Vec::new();
    for candidate in &selected {
        if authors
            .iter()
            .any(|value: &String| metadata_values_equal(value, &candidate.value))
        {
            continue;
        }
        let representative = selected
            .iter()
            .copied()
            .filter(|other| metadata_values_equal(&other.value, &candidate.value))
            .max_by(|first, second| {
                author_spelling_score(&first.value)
                    .cmp(&author_spelling_score(&second.value))
                    .then_with(|| {
                        candidate_resolution_score(first, candidates)
                            .cmp(&candidate_resolution_score(second, candidates))
                    })
                    .then_with(|| second.order.cmp(&first.order))
            })
            .unwrap_or(candidate);
        authors.push(representative.value.clone());
        if authors.len() == 10 {
            break;
        }
    }
    authors
}

fn resolve_many(candidates: &[MetadataCandidate]) -> Vec<String> {
    let mut candidates = candidates.to_vec();
    candidates.sort_by_key(|candidate| candidate.order);
    let mut values = Vec::new();
    for candidate in candidates {
        if !values
            .iter()
            .any(|value: &String| metadata_values_equal(value, &candidate.value))
        {
            values.push(candidate.value);
        }
    }
    values
}

fn is_ambiguous_person_placeholder(value: &str) -> bool {
    value.eq_ignore_ascii_case("unknown")
}

fn author_spelling_score(value: &str) -> u8 {
    u8::from(value.chars().any(char::is_lowercase))
}

fn metadata_values_equal(first: &str, second: &str) -> bool {
    first.eq_ignore_ascii_case(second)
        || (!first.is_ascii() || !second.is_ascii())
            && caseless::canonical_caseless_match_str(first, second)
}

fn strip_site_affix(title: &str, site: &str) -> Option<String> {
    for separator in [" | ", " - ", " — ", " – ", " :: ", " · "] {
        if let Some(value) = title.strip_suffix(&format!("{separator}{site}")) {
            return normalize_text(value);
        }
        if let Some(value) = title.strip_prefix(&format!("{site}{separator}")) {
            return normalize_text(value);
        }
    }
    None
}

pub fn unescape_html_entities<'a>(value: &'a str) -> Cow<'a, str> {
    if value.is_empty() || !value.contains('&') {
        return Cow::Borrowed(value);
    }
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        let rest = &value[index..];
        let Some(ampersand) = rest.find('&') else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..ampersand]);
        index += ampersand;
        if let Some(semicolon) = value[index..].find(';') {
            let entity = &value[index + 1..index + semicolon];
            if let Some(character) = parse_numeric(entity) {
                output.push(character);
                index += semicolon + 1;
                continue;
            }
            let name = &value[index + 1..index + semicolon + 1];
            if let Some(&(first, second)) = html5ever::data::NAMED_ENTITIES.get(name) {
                if let Some(character) = char::from_u32(first) {
                    output.push(character);
                }
                if second != 0
                    && let Some(character) = char::from_u32(second)
                {
                    output.push(character);
                }
                index += semicolon + 1;
                continue;
            }
        }
        output.push('&');
        index += 1;
    }
    Cow::Owned(output)
}

fn parse_numeric(value: &str) -> Option<char> {
    if !value.starts_with('#') {
        return None;
    }
    let number = if value[1..].starts_with(['x', 'X']) {
        u32::from_str_radix(&value[2..], 16).ok()?
    } else {
        value[1..].parse().ok()?
    };
    if number == 0 || number > 0x10ffff || (0xd800..=0xdfff).contains(&number) {
        Some('\u{fffd}')
    } else {
        char::from_u32(number).or(Some('\u{fffd}'))
    }
}

pub(crate) fn get_page_title(dom: &Dom) -> String {
    let Some(id) = dom.first_descendant_by_tag(dom.root(), Tag::Title) else {
        return String::new();
    };
    let original = get_inner_text_owned(dom, id);
    if original.is_empty() {
        return original;
    }
    let mut current = Cow::Borrowed(original.as_str());
    let mut hierarchical = false;
    fn word_count(value: &str) -> usize {
        value.split_whitespace().count()
    }
    if has_title_separator(&original) {
        hierarchical = has_hierarchical_title_separator(&original);
        if let Some(start) = find_last_title_separator_start(&original) {
            current = Cow::Borrowed(&original[..start]);
        }
        if word_count(&current) < 3 {
            current = Cow::Owned(remove_title_first_part(&original));
        }
    } else if original.contains(": ") {
        let mut text_buffer = String::new();
        let has_matching_heading = dom
            .descendants(dom.root())
            .filter(|&id| matches!(dom.tag(id), Some(Tag::H1 | Tag::H2)))
            .any(|id| get_inner_text(dom, id, &mut text_buffer) == original.trim());
        if !has_matching_heading && let Some(position) = original.rfind(": ") {
            current = Cow::Borrowed(&original[position + 2..]);
            if word_count(&current) < 3
                && let Some(first) = original.find(": ")
            {
                current = if word_count(&original[..first]) <= 5 {
                    Cow::Borrowed(&original[first + 2..])
                } else {
                    Cow::Borrowed(&original)
                };
            }
        }
    } else if !(15..=150).contains(&original.chars().count()) {
        let headings: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::H1))
            .collect();
        if headings.len() == 1 {
            let mut normalized = String::new();
            get_normalized_inner_text(dom, headings[0], &mut normalized);
            current = Cow::Owned(normalized);
        }
    }
    let mut current = normalize_whitespace(current.trim());
    if word_count(&current) <= 4 {
        let without_separator = remove_title_separators(&original);
        if !hierarchical || word_count(&current) != word_count(&without_separator).saturating_sub(1)
        {
            current = original;
        }
    }
    current
}

pub(crate) fn text_similarity(first: &str, second: &str) -> f64 {
    let first = first.to_lowercase();
    let second = second.to_lowercase();
    let set: SmallVec<[&str; 16]> = split_word_tokens(&first).collect();
    let tokens: SmallVec<[&str; 16]> = split_word_tokens(&second).collect();
    if set.is_empty() || tokens.is_empty() {
        return 0.0;
    }
    let total = tokens
        .iter()
        .map(|value| value.chars().count())
        .sum::<usize>()
        + tokens.len().saturating_sub(1);
    let unique_tokens: SmallVec<[&&str; 16]> = tokens
        .iter()
        .filter(|value| !set.contains(*value))
        .collect();
    let unique = unique_tokens
        .iter()
        .map(|value| value.chars().count())
        .sum::<usize>()
        + unique_tokens.len().saturating_sub(1);
    1.0 - unique as f64 / total as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(html: &str, url: Option<&str>, structured: bool) -> Metadata {
        let dom = Dom::parse_document(html).unwrap();
        let data = if structured {
            StructuredData::parse(&dom, &ParseBudget::default()).unwrap()
        } else {
            StructuredData::default()
        };
        let base = url.map(Url::parse).transpose().unwrap();
        let title = get_page_title(&dom);
        discover(&dom, &data, &title, base.as_ref(), base.as_ref())
    }

    #[test]
    fn visible_brand_precedes_the_hostname_but_not_explicit_metadata() {
        let result = metadata(
            r#"<title>Example Journal</title><body><header><a href='/' class='brand'>Example Journal</a></header><main>Content</main></body>"#,
            Some("https://news.example.test/"),
            false,
        );
        assert_eq!(result.title.as_deref(), Some("Example Journal"));
        assert_eq!(result.site_name.as_deref(), Some("Example Journal"));

        let explicit = metadata(
            r#"<title>Visible Brand</title><meta property='og:site_name' content='Explicit Publisher'><body><header><a href='/' class='brand'>Visible Brand</a></header></body>"#,
            Some("https://example.test/article"),
            false,
        );
        assert_eq!(explicit.site_name.as_deref(), Some("Explicit Publisher"));

        for url in ["https://example.test/article", "https://example.test/"] {
            let article = metadata(
                r#"<title>Article title</title><body><header><a href='/article'>Article title</a></header></body>"#,
                Some(url),
                false,
            );
            assert_eq!(article.site_name.as_deref(), Some("example.test"));
        }
    }

    #[test]
    fn structured_data_parses_arrays_graphs_and_later_valid_blocks() {
        let dom = Dom::parse_document(
            r#"<script type="application/ld+json">not json</script>
            <script type="application/ld+json">[
              {"@context":"https://schema.org","@type":"WebSite","name":"Example"},
              {"@context":"https://schema.org","@graph":[
                {"@type":"Article","headline":"Graph title","author":[{"name":"Ada"},{"name":"Grace"}],"articleBody":"Full article body","text":"Alternate article text"}
              ]}
            ]</script>"#,
        )
        .unwrap();
        let data = StructuredData::parse(&dom, &ParseBudget::default()).unwrap();

        assert_eq!(data.items.len(), 2);
        assert_eq!(
            data.article_texts().collect::<Vec<_>>(),
            ["Full article body", "Alternate article text"]
        );
        let result = discover(&dom, &data, "", None, None);
        assert_eq!(result.title.as_deref(), Some("Graph title"));
        assert_eq!(result.site_name.as_deref(), Some("Example"));
        assert_eq!(result.authors, ["Ada", "Grace"]);
    }

    #[test]
    fn resolves_metadata_candidates_and_relative_urls() {
        let result = metadata(
            r#"<html lang="en_US"><head><title>Post | Example</title>
            <meta property="og:title" content="Post | Example">
            <meta property="og:site_name" content="Example">
            <meta name="citation_author" content="Ada Lovelace">
            <meta name="citation_author" content="Grace Hopper">
            <meta property="article:tag" content="Rust">
            <meta name="keywords" content="rust, HTML">
            <link rel="canonical" href="/post"><link rel="icon" href="icons/site.png">
            </head><body><h1>Post</h1></body></html>"#,
            Some("https://example.com/docs/page"),
            true,
        );

        assert_eq!(result.title.as_deref(), Some("Post"));
        let cleaned_title = metadata(
            r#"<title>Post | Example</title><meta property="og:site_name" content="Example"><main>Text</main>"#,
            None,
            false,
        );
        assert_eq!(cleaned_title.title.as_deref(), Some("Post"));
        assert_eq!(result.authors, ["Ada Lovelace", "Grace Hopper"]);
        assert_eq!(result.tags, ["Rust", "HTML"]);
        assert_eq!(result.language.as_deref(), Some("en-US"));
        assert_eq!(
            result.canonical_url.as_deref(),
            Some("https://example.com/post")
        );
        assert_eq!(
            result.favicon.as_deref(),
            Some("https://example.com/docs/icons/site.png")
        );
    }

    #[test]
    fn rejects_placeholders_and_deduplicates_values() {
        let result = metadata(
            r#"<head><meta property="og:title" content="{{title}}">
            <meta name="title" content="Real title">
            <meta name="author" content="By Ada"><meta name="citation_author" content="ada">
            <meta name="keywords" content="Rust, rust, --">
            <meta name="citation_keywords" content="HTML, Extraction"></head><body><p>Text</p></body>"#,
            None,
            true,
        );

        assert_eq!(result.title.as_deref(), Some("Real title"));
        assert_eq!(result.authors, ["ada"]);
        assert_eq!(result.tags, ["Rust", "HTML", "Extraction"]);
        assert_eq!(normalize_person("By Ada").as_deref(), Some("Ada"));
        assert_eq!(normalize_person("By\u{a0}Ada").as_deref(), Some("Ada"));
        assert_eq!(
            normalize_person("Byron Smith").as_deref(),
            Some("Byron Smith")
        );
        assert_eq!(normalize_person("李").as_deref(), Some("李"));
        assert_eq!(
            normalize_person("Mary May Smith").as_deref(),
            Some("Mary May Smith")
        );
        assert_eq!(
            normalize_person("Mary May Smith May 5, 2026").as_deref(),
            Some("Mary May Smith")
        );
        assert_eq!(
            normalize_person("Jane Doe|Senior Editor").as_deref(),
            Some("Jane Doe")
        );
        assert_eq!(
            normalize_person("Jane Doe published August 13, 2026").as_deref(),
            Some("Jane Doe")
        );
        assert_eq!(
            normalize_person("Published by Jane Doe").as_deref(),
            Some("Jane Doe")
        );
        assert_eq!(
            normalize_person("Sarah Archer 1:39 PM ET").as_deref(),
            Some("Sarah Archer")
        );
        assert_eq!(normalize_person("Last Updated: January 7, 2025"), None);
        assert_eq!(
            normalize_person("| Carl Sverre , Senior Software Engineer").as_deref(),
            Some("Carl Sverre")
        );
        assert_eq!(
            normalize_person("Contact editor@example.com Editorial team").as_deref(),
            Some("Contact editor@example.com Editorial team")
        );
        assert_eq!(
            normalize_person("Daroc AldenJuly 29, 2026 LSFMM+BPF").as_deref(),
            Some("Daroc Alden")
        );
        assert_eq!(normalize_person("By --"), None);
        assert_eq!(normalize_person("By "), None);
        assert!(metadata_values_equal("Émilie", "E\u{301}MILIE"));
        assert!(metadata_values_equal("Straße", "STRASSE"));
        assert!(metadata_values_equal("ΟΣ", "ος"));
    }

    #[test]
    fn uses_agreement_and_conservative_dom_fallbacks() {
        let result = metadata(
            r#"<html><head><title>Fallback title</title>
            <meta property="og:title" content="Agreed title">
            <meta property="og:image" content="/hero.jpg">
            <script type="application/ld+json; charset=utf-8">[
              {"@context":"https://schema.org","@type":"Article","headline":"Agreed title","sourceOrganization":{"name":"Source Site"}},
              {"@type":"WebPage","name":"Other object"}
            ]</script></head><body>
            <header><h1>Agreed title</h1><time datetime="2025-02-03">Today</time><p class="byline">By Jane Doe</p></header>
            <aside class="related"><time datetime="1999-01-01">Old card</time></aside>
            </body></html>"#,
            Some("https://example.com/page"),
            true,
        );

        assert_eq!(result.title.as_deref(), Some("Agreed title"));
        assert_eq!(result.site_name.as_deref(), Some("Source Site"));
        assert_eq!(
            result.image.as_deref(),
            Some("https://example.com/hero.jpg")
        );
        assert_eq!(result.published_time.as_deref(), Some("2025-02-03"));
    }

    #[test]
    fn ignores_a_related_card_date_without_a_nearby_primary_heading() {
        let result = metadata(
            r#"<main><h1>Page</h1><p>Body</p>
            <aside><h2>Related</h2><time datetime="1999-01-01">Old</time></aside></main>"#,
            None,
            false,
        );

        assert!(result.published_time.is_none());

        let result = metadata(
            r#"<main><h1>Page</h1><p>1</p><p>2</p><p>3</p><p>4</p><p>5</p><p>6</p><p>7</p><time datetime="1999-01-01">Related card</time></main>"#,
            None,
            false,
        );
        assert!(result.published_time.is_none());
    }

    #[test]
    fn does_not_treat_a_byline_action_link_as_a_section() {
        let result = metadata(
            r#"<article><h1>Page</h1><div class="byline"><span class="author-name">Jane Doe</span><time itemprop="datePublished" datetime="2026-08-13">August 13, 2026</time><a href="/subscribe">Subscribe</a></div><p>Article body.</p></article>"#,
            None,
            false,
        );

        assert_eq!(result.authors, ["Jane Doe"]);
        assert_eq!(result.published_time.as_deref(), Some("2026-08-13"));
        assert!(result.section.is_none());
    }

    #[test]
    fn does_not_treat_a_multi_author_wrapper_as_one_author() {
        let result = metadata(
            r#"<article><h1>Page</h1><div class="authors">By <span class="author"><a href="/author/ada">Ada Lovelace</a></span> and <span class="author"><a href="/author/grace">Grace Hopper</a></span></div><p>Article body.</p></article>"#,
            None,
            false,
        );

        assert_eq!(result.authors, ["Ada Lovelace", "Grace Hopper"]);
    }

    #[test]
    fn ignores_author_cards_as_page_authors() {
        let result = metadata(
            r#"<article><h1>Page</h1><p>Article body.</p><section class="profile-card-grid"><div class="author-card"><img src="avatar.jpg" alt="Pat Example" /><p>Pat Example, founder and engineer.</p></div></section></article>"#,
            None,
            false,
        );

        assert!(result.authors.is_empty());
    }

    #[test]
    fn ignores_comment_authors_as_page_authors() {
        let result = metadata(
            r#"<article><h1>Page</h1><p>Article body.</p><div class="comment-author"><a href="/people/commenter">Commenter</a></div><div class="reply-author">Reply Writer</div></article>"#,
            None,
            false,
        );

        assert!(result.authors.is_empty());
    }

    #[test]
    fn excludes_a_bem_byline_role_from_the_author_name() {
        let result = metadata(
            r#"<article><h1>Page</h1><div class="byline"><a class="byline__author"><img class="byline__avatar" src="avatar.jpg" />Mark Di Stefano</a><div class="byline__title">News Reporter</div></div><p>Article body.</p></article>"#,
            None,
            false,
        );

        assert_eq!(result.authors, ["Mark Di Stefano"]);
    }

    #[test]
    fn keeps_dates_on_simple_author_containers() {
        let result = metadata(
            r#"<article><h1>Page</h1><div class="author">Jane Doe <time>August 13, 2026</time></div><p>Article body.</p></article>"#,
            None,
            false,
        );

        assert_eq!(result.authors, ["Jane Doe"]);
        assert_eq!(result.published_time.as_deref(), Some("August 13, 2026"));
    }

    #[test]
    fn excludes_nested_roles_from_a_preferred_author_name() {
        let result = metadata(
            r#"<article><h1>Page</h1><div class="byline"><span class="author-name">Jane Doe <span class="author-title">Editor</span></span></div><p>Article body.</p></article>"#,
            None,
            false,
        );

        assert_eq!(result.authors, ["Jane Doe"]);
    }

    #[test]
    fn handles_non_ascii_whitespace_before_a_byline_time() {
        assert_eq!(
            normalize_person("Jane Doe\u{a0}1:39 PM ET").as_deref(),
            Some("Jane Doe")
        );
    }

    #[test]
    fn prefers_an_article_heading_over_a_site_heading_in_main() {
        let result = metadata(
            r#"<main><h1>Acme</h1><article><h1>Real Story</h1><p>Article body.</p></article></main>"#,
            None,
            false,
        );

        assert_eq!(result.title.as_deref(), Some("Real Story"));
    }

    #[test]
    fn hidden_and_empty_headings_do_not_supply_a_title() {
        let result = metadata(
            r#"<main><h1 style="display:none">Hidden title</h1><h1> </h1><p>Body.</p></main>"#,
            None,
            false,
        );

        assert!(result.title.is_none());
    }

    #[test]
    fn written_date_uses_document_order_instead_of_month_order() {
        assert_eq!(
            extract_written_date("Published July 29, 2026 Updated January 2, 2027").as_deref(),
            Some("July 29, 2026")
        );
    }

    #[test]
    fn collects_dublin_core_itemprop_and_author_links() {
        let result = metadata(
            r#"<html><head>
            <meta name="DCTERMS.title" content="Dublin title">
            <meta itemprop="datePublished" content="2025-03-04">
            </head><body><header><h1>Heading</h1>
            <span itemprop="author"><meta itemprop="name" content="Item Author"></span>
            <a rel="author" href="/people/rel">Rel Author</a>
            </header><main>Text</main></body></html>"#,
            None,
            false,
        );

        assert_eq!(result.title.as_deref(), Some("Dublin title"));
        assert_eq!(result.authors, ["Item Author", "Rel Author"]);
        assert_eq!(result.published_time.as_deref(), Some("2025-03-04"));
    }

    #[test]
    fn collects_a_nearby_author_profile_link() {
        let result = metadata(
            r#"<header><h1>Heading</h1><div><span><a href="/author/ada">Ada</a></span></div></header><main>Text</main>"#,
            None,
            false,
        );

        assert_eq!(result.authors, ["Ada"]);
    }

    #[test]
    fn ambiguous_structured_items_do_not_provide_content_hints() {
        let dom = Dom::parse_document(
            r#"<script type="application/ld+json">[
                {"@context":"https://schema.org","@type":"Article","description":"First unrelated description","author":{"name":"First Author"},"articleBody":"First possible article body with enough useful words."},
                {"@context":"https://schema.org","@type":"Article","description":"Second unrelated description","author":{"name":"Second Author"},"articleBody":"Second possible article body with enough useful words."}
            ]</script>"#,
        )
        .unwrap();
        let data = StructuredData::parse(&dom, &ParseBudget::default()).unwrap();

        assert!(data.primary_texts("", None).next().is_none());
        let resolved = discover(&dom, &data, "", None, None);
        assert!(resolved.description.is_none());
        assert!(resolved.authors.is_empty());
    }

    #[test]
    fn duplicate_structured_articles_retain_metadata_and_content_hints() {
        let dom = Dom::parse_document(
            r#"<title>Repeated article</title>
            <script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","headline":"Repeated article","author":{"name":"Ada"},"datePublished":"2025-04-05","articleBody":"The repeated article body has enough useful words to identify its content."}</script>
            <script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","headline":"Repeated article","author":{"name":"Ada"},"datePublished":"2025-04-05","articleBody":"The repeated article body has enough useful words to identify its content."}</script>"#,
        )
        .unwrap();
        let data = StructuredData::parse(&dom, &ParseBudget::default()).unwrap();
        let title = get_page_title(&dom);
        let resolved = discover(&dom, &data, &title, None, None);

        assert_eq!(resolved.authors, ["Ada"]);
        assert_eq!(resolved.published_time.as_deref(), Some("2025-04-05"));
        assert_eq!(
            data.primary_texts(&title, None).collect::<Vec<_>>(),
            ["The repeated article body has enough useful words to identify its content."]
        );
    }

    #[test]
    fn structured_item_fingerprints_are_stable_and_bounded() {
        let first = serde_json::json!({
            "@type": "Article",
            "headline": "Same title",
            "articleBody": "Body"
        });
        let duplicate = first.clone();
        let different = serde_json::json!({
            "@type": "Article",
            "headline": "Different title",
            "articleBody": "Body"
        });

        assert_eq!(
            structured_item_fingerprint(&first),
            structured_item_fingerprint(&duplicate)
        );
        assert_ne!(
            structured_item_fingerprint(&first),
            structured_item_fingerprint(&different)
        );
    }

    #[test]
    fn fingerprint_collisions_do_not_merge_distinct_structured_items() {
        let mut first = serde_json::Map::new();
        first.insert("@type".to_owned(), Value::String("Article".to_owned()));
        for index in 0..300 {
            first.insert(format!("a{index:03}"), Value::Null);
        }
        first.insert(
            "headline".to_owned(),
            Value::String("Related article".to_owned()),
        );
        let mut second = first.clone();
        second.insert(
            "headline".to_owned(),
            Value::String("Primary article".to_owned()),
        );
        let first = Value::Object(first);
        let second = Value::Object(second);

        assert_eq!(
            structured_item_fingerprint(&first),
            structured_item_fingerprint(&second)
        );
        assert_eq!(
            select_unique_structured_item([&first, &second], "Primary article", None,),
            Some(&second)
        );
    }

    #[test]
    fn overflowing_collision_buckets_are_ambiguous() {
        let mut items = Vec::new();
        for index in 0..(FINGERPRINT_COLLISION_LIMIT + 4) {
            let mut item = serde_json::Map::new();
            item.insert("@type".to_owned(), Value::String("Article".to_owned()));
            for field in 0..300 {
                item.insert(format!("a{field:03}"), Value::Null);
            }
            item.insert(
                "headline".to_owned(),
                Value::String(format!("Article {index}")),
            );
            items.push(Value::Object(item));
        }

        let fingerprint = structured_item_fingerprint(&items[0]);
        assert!(
            items
                .iter()
                .all(|item| structured_item_fingerprint(item) == fingerprint)
        );
        assert_eq!(
            select_unique_structured_item(items.iter(), "Article 11", None),
            None
        );
    }

    #[test]
    fn overflow_duplicates_do_not_change_the_winner_count() {
        let mut items = Vec::new();
        for index in 0..FINGERPRINT_COLLISION_LIMIT {
            let mut item = serde_json::Map::new();
            item.insert("@type".to_owned(), Value::String("Article".to_owned()));
            for field in 0..300 {
                item.insert(format!("a{field:03}"), Value::Null);
            }
            item.insert(
                "headline".to_owned(),
                Value::String(format!("Related article {index}")),
            );
            items.push(Value::Object(item));
        }
        let mut primary = items[0].clone();
        primary["headline"] = Value::String("Primary article".to_owned());
        items.push(primary.clone());
        items.push(primary);

        assert_eq!(
            select_unique_structured_item(items.iter(), "Primary article", None),
            None
        );
    }

    #[test]
    fn selects_the_structured_object_that_matches_the_page() {
        let result = metadata(
            r#"<title>Primary page</title>
            <script type="application/ld+json">{"@context":{"schema":"https://schema.org/"},"@graph":[
              {"@type":"schema:Article","headline":"Related story","url":"https://example.com/related"},
              {"@type":"schema:WebPage","mainEntity":{"@type":"schema:Article","headline":"Primary page","url":"https://example.com/current","author":{"name":"Primary Author"},"publisher":[{"name":"Primary Site"},{"name":"Secondary Publisher"}]}},
              {"@type":"schema:Organization","name":"Unrelated Sponsor"}
            ]}</script><main>Text</main>"#,
            Some("https://example.com/current"),
            true,
        );

        assert_eq!(result.title.as_deref(), Some("Primary page"));
        assert_eq!(result.authors, ["Primary Author"]);
        assert_eq!(result.site_name.as_deref(), Some("Primary Site"));
    }

    #[test]
    fn collects_general_schema_pages_and_decodes_named_entities() {
        let result = metadata(
            r#"<title>Crème — soup</title><script type="application/ld+json">[
              {"@context":"https://schema.org","@type":"Article","headline":"Unrelated story"},
              {"@context":"https://schema.org","@type":"Recipe","name":"Crème &mdash; soup","description":"Easy&nbsp;recipe","author":{"name":"Cook"},"image":{"url":"/soup.jpg"}}
            ]</script><main>Text</main>"#,
            Some("https://example.com/recipes/page"),
            true,
        );

        assert_eq!(result.title.as_deref(), Some("Crème — soup"));
        assert_eq!(result.description.as_deref(), Some("Easy\u{a0}recipe"));
        assert_eq!(result.authors, ["Cook"]);
        assert_eq!(
            result.image.as_deref(),
            Some("https://example.com/soup.jpg")
        );
    }

    #[test]
    fn ignores_json_ld_from_an_unrelated_vocabulary() {
        let result = metadata(
            r#"<script type="application/ld+json">{"@context":"https://example.org/vocab","@type":"Article","headline":"Wrong title"}</script><main>Text</main>"#,
            None,
            true,
        );

        assert!(result.title.is_none());
    }

    #[test]
    fn structured_data_can_be_omitted() {
        let html = r#"<script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","headline":"Schema title"}</script><main>Text</main>"#;
        assert_eq!(
            metadata(html, None, true).title.as_deref(),
            Some("Schema title")
        );
        assert!(metadata(html, None, false).title.is_none());
    }
}
