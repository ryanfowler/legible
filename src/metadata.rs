//! Metadata discovery and structured-data parsing.

use crate::constants::{
    find_last_title_separator_start, has_hierarchical_title_separator, has_title_separator,
    is_json_ld_article_type, is_schema_org_url, normalize_whitespace, remove_title_first_part,
    remove_title_separators, split_word_tokens,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::{get_inner_text, get_inner_text_owned, get_normalized_inner_text};
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
}

/// Parsed schema.org data. It remains available after metadata discovery so a
/// later extraction stage can use `articleBody` and `text` as location hints.
#[derive(Debug, Clone, Default)]
pub(crate) struct StructuredData {
    items: Vec<Value>,
}

impl StructuredData {
    pub(crate) fn parse(dom: &Dom) -> Self {
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
        for id in scripts {
            let content = script_text(dom, id, &mut buffer)
                .trim()
                .trim_start_matches("<![CDATA[")
                .trim_end_matches("]]>")
                .trim();
            let Ok(value) = serde_json::from_str::<Value>(content) else {
                continue;
            };
            collect_structured_items(&value, false, &mut items);
        }
        Self { items }
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
}

fn script_text<'a>(dom: &'a Dom, id: NodeId, buffer: &'a mut String) -> &'a str {
    let mut children = dom.children(id);
    let first = children.next();
    if let Some(node) = first
        && children.next().is_none()
        && let Some(text) = dom.text_node(node)
    {
        return text;
    }
    buffer.clear();
    dom.append_text(id, buffer);
    buffer
}

fn collect_structured_items(value: &Value, inherited_schema: bool, out: &mut Vec<Value>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_structured_items(value, inherited_schema, out);
            }
        }
        Value::Object(object) => {
            let schema = inherited_schema
                || object.get("@context").is_some_and(is_schema_context)
                || object
                    .get("@type")
                    .is_some_and(|kind| json_types(kind).any(is_absolute_schema_type));
            if !schema {
                return;
            }
            if object.get("@type").is_some() {
                out.push(value.clone());
            }
            if let Some(graph) = object.get("@graph") {
                collect_structured_items(graph, true, out);
            }
            for (key, nested) in object {
                if !matches!(key.as_str(), "@context" | "@graph" | "@type") {
                    collect_structured_items(nested, true, out);
                }
            }
        }
        _ => {}
    }
}

fn is_schema_context(value: &Value) -> bool {
    match value {
        Value::String(value) => is_schema_url(value),
        Value::Array(values) => values.iter().any(is_schema_context),
        Value::Object(values) => {
            values
                .get("@vocab")
                .and_then(Value::as_str)
                .is_some_and(is_schema_url)
                || values.values().filter_map(Value::as_str).any(is_schema_url)
        }
        _ => false,
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetadataSource {
    JsonLd,
    OpenGraph,
    Twitter,
    DublinCore,
    Citation,
    HtmlMeta,
    HtmlElement,
    LinkElement,
    Inferred,
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
pub(crate) fn discover(
    dom: &Dom,
    structured: &StructuredData,
    document_title: &str,
    base_url: Option<&Url>,
    source_url: Option<&Url>,
) -> Metadata {
    let mut candidates = CandidateSet::default();
    let identity_title = content_identity_title(dom, document_title);
    collect_structured_candidates(structured, &identity_title, source_url, &mut candidates);
    collect_meta_candidates(dom, &mut candidates);
    collect_link_candidates(dom, &mut candidates);
    collect_element_candidates(dom, document_title, &mut candidates);

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

    resolve_candidates(candidates, base_url)
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
        add_json_string(out, |set| &mut set.title, item.get("name"), 90);
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

fn hash_json_value(value: &Value, hasher: &mut impl Hasher, budget: &mut FingerprintBudget) {
    if budget.values == 0 {
        0xff_u8.hash(hasher);
        return;
    }
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
            for value in values {
                if budget.values == 0 {
                    break;
                }
                hash_json_value(value, hasher, budget);
            }
        }
        Value::Object(values) => {
            5_u8.hash(hasher);
            values.len().hash(hasher);
            for (key, value) in values {
                if budget.values == 0 {
                    break;
                }
                hash_bounded_bytes(key.as_bytes(), hasher, budget);
                hash_json_value(value, hasher, budget);
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
    if let Some(values) = value.as_array() {
        for value in values {
            collect_json_names(value, add);
        }
    } else if let Some(name) = json_name(value) {
        add(name);
    }
}

fn collect_json_keywords(value: Option<&Value>, out: &mut CandidateSet) {
    let Some(value) = value else { return };
    if let Some(values) = value.as_array() {
        for value in values {
            collect_json_keywords(Some(value), out);
        }
    } else if let Some(value) = value.as_str() {
        for tag in value.split(',') {
            out.add(|set| &mut set.tags, tag, MetadataSource::JsonLd, 88);
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
    let primary_heading = elements
        .iter()
        .copied()
        .find(|&id| dom.tag(id) == Some(Tag::H1));
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
            out.add(|set| &mut set.title, text, MetadataSource::HtmlElement, 78);
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
                && primary_heading.is_some_and(|heading| is_near_heading(dom, id, heading));
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
                .or_else(|| name_node.and_then(|node| dom.attr(node, AttrName::Content)))
                .unwrap_or_else(|| get_inner_text(dom, name_node.unwrap_or(id), &mut text_buffer));
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
                    && primary_heading.is_some_and(|heading| is_near_heading(dom, id, heading)))
            {
                let value = get_inner_text(dom, id, &mut text_buffer);
                out.add(
                    |set| &mut set.authors,
                    value,
                    MetadataSource::LinkElement,
                    if rel_author { 82 } else { 68 },
                );
            }
        }
        let author_element = [dom.attr(id, AttrName::Class), dom.attr(id, AttrName::Id)]
            .into_iter()
            .flatten()
            .flat_map(|value| value.split_ascii_whitespace())
            .any(|token| {
                token.eq_ignore_ascii_case("author")
                    || token.eq_ignore_ascii_case("byline")
                    || token.eq_ignore_ascii_case("p-author")
            });
        if author_element
            && primary_heading.is_some_and(|heading| is_near_heading(dom, id, heading))
        {
            let value = get_inner_text(dom, id, &mut text_buffer);
            if value.chars().count() <= 120 {
                out.add(|set| &mut set.authors, value, MetadataSource::Inferred, 62);
            }
        }
    }
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

fn resolve_candidates(mut candidates: CandidateSet, base_url: Option<&Url>) -> Metadata {
    normalize_all(&mut candidates.title, normalize_text);
    normalize_all(&mut candidates.description, normalize_text);
    normalize_all(&mut candidates.authors, normalize_person);
    normalize_all(&mut candidates.site_name, normalize_text);
    normalize_all(&mut candidates.published_time, normalize_text);
    normalize_all(&mut candidates.modified_time, normalize_text);
    normalize_all(&mut candidates.language, normalize_language);
    normalize_all(&mut candidates.direction, normalize_direction);
    normalize_all(&mut candidates.section, normalize_text);
    normalize_all(&mut candidates.tags, normalize_text);
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
            title = resolve_one_excluding(&candidates.title, title_value);
        } else if let Some(stripped) = strip_site_affix(title_value, site) {
            title = Some(stripped);
        }
    }

    Metadata {
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

fn normalize_person(value: &str) -> Option<String> {
    let value = normalize_text(value)?;
    let without_prefix = if value.eq_ignore_ascii_case("by") {
        ""
    } else {
        value
            .get(..3)
            .filter(|prefix| prefix.eq_ignore_ascii_case("by "))
            .map_or(value.as_str(), |_| &value[3..])
    };
    let value = normalize_text(without_prefix)?;
    if value.eq_ignore_ascii_case("author")
        || value.eq_ignore_ascii_case("authors")
        || Url::parse(&value).is_ok()
        || value.chars().count() > 120
    {
        None
    } else {
        Some(value)
    }
}

fn normalize_language(value: &str) -> Option<String> {
    let value = normalize_text(value)?;
    Some(value.replace('_', "-"))
}

fn normalize_direction(value: &str) -> Option<String> {
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
        let agreements = candidates
            .iter()
            .filter(|other| {
                other.source != candidate.source
                    && other.value.eq_ignore_ascii_case(&candidate.value)
            })
            .count() as u16;
        let score = u16::from(candidate.confidence) + agreements.min(3) * 5;
        if best.is_none_or(|(current, current_score)| {
            score > current_score || (score == current_score && candidate.order < current.order)
        }) {
            best = Some((candidate, score));
        }
    }
    best.map(|(candidate, _)| candidate)
}

fn resolve_one_excluding(candidates: &[MetadataCandidate], excluded: &str) -> Option<String> {
    let filtered: Vec<_> = candidates
        .iter()
        .filter(|candidate| !candidate.value.eq_ignore_ascii_case(excluded))
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
    let mut selected: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.confidence.saturating_add(8) >= maximum)
        .cloned()
        .collect();
    for candidate in &mut selected {
        if let Some(first) = candidates
            .iter()
            .find(|value| value.value.eq_ignore_ascii_case(&candidate.value))
        {
            candidate.value.clone_from(&first.value);
            candidate.order = first.order;
        }
    }
    resolve_many(&selected).into_iter().take(10).collect()
}

fn resolve_many(candidates: &[MetadataCandidate]) -> Vec<String> {
    let mut candidates = candidates.to_vec();
    candidates.sort_by_key(|candidate| candidate.order);
    let mut values = Vec::new();
    for candidate in candidates {
        if !values
            .iter()
            .any(|value: &String| value.eq_ignore_ascii_case(&candidate.value))
        {
            values.push(candidate.value);
        }
    }
    values
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
            StructuredData::parse(&dom)
        } else {
            StructuredData::default()
        };
        let base = url.map(Url::parse).transpose().unwrap();
        let title = get_page_title(&dom);
        discover(&dom, &data, &title, base.as_ref(), base.as_ref())
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
        let data = StructuredData::parse(&dom);

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
        assert_eq!(result.authors, ["Ada"]);
        assert_eq!(result.tags, ["Rust", "HTML", "Extraction"]);
        assert_eq!(normalize_person("By Ada").as_deref(), Some("Ada"));
        assert_eq!(normalize_person("By --"), None);
        assert_eq!(normalize_person("By "), None);
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
        let data = StructuredData::parse(&dom);

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
        let data = StructuredData::parse(&dom);
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
