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
use crate::tokens::{contains_ascii_case_insensitive, has_any_token, has_token};
use serde_json::Value;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use url::Url;

/// Metadata discovered in the source page.
///
/// Legible combines JSON-LD, standard metadata, and visible HTML evidence.
/// Missing values are `None` for optional fields and empty for list fields.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    #[cfg_attr(feature = "serde", serde(skip))]
    pub(crate) has_source_author: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
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
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn parse(dom: &Dom, budget: &ParseBudget) -> Result<Self, StructuredDataError> {
        let scripts: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| {
                dom.tag(id) == Some(Tag::Script)
                    && dom
                        .attr(id, AttrName::Type)
                        .is_some_and(crate::dom::is_json_ld_script_type)
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
            let Ok(value) = serde_json::from_str::<Value>(content) else {
                continue;
            };
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

    /// Counts structured items that can describe a document or application.
    /// This is broader than `primary_texts`: an application description is
    /// useful root evidence even when it is not an article-body hint.
    pub(crate) fn document_evidence_count(
        &self,
        document_title: &str,
        source_url: Option<&Url>,
    ) -> usize {
        self.items
            .iter()
            .filter(|item| {
                let typed = item.get("@type").is_some_and(|kind| {
                    json_types(kind).any(|kind| {
                        is_article_type(kind)
                            || is_general_content_type(kind)
                            || ["Dataset", "Product", "SoftwareApplication"]
                                .iter()
                                .any(|expected| schema_type(kind, expected))
                    })
                });
                let described = ["description", "headline", "name"].iter().any(|field| {
                    item.get(*field)
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty())
                });
                typed && described && structured_item_score(item, document_title, source_url) >= 45
            })
            .count()
    }

    fn has_article_item(&self) -> bool {
        self.items.iter().any(|item| {
            item.get("@type")
                .is_some_and(|kind| json_types(kind).any(is_article_type))
        })
    }

    pub(crate) fn primary_item_matches(
        &self,
        document_title: &str,
        source_url: Option<&Url>,
    ) -> bool {
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
        self.items.clone()
    }

    /// Returns compact fields from the primary job posting, when the page
    /// exposes Schema.org job data. The description itself stays in the HTML
    /// extraction path. These fields fill the common gap where a job card is
    /// separate from the selected job description.
    pub(crate) fn job_posting_fields(
        &self,
        document_title: &str,
        source_url: Option<&Url>,
    ) -> Vec<(String, String)> {
        let Some(item) =
            select_job_posting(self.typed_items("JobPosting"), document_title, source_url)
        else {
            return Vec::new();
        };
        let mut fields = Vec::with_capacity(5);
        add_job_field(
            &mut fields,
            "Role",
            item.get("title").and_then(Value::as_str),
        );
        add_job_field(
            &mut fields,
            "Company",
            item.get("hiringOrganization").and_then(job_name_value),
        );
        if let Some(value) = item.get("jobLocation").and_then(job_location_value) {
            add_job_field(&mut fields, "Location", Some(&value));
        }
        add_job_field(
            &mut fields,
            "Employment",
            item.get("employmentType")
                .and_then(job_string_value)
                .as_deref(),
        );
        if let Some(value) = item.get("baseSalary").and_then(job_salary_value) {
            add_job_field(&mut fields, "Salary", Some(&value));
        }
        if item
            .get("jobLocationType")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("TELECOMMUTE"))
        {
            add_job_field(&mut fields, "Workplace", Some("Remote"));
        }
        fields
    }
}

fn add_job_field(fields: &mut Vec<(String, String)>, label: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    fields.push((label.to_owned(), value.to_owned()));
}

fn select_job_posting<'a>(
    items: impl IntoIterator<Item = &'a Value>,
    document_title: &str,
    source_url: Option<&Url>,
) -> Option<&'a Value> {
    let mut unique_by_fingerprint = HashMap::<u64, Vec<&Value>>::new();
    for item in items {
        let bucket = unique_by_fingerprint
            .entry(structured_item_fingerprint(item))
            .or_default();
        if !bucket.contains(&item) {
            bucket.push(item);
        }
    }
    let postings = unique_by_fingerprint
        .into_values()
        .flatten()
        .collect::<Vec<_>>();
    if postings.len() == 1 {
        return postings.into_iter().next();
    }
    let mut matches = postings.into_iter().filter(|item| {
        let title_match = item
            .get("title")
            .and_then(Value::as_str)
            .is_some_and(|title| {
                !document_title.is_empty()
                    && text_similarity(title, document_title) >= 0.6
                    && text_similarity(document_title, title) >= 0.8
            });
        let url_match = json_url(item)
            .and_then(|value| Url::parse(value).ok())
            .zip(source_url)
            .is_some_and(|(candidate, source)| same_host_and_path(&candidate, source));
        title_match || url_match
    });
    let selected = matches.next()?;
    matches.next().is_none().then_some(selected)
}

fn job_string_value(value: &Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        return Some(value.to_owned());
    }
    let values = value.as_array()?.iter().filter_map(Value::as_str);
    let values = values
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join(", "))
}

fn job_name_value(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("name").and_then(Value::as_str))
}

fn job_location_value(value: &Value) -> Option<String> {
    let locations = value
        .as_array()
        .map(|values| values.iter().collect::<Vec<_>>())
        .unwrap_or_else(|| vec![value]);
    let values = locations
        .into_iter()
        .filter_map(job_single_location_value)
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join("; "))
}

fn job_single_location_value(location: &Value) -> Option<String> {
    let address = location.get("address").unwrap_or(location);
    let mut parts = [None; 4];
    parts[0] = address.get("streetAddress").and_then(Value::as_str);
    parts[1] = address.get("addressLocality").and_then(Value::as_str);
    parts[2] = address.get("addressRegion").and_then(Value::as_str);
    parts[3] = address.get("addressCountry").and_then(|country| {
        country
            .as_str()
            .or_else(|| country.get("name").and_then(Value::as_str))
    });
    let value = parts
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    (!value.is_empty()).then(|| value.join(", "))
}

fn job_salary_value(value: &Value) -> Option<String> {
    let currency = value.get("currency").and_then(Value::as_str).unwrap_or("");
    let salary_value = value.get("value").unwrap_or(value);
    let unit = salary_value
        .get("unitText")
        .or_else(|| value.get("unitText"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|unit| !unit.is_empty());
    let format_number = |number: &Value| -> Option<String> {
        number.as_str().map(str::to_owned).or_else(|| {
            number.as_f64().map(|value| {
                if value.fract() == 0.0 {
                    format!("{value:.0}")
                } else {
                    value.to_string()
                }
            })
        })
    };
    let formatted = if salary_value.is_object() {
        let min = salary_value.get("minValue").and_then(format_number);
        let max = salary_value.get("maxValue").and_then(format_number);
        match (min, max) {
            (Some(min), Some(max)) => Some(format_job_salary(currency, &min, &max, unit)),
            (Some(min), None) => Some(format_job_salary(currency, &min, "", unit)),
            (None, Some(max)) => Some(format_job_salary(currency, &max, "", unit)),
            _ => None,
        }
    } else {
        format_number(salary_value).map(|number| format_job_salary(currency, &number, "", unit))
    }?;
    Some(formatted)
}

fn format_job_salary(currency: &str, first: &str, second: &str, unit: Option<&str>) -> String {
    let range = if second.is_empty() {
        first.to_owned()
    } else {
        format!("{first}–{second}")
    };
    let separator = if currency
        .chars()
        .last()
        .is_some_and(|character| character.is_ascii_alphanumeric())
    {
        " "
    } else {
        ""
    };
    let unit = unit
        .map(|unit| {
            let normalized = unit.to_ascii_lowercase();
            let normalized = normalized.strip_prefix("per ").unwrap_or(&normalized);
            match normalized {
                "year" | "yearly" | "annual" | "annually" => " per year".to_owned(),
                "month" | "monthly" => " per month".to_owned(),
                "hour" | "hourly" => " per hour".to_owned(),
                value => format!(" per {value}"),
            }
        })
        .unwrap_or_default();
    format!("{currency}{separator}{range}{unit}")
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MetadataFieldDiagnostics<T> {
    /// The selected value.
    pub selected: Option<MetadataValue<T>>,
    /// The normalized values that were not selected.
    pub alternatives: Vec<MetadataValue<T>>,
}

/// Selection details for a metadata field that can have many values.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MetadataListFieldDiagnostics<T> {
    /// The selected values in source order.
    pub selected: Vec<MetadataValue<T>>,
    /// The normalized values that were not selected.
    pub alternatives: Vec<MetadataValue<T>>,
}

/// Optional provenance for all public metadata fields.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    evidence: CandidateEvidence,
}

/// How a candidate value relates to other evidence on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateEvidence {
    /// The page declares the value, for example in a `meta` element.
    Declared,
    /// The primary content shows the value to the reader. Agreement between
    /// a visible value and a declared value is independent corroboration,
    /// also when both values have the same public source.
    Visible,
    /// The page-title rules derived the value from a heading. It repeats
    /// that heading, so it never corroborates another candidate and no
    /// candidate corroborates it.
    DerivedFromHeading,
}

#[derive(Default)]
struct MetadataCandidates {
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

impl MetadataCandidates {
    fn add(
        &mut self,
        field: fn(&mut Self) -> &mut Vec<MetadataCandidate>,
        value: impl Into<String>,
        source: MetadataSource,
        confidence: u8,
    ) {
        self.push(
            field,
            value.into(),
            source,
            confidence,
            CandidateEvidence::Declared,
        );
    }

    /// Adds a value that the primary content shows to the reader.
    fn add_visible(
        &mut self,
        field: fn(&mut Self) -> &mut Vec<MetadataCandidate>,
        value: impl Into<String>,
        confidence: u8,
    ) {
        self.push(
            field,
            value.into(),
            MetadataSource::HtmlElement,
            confidence,
            CandidateEvidence::Visible,
        );
    }

    /// Adds a document title that the page-title rules derived from a
    /// heading.
    fn add_derived_title(&mut self, value: impl Into<String>, confidence: u8) {
        self.push(
            |set| &mut set.title,
            value.into(),
            MetadataSource::HtmlElement,
            confidence,
            CandidateEvidence::DerivedFromHeading,
        );
    }

    fn push(
        &mut self,
        field: fn(&mut Self) -> &mut Vec<MetadataCandidate>,
        value: String,
        source: MetadataSource,
        confidence: u8,
        evidence: CandidateEvidence,
    ) {
        let order = self.next_order;
        self.next_order += 1;
        field(self).push(MetadataCandidate {
            value,
            source,
            confidence,
            order,
            evidence,
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
    let mut candidates = MetadataCandidates::default();
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

    resolve_candidates(candidates, base_url, source_url, retain_diagnostics)
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
            return dom
                .attr(node, AttrName::Rel)
                .is_some_and(|rel| has_token(rel, "canonical"))
                && dom
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
    out: &mut MetadataCandidates,
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
    out: &mut MetadataCandidates,
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
        .map(|heading| visible_heading_text(dom, heading))
        .unwrap_or_default()
}

/// Returns authored heading text without embedded interface controls.
fn visible_heading_text(dom: &Dom, heading: NodeId) -> String {
    let mut raw = String::new();
    let mut stack: Vec<_> = dom
        .children_rev(heading)
        .map(|node| (node, false))
        .collect();
    while let Some((node, hidden)) = stack.pop() {
        if let Some(value) = dom.text_node(node) {
            if !hidden {
                raw.push_str(value);
            }
            continue;
        }
        let hidden = hidden
            || matches!(
                dom.tag(node),
                Some(Tag::Input | Tag::Select | Tag::Textarea)
            )
            || dom.tag(node) == Some(Tag::Button) && is_heading_utility_button(dom, node)
            || dom
                .attr(node, AttrName::AriaHidden)
                .is_some_and(|value| value.eq_ignore_ascii_case("true"));
        stack.extend(dom.children_rev(node).map(|child| (child, hidden)));
    }
    normalize_whitespace(raw.trim())
}

fn is_heading_utility_button(dom: &Dom, button: NodeId) -> bool {
    let mut text = String::new();
    dom.append_normalized_text_limited(button, &mut text, 64);
    let utility_label = |label: &str| {
        matches!(
            label.trim().to_ascii_lowercase().as_str(),
            "copy" | "copy item path" | "copy link" | "copy permalink" | "permalink"
        )
    };
    utility_label(&text)
        || ["aria-label", "title"]
            .into_iter()
            .filter_map(|name| dom.attr_by_local_name(button, name))
            .any(utility_label)
}

fn metadata_identity_title(dom: &Dom, document_title: &str) -> String {
    best_visible_heading(
        dom,
        dom.descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::H1)),
    )
    .filter(|&heading| visible_heading_confidence(dom, heading) > 78)
    .map(|heading| visible_heading_text(dom, heading))
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
        .or_else(|| item.get("title"))
        .and_then(Value::as_str);
    let title_score = title
        .map(|title| (text_similarity(title, document_title) * 100.0) as u16)
        .unwrap_or(0);
    let url_score = json_url(item)
        .and_then(|value| Url::parse(value).ok())
        .zip(source_url)
        .filter(|(candidate, source)| same_host_and_path(candidate, source))
        .map_or(0, |_| 150);
    title_score + url_score
}

fn same_host_and_path(candidate: &Url, source: &Url) -> bool {
    candidate.host() == source.host()
        && candidate.path().trim_end_matches('/') == source.path().trim_end_matches('/')
}

fn add_json_string(
    out: &mut MetadataCandidates,
    field: fn(&mut MetadataCandidates) -> &mut Vec<MetadataCandidate>,
    value: Option<&Value>,
    confidence: u8,
) {
    if let Some(value) = value.and_then(Value::as_str) {
        out.add(field, value, MetadataSource::JsonLd, confidence);
    }
}

fn add_json_names(
    out: &mut MetadataCandidates,
    field: fn(&mut MetadataCandidates) -> &mut Vec<MetadataCandidate>,
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
    out: &mut MetadataCandidates,
    field: fn(&mut MetadataCandidates) -> &mut Vec<MetadataCandidate>,
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

fn collect_json_keywords(value: Option<&Value>, out: &mut MetadataCandidates) {
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

fn collect_meta_candidates(dom: &Dom, out: &mut MetadataCandidates) {
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
            .or_else(|| dom.attr(id, AttrName::HttpEquiv))
        else {
            continue;
        };
        for raw_key in raw_name.split_ascii_whitespace() {
            collect_meta_value(out, &normalize_key(raw_key), content, property.is_some());
        }
    }
}

fn collect_meta_value(out: &mut MetadataCandidates, name: &str, content: &str, is_property: bool) {
    if is_property && !name.contains(':') {
        return;
    }
    let (source, confidence) = metadata_source(name);
    let add = |out: &mut MetadataCandidates,
               field: fn(&mut MetadataCandidates) -> &mut Vec<MetadataCandidate>,
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

/// The list form of one declared author value.
enum PersonList<'a> {
    /// One person or organization.
    Single,
    /// An explicit list of full person names.
    People(SmallVec<[&'a str; 4]>),
    /// Two parts joined by a comma, `and`, or `&`. This can also be one
    /// inverted name, such as `García Márquez, Gabriel José`, or one firm,
    /// such as `Davis Polk & Wardwell`.
    AmbiguousPair([&'a str; 2]),
}

/// Classifies a declared author value as one name or a list of names.
///
/// A general `meta` author has no list form, so pages join several people
/// with commas, semicolons, `and`, or `&`. A value is a list only when every
/// part looks like a full person name. This keeps `Smith, John`,
/// `Jane Doe, PhD`, role suffixes, and organization names such as
/// `Johnson & Johnson` intact. Every two-part value is ambiguous, because
/// it can also be one inverted name or one firm named after two people. A
/// semicolon or three or more parts makes the list explicit.
fn person_list(value: &str) -> PersonList<'_> {
    const MAX_PEOPLE: usize = 10;
    let value = value.trim();
    let parts: SmallVec<[&str; 4]> = value
        .split([',', ';'])
        .flat_map(|segment| segment.split(" and ").flat_map(|part| part.split(" & ")))
        .map(|part| {
            let part = part.trim();
            let part = ["and ", "& "]
                .iter()
                .find_map(|prefix| part.strip_prefix(prefix))
                .unwrap_or(part);
            strip_by_prefix(part.trim()).trim()
        })
        .filter(|part| !part.is_empty())
        .collect();
    let all_caps = !value.chars().any(char::is_lowercase);
    if parts.len() < 2
        || parts.len() > MAX_PEOPLE
        || !parts.iter().all(|part| is_full_person_name(part))
        || parts.iter().any(|part| is_credential_list(part, all_caps))
    {
        PersonList::Single
    } else if parts.len() == 2 && !value.contains(';') {
        PersonList::AmbiguousPair([parts[0], parts[1]])
    } else {
        PersonList::People(parts)
    }
}

/// Returns whether a list part has only credential or suffix words, such as
/// `MD FACC`, `PhD`, `M.D.`, `Jr`, or `III`. Such a part qualifies the name
/// before it, so it is not a person. A short all-caps word is a credential
/// only when the value also has lowercase letters, because some pages write
/// whole names in capitals.
fn is_credential_list(part: &str, all_caps: bool) -> bool {
    const CREDENTIALS: [&str; 24] = [
        "bsc", "cpa", "dds", "dnp", "dvm", "edd", "esq", "facc", "facs", "frcp", "ii", "iii", "iv",
        "jd", "jr", "mba", "md", "mph", "msc", "pharmd", "phd", "psyd", "rn", "sr",
    ];
    let mut words = part.split_whitespace().peekable();
    words.peek().is_some()
        && words.all(|word| {
            let bare = word.trim_end_matches(['.', ',']);
            CREDENTIALS
                .iter()
                .any(|credential| bare.eq_ignore_ascii_case(credential))
                || bare.contains('.')
                || (!all_caps
                    && (2..=5).contains(&bare.len())
                    && bare.chars().all(|character| character.is_ascii_uppercase()))
        })
}

/// Splits general `meta` author lists into one candidate for each person.
///
/// Only general and Open Graph author fields are split. Citation and Dublin
/// Core fields repeat their element for each author, so their commas belong
/// to one inverted name. JSON-LD uses arrays for several people. An ambiguous
/// two-part value is split only when another author candidate, such as a
/// visible byline, names one of its parts.
fn split_declared_author_lists(candidates: &mut MetadataCandidates) {
    let authors = std::mem::take(&mut candidates.authors);
    let declared_parts: Vec<SmallVec<[&str; 4]>> = authors
        .iter()
        .map(|candidate| {
            if !matches!(
                candidate.source,
                MetadataSource::HtmlMeta | MetadataSource::OpenGraph
            ) {
                return SmallVec::new();
            }
            match person_list(&candidate.value) {
                PersonList::Single => SmallVec::new(),
                PersonList::People(parts) => parts,
                PersonList::AmbiguousPair(parts) => {
                    let corroborated = authors.iter().any(|other| {
                        other.order != candidate.order
                            && parts
                                .iter()
                                .any(|part| metadata_values_equal(part, &other.value))
                    });
                    if corroborated {
                        parts.into_iter().collect()
                    } else {
                        SmallVec::new()
                    }
                }
            }
        })
        .collect();
    let mut split = Vec::with_capacity(authors.len());
    for (candidate, parts) in authors.iter().zip(&declared_parts) {
        // Another source can repeat the joined list, such as a byline link
        // with the same text. That value is the same evidence, so it splits
        // into the same people instead of remaining as a combined author.
        let parts = if parts.is_empty() {
            authors
                .iter()
                .zip(&declared_parts)
                .find(|(other, other_parts)| {
                    !other_parts.is_empty() && metadata_values_equal(&other.value, &candidate.value)
                })
                .map_or(parts, |(_, other_parts)| other_parts)
        } else {
            parts
        };
        if parts.is_empty() {
            split.push(candidate.clone());
            continue;
        }
        for part in parts {
            if let Some(value) = normalize_person(part) {
                split.push(MetadataCandidate {
                    value,
                    order: candidates.next_order,
                    ..candidate.clone()
                });
                candidates.next_order += 1;
            }
        }
    }
    candidates.authors = split;
}

/// Returns whether a list part has the form of a full person name: two to four
/// capitalized words with optional lowercase name particles, no digits, and
/// no organization or role word.
fn is_full_person_name(value: &str) -> bool {
    const PARTICLES: [&str; 14] = [
        "al", "bin", "da", "de", "del", "der", "di", "du", "la", "le", "ten", "van", "von", "y",
    ];
    const NON_PERSON_WORDS: [&str; 45] = [
        "agency",
        "associated",
        "associates",
        "association",
        "board",
        "bureau",
        "co",
        "college",
        "company",
        "corp",
        "correspondent",
        "council",
        "department",
        "desk",
        "developer",
        "director",
        "editor",
        "editors",
        "engineer",
        "foundation",
        "gmbh",
        "group",
        "inc",
        "institute",
        "llc",
        "llp",
        "ltd",
        "manager",
        "media",
        "news",
        "newsroom",
        "officer",
        "partners",
        "phd",
        "plc",
        "president",
        "press",
        "reporter",
        "school",
        "society",
        "staff",
        "team",
        "times",
        "university",
        "writer",
    ];
    let words: SmallVec<[&str; 4]> = value.split_whitespace().collect();
    if !(2..=4).contains(&words.len()) {
        return false;
    }
    let mut capitalized = 0;
    for word in &words {
        let bare = word.trim_end_matches('.');
        if bare.is_empty()
            || !bare.chars().all(|character| {
                character.is_alphabetic() || matches!(character, '\'' | '’' | '-' | '.')
            })
            || NON_PERSON_WORDS
                .iter()
                .any(|excluded| bare.eq_ignore_ascii_case(excluded))
        {
            return false;
        }
        if bare.chars().next().is_some_and(char::is_uppercase) {
            capitalized += 1;
        } else if !PARTICLES.contains(&bare) {
            return false;
        }
    }
    capitalized >= 2
        && words
            .first()
            .is_some_and(|word| word.chars().next().is_some_and(char::is_uppercase))
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

fn collect_link_candidates(dom: &Dom, out: &mut MetadataCandidates) {
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
        if has_token(rel, "canonical") {
            out.add(
                |set| &mut set.canonical_url,
                href,
                MetadataSource::LinkElement,
                100,
            );
        }
        if has_any_token(rel, &["icon", "shortcut", "apple-touch-icon"]) {
            out.add(
                |set| &mut set.favicon,
                href,
                MetadataSource::LinkElement,
                90,
            );
        }
    }
}

fn collect_element_candidates(dom: &Dom, document_title: &str, out: &mut MetadataCandidates) {
    let elements: Vec<_> = dom
        .descendants(dom.root())
        .filter(|&id| dom.is_element(id))
        .collect();
    let mut element_positions = vec![usize::MAX; dom.len()];
    let mut metadata_visible = vec![false; dom.len()];
    let mut heading_context = vec![78_u8; dom.len()];
    let mut inside_metadata_sidebar = vec![false; dom.len()];
    for (position, &node) in elements.iter().enumerate() {
        element_positions[node.index()] = position;
        let parent = dom.parent(node).filter(|&parent| dom.is_element(parent));
        metadata_visible[node.index()] = parent
            .is_none_or(|parent| metadata_visible[parent.index()])
            && !has_static_hidden_marker(dom, node)
            && !dom
                .attr(node, AttrName::AriaHidden)
                .is_some_and(|value| value.eq_ignore_ascii_case("true"));
        inside_metadata_sidebar[node.index()] =
            matches!(dom.tag(node), Some(Tag::Aside | Tag::Footer | Tag::Nav))
                || parent.is_some_and(|parent| inside_metadata_sidebar[parent.index()]);
        heading_context[node.index()] = if dom.tag(node) == Some(Tag::Article) {
            82
        } else if dom.tag(node) == Some(Tag::Main) {
            80
        } else if [
            dom.attr(node, AttrName::Class),
            dom.attr(node, AttrName::Id),
        ]
        .into_iter()
        .flatten()
        .flat_map(|value| value.split_ascii_whitespace())
        .any(is_primary_content_token)
        {
            88
        } else {
            parent.map_or(78, |parent| heading_context[parent.index()])
        };
    }
    let combined_byline_positions: Vec<_> = elements
        .iter()
        .enumerate()
        .filter_map(|(position, &node)| {
            (is_combined_date_author_container(dom, node) && metadata_visible[node.index()])
                .then_some(position)
        })
        .collect();
    let primary_heading = best_precomputed_heading(
        dom,
        elements
            .iter()
            .copied()
            .filter(|&id| dom.tag(id) == Some(Tag::H1)),
        &metadata_visible,
        &heading_context,
    )
    .or_else(|| {
        best_precomputed_heading(
            dom,
            elements.iter().enumerate().filter_map(|(position, &id)| {
                (dom.tag(id) == Some(Tag::H2)
                    && heading_matches_document_title(dom, id, document_title)
                    && is_safe_h2_metadata_heading(
                        position,
                        inside_metadata_sidebar[id.index()],
                        &combined_byline_positions,
                    ))
                .then_some(id)
            }),
            &metadata_visible,
            &heading_context,
        )
    });
    collect_title_candidates(dom, document_title, &elements, &heading_context, out);
    collect_language_candidates(dom, primary_heading, out);
    let mut text_buffer = String::new();
    for id in elements {
        if dom.tag(id) == Some(Tag::Html)
            && let Some(direction) = dom.attr(id, AttrName::Dir)
        {
            out.add(
                |set| &mut set.direction,
                direction,
                MetadataSource::HtmlElement,
                100,
            );
        }
        let itemprop = dom.attr(id, AttrName::ItemProp).unwrap_or("");
        let has_author_itemprop = has_token(itemprop, "author");
        let has_published_itemprop = has_token(itemprop, "datePublished");
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
                dom.attr(child, AttrName::ItemProp)
                    .is_some_and(|value| has_token(value, "name"))
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
                .attr(id, AttrName::Datetime)
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
                .attr(id, AttrName::Datetime)
                .unwrap_or_else(|| get_inner_text(dom, id, &mut text_buffer));
            out.add(
                |set| &mut set.published_time,
                value,
                MetadataSource::Inferred,
                48,
            );
        } else if is_dateline_candidate(dom, id)
            && metadata_visible[id.index()]
            && !inside_metadata_sidebar[id.index()]
            && primary_heading.is_some_and(|heading| {
                element_positions[id.index()].abs_diff(element_positions[heading.index()]) <= 12
                    && !dom.ancestors(heading).any(|ancestor| ancestor == id)
                    && is_near_heading(dom, id, heading)
            })
            && let Some(date) =
                primary_heading.and_then(|heading| visible_dateline_date(dom, id, heading))
        {
            out.add(
                |set| &mut set.published_time,
                date,
                MetadataSource::Inferred,
                48,
            );
        }
        if dom.tag(id) == Some(Tag::A) {
            let rel_author = dom
                .attr(id, AttrName::Rel)
                .is_some_and(|rel| has_token(rel, "author"));
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
        let combined_date_author = is_combined_date_author_container(dom, id);
        let author_element = is_author_container(dom, id);
        if author_element
            && !combined_date_author
            && primary_heading
                .is_some_and(|heading| is_byline_near_heading(dom, id, heading, &element_positions))
        {
            let author_node = preferred_author_node(dom, id);
            let value = byline_name(dom, id).unwrap_or_else(|| {
                get_inner_text(dom, author_node.unwrap_or(id), &mut text_buffer).to_owned()
            });
            if value.chars().count() <= 120 {
                out.add(|set| &mut set.authors, value, MetadataSource::Inferred, 62);
            }
            if is_byline_container(dom, id) {
                let visible_date = preferred_byline_date(dom, id, written_date);
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
        } else if combined_date_author
            && metadata_visible[id.index()]
            && primary_heading.is_some_and(|heading| {
                !inside_metadata_sidebar[id.index()]
                    && element_positions[id.index()].abs_diff(element_positions[heading.index()])
                        <= 12
            })
        {
            // Some publishers keep the date and author in one compact line.
            // Treat this as one byline only when its name states both roles.
            // This avoids turning an ordinary date wrapper into an author.
            if let Some(value) = combined_date_author_name(dom, id) {
                out.add(|set| &mut set.authors, value, MetadataSource::Inferred, 62);
            }
            if let Some((_, date)) = combined_written_date(dom, id) {
                out.add(
                    |set| &mut set.published_time,
                    date,
                    MetadataSource::Inferred,
                    48,
                );
            }
        }
    }
}

/// Where a visible level-one heading is on the page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeadingPlacement {
    /// Inside an `article`, `main`, or primary-content container.
    Content,
    /// In the page banner or page footer.
    PageChrome,
    /// Elsewhere.
    Other,
}

/// Adds the document title and the visible level-one headings.
///
/// A heading in the primary content is visible evidence. It corroborates an
/// equal document title. A document title that only repeats a page-chrome
/// heading, such as a site masthead, names the site or desk, not the page.
/// See `is_page_chrome_heading` for the evidence.
/// That title gets less confidence than a different primary-content heading.
fn collect_title_candidates(
    dom: &Dom,
    document_title: &str,
    elements: &[NodeId],
    heading_context: &[u8],
    out: &mut MetadataCandidates,
) {
    let headings: Vec<_> = elements
        .iter()
        .filter(|&&id| dom.tag(id) == Some(Tag::H1))
        .filter_map(|&id| {
            let confidence = visible_heading_confidence(dom, id);
            if confidence == 0 {
                return None;
            }
            let text = visible_heading_text(dom, id);
            let placement = if heading_context[id.index()] >= 80 {
                HeadingPlacement::Content
            } else if is_page_chrome_heading(dom, id) {
                HeadingPlacement::PageChrome
            } else {
                HeadingPlacement::Other
            };
            Some((text, confidence, placement))
        })
        .collect();
    if !document_title.is_empty() {
        let matches_heading = |expected: HeadingPlacement| {
            headings.iter().any(|(text, _, placement)| {
                *placement == expected && metadata_values_equal(text, document_title)
            })
        };
        let names_page_chrome = matches_heading(HeadingPlacement::PageChrome)
            && !matches_heading(HeadingPlacement::Content)
            && !matches_heading(HeadingPlacement::Other)
            && headings
                .iter()
                .any(|(_, _, placement)| *placement == HeadingPlacement::Content);
        let confidence = if names_page_chrome { 76 } else { 84 };
        if declared_title_names(dom, document_title) {
            out.add(
                |set| &mut set.title,
                document_title,
                MetadataSource::HtmlElement,
                confidence,
            );
        } else {
            out.add_derived_title(document_title, confidence);
        }
    }
    for (text, confidence, placement) in headings {
        if placement == HeadingPlacement::Content {
            out.add_visible(|set| &mut set.title, text, confidence);
        } else {
            out.add(
                |set| &mut set.title,
                text,
                MetadataSource::HtmlElement,
                confidence,
            );
        }
    }
}

/// Returns whether the declared `<title>` text names `title`. The text can
/// equal it or add a site or section segment after a title separator, such
/// as `Headline | Site` or `Section: Headline`. A title that the page-title
/// rules derived from a heading does not pass, because the declaration does
/// not state it.
fn declared_title_names(dom: &Dom, title: &str) -> bool {
    const SEPARATORS: [&str; 7] = [" | ", " - ", " — ", " – ", " :: ", " · ", ": "];
    let Some(element) = dom.first_descendant_by_tag(dom.root(), Tag::Title) else {
        return false;
    };
    let declared = normalize_whitespace(get_inner_text_owned(dom, element).trim());
    let title = title.trim();
    metadata_values_equal(&declared, title)
        || SEPARATORS.iter().any(|separator| {
            declared
                .split(separator)
                .any(|segment| metadata_values_equal(segment.trim(), title))
        })
}

/// Returns whether a visible heading is in the page banner or page footer.
///
/// HTML scopes a `header` or `footer` inside `article`, `aside`, `main`,
/// `nav`, or `section` to that content, so such a landmark is not page chrome.
/// The landmark must also be a child of `body` or of one top-level wrapper,
/// because a deeply nested component header is not the site masthead. A
/// document title that equals a declared site name is handled when the title
/// is resolved.
fn is_page_chrome_heading(dom: &Dom, heading: NodeId) -> bool {
    let Some(landmark) = dom
        .ancestors(heading)
        .find(|&node| matches!(dom.tag(node), Some(Tag::Header | Tag::Footer)))
    else {
        return false;
    };
    !dom.ancestors(landmark).any(|node| {
        matches!(
            dom.tag(node),
            Some(Tag::Article | Tag::Aside | Tag::Main | Tag::Nav | Tag::Section)
        )
    }) && dom
        .ancestors(landmark)
        .take(2)
        .any(|node| dom.tag(node) == Some(Tag::Body))
}

/// Adds the document language and a more specific content-root language.
///
/// The `lang` attribute that governs the primary heading on an `article`,
/// `main`, `body`, or primary-content container states the language of the
/// page content. When its primary language subtag conflicts with the
/// document language, it replaces the document language as the best value.
fn collect_language_candidates(
    dom: &Dom,
    primary_heading: Option<NodeId>,
    out: &mut MetadataCandidates,
) {
    let document_language = dom
        .first_descendant_by_tag(dom.root(), Tag::Html)
        .and_then(|html| dom.attr(html, AttrName::Lang))
        .filter(|value| !value.trim().is_empty());
    let content_language = primary_heading
        .and_then(|heading| content_root_language(dom, heading))
        .filter(|content| {
            document_language.is_none_or(|document| !same_primary_language(document, content))
        });
    if let Some(language) = content_language {
        out.add(
            |set| &mut set.language,
            language,
            MetadataSource::HtmlElement,
            100,
        );
    }
    if let Some(language) = document_language {
        out.add(
            |set| &mut set.language,
            language,
            MetadataSource::HtmlElement,
            if content_language.is_some() { 80 } else { 100 },
        );
    }
}

fn content_root_language(dom: &Dom, heading: NodeId) -> Option<&str> {
    let mut current = dom.parent(heading);
    while let Some(node) = current {
        if dom.tag(node) == Some(Tag::Html) {
            return None;
        }
        if let Some(language) = dom
            .attr(node, AttrName::Lang)
            .filter(|value| !value.trim().is_empty())
        {
            let content_root = matches!(dom.tag(node), Some(Tag::Article | Tag::Main | Tag::Body))
                || [
                    dom.attr(node, AttrName::Class),
                    dom.attr(node, AttrName::Id),
                ]
                .into_iter()
                .flatten()
                .flat_map(|value| value.split_ascii_whitespace())
                .any(is_primary_content_token);
            return content_root.then_some(language);
        }
        current = dom.parent(node);
    }
    None
}

fn same_primary_language(first: &str, second: &str) -> bool {
    fn primary(value: &str) -> &str {
        value.trim().split(['-', '_']).next().unwrap_or("")
    }
    primary(first).eq_ignore_ascii_case(primary(second))
}

/// Returns the publication date in a byline or dateline container.
///
/// The container can hold several dates. Each visible date has a role:
///
/// - A publication label, such as `Published` or `Posted on`, or a
///   publication class, ID, or `itemprop` makes it a publication date. The
///   date with the strongest [`PublicationEvidence`] wins. Equal evidence
///   keeps the first date in source order.
/// - A modification label or marker, such as `Updated at` or
///   `class="updated"`, excludes it.
/// - Other dates have no stated role. Without a publication date, the first
///   of them in source order is used. A `time` with a calendar `datetime`
///   gives that value, because it keeps the source format. A text date comes
///   from `parse`.
///
/// A `datetime` value must be a calendar date or date-time. Text inside a
/// `time` element with such a value is the same date, so it is not read again.
fn preferred_byline_date(
    dom: &Dom,
    container: NodeId,
    parse: fn(&str) -> Option<(usize, String)>,
) -> Option<String> {
    // The strongest publication evidence wins. Equal evidence keeps the
    // first date in source order. Unstated dates have the weakest evidence.
    let mut best: Option<(Option<PublicationEvidence>, String)> = None;
    let mut consider = |node: NodeId, prefix: &str, value: String| {
        let strength = match date_role(dom, node, container, prefix) {
            DateRole::Publication(evidence) => Some(evidence),
            DateRole::Unstated => None,
            DateRole::Modification | DateRole::Other => return,
        };
        if best.as_ref().is_none_or(|(current, _)| strength > *current) {
            best = Some((strength, value));
        }
    };
    for node in dom.descendants(container) {
        if dom.tag(node) == Some(Tag::Time) {
            if let Some(value) = calendar_datetime(dom, node)
                && !is_hidden_within(dom, node, container)
            {
                consider(node, "", value.to_owned());
            }
            continue;
        }
        let Some(text) = dom.text_node(node) else {
            continue;
        };
        if dom
            .ancestors(node)
            .take_while(|&ancestor| ancestor != container)
            .any(|ancestor| calendar_datetime(dom, ancestor).is_some())
            || is_hidden_within(dom, node, container)
        {
            continue;
        }
        // One text node can hold several dates, such as
        // `Updated August 14, 2026 · Published August 13, 2026`. The text
        // directly before each date is its label.
        let mut offset = 0;
        while let Some((start, date)) = text.get(offset..).and_then(parse) {
            let start = offset + start;
            consider(node, label_window(&text[..start]), date);
            offset = start + text[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    best.map(|(_, value)| value)
}

/// Returns the end of the text before a date, bounded to the label length
/// that `label_role_at_end` can use.
fn label_window(prefix: &str) -> &str {
    const LABEL_BYTE_LIMIT: usize = 64;
    let mut start = prefix.len().saturating_sub(LABEL_BYTE_LIMIT);
    while !prefix.is_char_boundary(start) {
        start += 1;
    }
    &prefix[start..]
}

/// The role that labels and markers give a visible date.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DateRole {
    Publication(PublicationEvidence),
    Modification,
    /// A different date, such as an event start date.
    Other,
    Unstated,
}

/// The kind of evidence that marks a publication date, from weakest to
/// strongest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PublicationEvidence {
    /// A generic post-date class, such as `entry-date`.
    GenericClass,
    /// A specific class or ID part, such as `published`.
    SpecificClass,
    /// A visible label, such as `Published` or `First published`.
    Label,
    /// The schema.org `datePublished` property.
    Property,
}

fn calendar_datetime(dom: &Dom, node: NodeId) -> Option<&str> {
    (dom.tag(node) == Some(Tag::Time))
        .then(|| dom.attr(node, AttrName::Datetime))
        .flatten()
        .map(str::trim)
        .filter(|value| is_calendar_datetime(value))
}

/// Returns whether a `datetime` value names a calendar date or date-time,
/// such as `2026-08-13` or `2026-08-13T10:00Z`. A duration such as `PT5M` or
/// a time of day such as `10:00` is not a publication date.
fn is_calendar_datetime(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 4
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes.get(4).is_none_or(|&byte| byte == b'-')
}

/// Returns whether a node or an ancestor up to and including the container
/// is hidden.
fn is_hidden_within(dom: &Dom, node: NodeId, container: NodeId) -> bool {
    let mut current = Some(node);
    while let Some(element) = current {
        if is_hidden_node(dom, element) {
            return true;
        }
        current = (element != container)
            .then(|| dom.parent(element))
            .flatten();
    }
    false
}

/// Returns the role of a date node inside a container. `prefix` is the text
/// of a text node before its date.
///
/// The evidence is checked in this order. The nearest marker on the node or
/// an ancestor inside the container decides each marker step.
///
/// 1. A schema.org `itemprop`, such as `datePublished` or `dateModified`.
/// 2. A visible label at the start of a date element, or directly before the
///    date: the prefix when it has words, or else the end of the nearest
///    preceding non-blank sibling at the node or an ancestor inside the
///    container.
/// 3. A specific class or ID part, such as `published` or `updated`.
/// 4. A generic post-date class or ID, such as `entry-date`.
fn date_role(dom: &Dom, node: NodeId, container: NodeId, prefix: &str) -> DateRole {
    const LABEL_CHAR_LIMIT: usize = 48;
    let nearest_marker = |tier: MarkerTier| {
        let mut current = Some(node);
        while let Some(element) = current {
            if let Some(role) = marker_role(dom, element, tier) {
                return Some(role);
            }
            current = (element != container)
                .then(|| dom.parent(element))
                .flatten();
        }
        None
    };
    if let Some(role) = nearest_marker(MarkerTier::Property) {
        return role;
    }
    if dom.is_element(node) {
        let mut own_text = String::new();
        dom.append_normalized_text_limited(node, &mut own_text, 4 * LABEL_CHAR_LIMIT);
        if let Some(role) = label_role_at_start(&own_text) {
            return role;
        }
    }
    if let Some(role) = label_role_at_end(&preceding_label(dom, node, container, prefix)) {
        return role;
    }
    nearest_marker(MarkerTier::SpecificClass)
        .or_else(|| nearest_marker(MarkerTier::GenericClass))
        .unwrap_or(DateRole::Unstated)
}

/// Assembles the visible text directly before a date, starting with
/// `prefix`. The text grows backward through preceding siblings, and then
/// through the siblings of ancestors inside the container. It never crosses
/// an earlier `time` element. It stops when it
/// has a word that is not a connector, such as `Updated` in
/// `<span>Updated</span> on <time>`, or a separator such as `·`. Hidden
/// subtrees do not contribute.
fn preceding_label(dom: &Dom, node: NodeId, container: NodeId, prefix: &str) -> String {
    const LABEL_CHAR_LIMIT: usize = 48;
    let mut label = prefix.to_owned();
    let mut current = node;
    while !is_complete_label(&label) && current != container {
        let mut sibling = dom.prev_sibling(current);
        while let Some(previous) = sibling {
            if !is_hidden_node(dom, previous) {
                let (text, reached_date) = trailing_text(dom, previous, LABEL_CHAR_LIMIT);
                label.insert(0, ' ');
                label.insert_str(0, &text);
                if reached_date || is_complete_label(&label) {
                    return label;
                }
            }
            sibling = dom.prev_sibling(previous);
        }
        let Some(parent) = dom.parent(current) else {
            break;
        };
        current = parent;
    }
    label
}

/// The longest label segment, in characters, that can describe a date.
const LABEL_SEGMENT_CHARS: usize = 48;

/// Returns whether assembled label text reaches back far enough: it has a
/// separator, an earlier date, or a full label segment.
fn is_complete_label(text: &str) -> bool {
    text.contains(is_label_separator)
        || text.chars().count() >= LABEL_SEGMENT_CHARS
        || combined_written_date_value(text).is_some()
}

fn is_label_separator(character: char) -> bool {
    matches!(character, '·' | '|' | '•' | ';' | '\n')
}

fn is_hidden_node(dom: &Dom, node: NodeId) -> bool {
    has_static_hidden_marker(dom, node)
        || dom
            .attr(node, AttrName::AriaHidden)
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

/// Returns the last `limit` characters of the visible text in a subtree. The
/// scan starts at the end, so a long subtree costs only the text it needs.
///
/// A `time` element is an earlier date, so the scan stops before it, whatever
/// text it shows. The flag is `true` when the scan reached such a date.
fn trailing_text(dom: &Dom, root: NodeId, limit: usize) -> (String, bool) {
    let mut pieces = Vec::new();
    let mut chars = 0;
    let mut reached_date = false;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if let Some(text) = dom.text_node(node) {
            chars += text.chars().count();
            pieces.push(text);
            if chars >= limit {
                break;
            }
            continue;
        }
        if dom.tag(node) == Some(Tag::Time) {
            reached_date = true;
            break;
        }
        if !is_hidden_node(dom, node) {
            stack.extend(dom.children(node));
        }
    }
    let joined: String = pieces.into_iter().rev().collect();
    let normalized = normalize_whitespace(&joined);
    let skip = normalized.chars().count().saturating_sub(limit);
    (normalized.chars().skip(skip).collect(), reached_date)
}

const MODIFICATION_LABELS: [&str; 5] = ["updated", "update", "modified", "revised", "edited"];
const PUBLICATION_LABELS: [&str; 2] = ["published", "posted"];

fn label_role(label: &str) -> Option<DateRole> {
    if MODIFICATION_LABELS
        .iter()
        .any(|expected| label.eq_ignore_ascii_case(expected))
    {
        Some(DateRole::Modification)
    } else if PUBLICATION_LABELS
        .iter()
        .any(|expected| label.eq_ignore_ascii_case(expected))
    {
        Some(DateRole::Publication(PublicationEvidence::Label))
    } else {
        None
    }
}

/// Returns the role of a label at the start of a date element's text, such
/// as `Updated 10:00` or `Last updated`. `Last` and `First` are qualifiers.
fn label_role_at_start(text: &str) -> Option<DateRole> {
    let mut words = text
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty());
    let first = words.next()?;
    if ["last", "first"]
        .iter()
        .any(|qualifier| first.eq_ignore_ascii_case(qualifier))
    {
        label_role(words.next()?)
    } else {
        label_role(first)
    }
}

/// Returns the role of the label segment directly before a date.
///
/// The segment is the text after the nearest separator (`·`, `|`, `•`, `;`,
/// or a line break) or after an earlier date, bounded to
/// `LABEL_SEGMENT_CHARS` characters. A whole modification word, such as
/// `updated` in `Last updated as of`, makes the date a modification date. Else
/// a whole publication word, such as `published` in `First published on:`,
/// makes it a publication date. A modification word followed by `by`, such as
/// `edited by`, credits a person, so it is not a date label.
fn label_role_at_end(text: &str) -> Option<DateRole> {
    let segment = label_segment(text);
    let words: SmallVec<[&str; 8]> = segment
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let has_word = |labels: &[&str]| {
        words.iter().enumerate().any(|(index, word)| {
            labels.iter().any(|label| word.eq_ignore_ascii_case(label))
                && !words
                    .get(index + 1)
                    .is_some_and(|next| next.eq_ignore_ascii_case("by"))
        })
    };
    if has_word(&MODIFICATION_LABELS) {
        Some(DateRole::Modification)
    } else if has_word(&PUBLICATION_LABELS) {
        Some(DateRole::Publication(PublicationEvidence::Label))
    } else {
        None
    }
}

/// Returns the byte length of a written date at the start of `text`. A
/// written date has a month, a day, and a year word.
fn written_date_len(text: &str) -> usize {
    let mut words = 0;
    let mut in_word = false;
    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if in_word {
                words += 1;
                if words == 3 {
                    return index;
                }
            }
            in_word = false;
        } else {
            in_word = true;
        }
    }
    text.len()
}

/// Returns the label segment at the end of `text`: the text after the last
/// separator or earlier written date, bounded to `LABEL_SEGMENT_CHARS`
/// characters.
fn label_segment(text: &str) -> &str {
    let mut segment = text.rfind(is_label_separator).map_or(text, |index| {
        &text[index + text[index..].chars().next().map_or(1, char::len_utf8)..]
    });
    while let Some((start, _)) = combined_written_date_value(segment) {
        segment = &segment[start + written_date_len(&segment[start..])..];
    }
    let skip = segment.chars().count().saturating_sub(LABEL_SEGMENT_CHARS);
    segment
        .char_indices()
        .nth(skip)
        .map_or("", |(index, _)| &segment[index..])
}

/// Returns the publication date of a short visible dateline near the primary
/// heading, such as `<p class="date">Published August 13, 2026</p>`.
///
/// The line must state its role with a publication label, such as
/// `Published`, or a publication marker, such as `class="published"`,
/// `class="entry-date"`, or `itemprop="datePublished"`. A generic `date`
/// token is not enough. The date must follow the label directly, and little text can
/// follow the date. A modification label, class, ID, or `itemprop` on the line
/// or on a wrapper rejects it. The caller checks visibility and heading
/// proximity.
fn visible_dateline_date(dom: &Dom, node: NodeId, heading: NodeId) -> Option<String> {
    const MAX_DATELINE_CHARS: usize = 80;
    const TEXT_BYTE_LIMIT: usize = 256;
    const MAX_TRAILING_CHARS: usize = 24;
    let mut text = String::new();
    dom.append_normalized_text_limited(node, &mut text, TEXT_BYTE_LIMIT);
    let text = text.trim();
    if text.is_empty() || text.len() >= TEXT_BYTE_LIMIT || text.chars().count() > MAX_DATELINE_CHARS
    {
        return None;
    }
    let marked_publication = match dateline_marker_role(dom, node, heading) {
        DateRole::Publication(_) => true,
        DateRole::Unstated => false,
        DateRole::Modification | DateRole::Other => return None,
    };
    if label_role_at_start(text) == Some(DateRole::Modification) {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    let labelled = ["first published", "published", "posted"]
        .iter()
        .find_map(|label| text.get(label.len()..).filter(|_| lower.starts_with(label)))
        .map(|remainder| {
            let remainder = remainder.trim_start_matches([':', ' ']);
            remainder
                .strip_prefix("on ")
                .unwrap_or(remainder)
                .trim_start()
        });
    let remainder = match labelled {
        Some(remainder) => remainder,
        None if marked_publication => text,
        None => return None,
    };
    let (start, _) = combined_written_date_value(remainder)?;
    // A written date has a month, a day, and a year word.
    let trailing_chars: usize = remainder[start..]
        .split_ascii_whitespace()
        .skip(3)
        .map(|word| word.chars().count() + 1)
        .sum();
    if start > 0 || trailing_chars > MAX_TRAILING_CHARS {
        return None;
    }
    preferred_byline_date(dom, node, combined_written_date_value)
}

/// Returns the date role that markers give a dateline.
///
/// The search covers the dateline and its wrappers up to the first ancestor
/// that also holds the primary heading, such as the article header, so
/// page-level classes do not count. An `itemprop` other than `datePublished`
/// states a different property, such as an event `startDate`, so it gives
/// [`DateRole::Other`]. Otherwise the nearest marker decides.
fn dateline_marker_role(dom: &Dom, node: NodeId, heading: NodeId) -> DateRole {
    let elements = || {
        std::iter::once(node).chain(
            dom.ancestors(node)
                .take_while(|&ancestor| !dom.ancestors(heading).any(|shared| shared == ancestor)),
        )
    };
    if elements().any(|element| {
        dom.attr(element, AttrName::ItemProp)
            .is_some_and(|itemprop| {
                !itemprop.trim().is_empty() && !has_token(itemprop, "datePublished")
            })
    }) {
        return DateRole::Other;
    }
    [
        MarkerTier::Property,
        MarkerTier::SpecificClass,
        MarkerTier::GenericClass,
    ]
    .into_iter()
    .find_map(|tier| elements().find_map(|element| marker_role(dom, element, tier)))
    .unwrap_or(DateRole::Unstated)
}

/// The kind of marker that `marker_role` reads. Stronger kinds come first.
#[derive(Clone, Copy)]
enum MarkerTier {
    /// A schema.org `itemprop`.
    Property,
    /// A class or ID part that names the date role, such as `published`.
    SpecificClass,
    /// A class or ID that names a post date without a role, such as
    /// `entry-date`.
    GenericClass,
}

/// Returns the date role that one kind of marker on an element states.
///
/// - [`MarkerTier::Property`]: `datePublished` and `dateModified` give their
///   roles. Another date or time property, such as `startDate`, gives
///   [`DateRole::Other`].
/// - [`MarkerTier::SpecificClass`]: every class and ID part counts, so
///   `class="date updated"` marks a modification date. Publishing systems
///   often mark the original date with both roles, such as WordPress
///   `class="entry-date published updated"`, so a publication part wins on
///   the same element. A bare `update` part is not a marker, because script
///   hooks such as `update_posted_time` name an action, not the date role.
/// - [`MarkerTier::GenericClass`]: `entry-date` and `post-date` usually name
///   the publication date of a post. A visible label or a specific marker
///   overrides them.
fn marker_role(dom: &Dom, element: NodeId, tier: MarkerTier) -> Option<DateRole> {
    const PUBLICATION_PARTS: [&str; 5] = [
        "datepublished",
        "pubdate",
        "publicationdate",
        "publishdate",
        "published",
    ];
    const MODIFICATION_PARTS: [&str; 9] = [
        "datemodified",
        "dateupdated",
        "edited",
        "lastmod",
        "lastmodified",
        "lastupdated",
        "modified",
        "revised",
        "updated",
    ];
    const GENERIC_TOKENS: [&str; 5] = [
        "entry-date",
        "entry_date",
        "post-date",
        "post_date",
        "postdate",
    ];
    let class_and_id = || {
        [
            dom.attr(element, AttrName::Class),
            dom.attr(element, AttrName::Id),
        ]
        .into_iter()
        .flatten()
    };
    match tier {
        MarkerTier::Property => {
            let itemprop = dom.attr(element, AttrName::ItemProp)?;
            if has_token(itemprop, "datePublished") {
                Some(DateRole::Publication(PublicationEvidence::Property))
            } else if has_token(itemprop, "dateModified") {
                Some(DateRole::Modification)
            } else if itemprop.split_ascii_whitespace().any(|property| {
                contains_ascii_case_insensitive(property, "date")
                    || contains_ascii_case_insensitive(property, "time")
                    || ["validFrom", "validThrough", "expires"]
                        .iter()
                        .any(|name| property.eq_ignore_ascii_case(name))
            }) {
                Some(DateRole::Other)
            } else {
                None
            }
        }
        MarkerTier::SpecificClass => {
            let has_part = |parts: &[&str]| {
                class_and_id()
                    .flat_map(|value| {
                        value.split(|character: char| !character.is_ascii_alphanumeric())
                    })
                    .any(|part| parts.iter().any(|marker| part.eq_ignore_ascii_case(marker)))
            };
            if has_part(&PUBLICATION_PARTS) {
                Some(DateRole::Publication(PublicationEvidence::SpecificClass))
            } else if has_part(&MODIFICATION_PARTS) {
                Some(DateRole::Modification)
            } else {
                None
            }
        }
        MarkerTier::GenericClass => class_and_id()
            .flat_map(str::split_ascii_whitespace)
            .any(|token| {
                GENERIC_TOKENS
                    .iter()
                    .any(|name| token.eq_ignore_ascii_case(name))
            })
            .then_some(DateRole::Publication(PublicationEvidence::GenericClass)),
    }
}

/// Cheap tag gate for a short dateline element. Headings, times, and author
/// containers have their own rules.
fn is_dateline_candidate(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(Tag::P | Tag::Div | Tag::Span | Tag::Small | Tag::Address)
    ) && !is_author_container(dom, node)
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
    // Every accepted token names an author or byline role. Most class and ID
    // tokens do not, so reject them before allocating lowercase parts.
    if !contains_ascii_case_insensitive(token, "author")
        && !contains_ascii_case_insensitive(token, "byline")
    {
        return false;
    }
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

fn is_combined_date_author_container(dom: &Dom, node: NodeId) -> bool {
    [
        dom.attr(node, AttrName::Class),
        dom.attr(node, AttrName::Id),
    ]
    .into_iter()
    .flatten()
    .flat_map(|value| value.split_ascii_whitespace())
    .any(|token| {
        if !contains_ascii_case_insensitive(token, "author") {
            return false;
        }
        let mut has_author = false;
        let mut has_date = false;
        for part in token
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|part| !part.is_empty())
        {
            has_author |= part.eq_ignore_ascii_case("author");
            has_date |= ["date", "publish", "published"]
                .iter()
                .any(|name| part.eq_ignore_ascii_case(name));
        }
        has_author && has_date
    })
}

fn combined_date_author_name(dom: &Dom, container: NodeId) -> Option<String> {
    if let Some(node) = preferred_author_node(dom, container)
        && let Some(value) =
            author_text_segment(dom, node).and_then(|value| normalize_person(&value))
    {
        return Some(value);
    }
    let value = get_inner_text_owned(dom, container);
    let (date_start, _) = combined_written_date_value(&value)?;
    let lower = value.to_ascii_lowercase();
    let remainder = if let Some(offset) = lower[date_start..].find(" by ") {
        value[date_start + offset + " by ".len()..].trim()
    } else {
        best_unmarked_author_segment(&value[date_start..])?
    };
    let lower = remainder.to_ascii_lowercase();
    let end = [" on behalf of ", " for the "]
        .into_iter()
        .filter_map(|marker| lower.find(marker))
        .min()
        .unwrap_or(remainder.len());
    normalize_person(&remainder[..end])
}

fn best_unmarked_author_segment(value: &str) -> Option<&str> {
    let mut best = None;
    let mut best_confidence = 0;
    for segment in value
        .split(['|', '·', '—', '–'])
        .skip(1)
        .map(str::trim)
        .filter(|segment| !is_non_author_metadata_segment(segment))
    {
        let lower = segment.to_ascii_lowercase();
        let end = [" on behalf of ", " for the "]
            .into_iter()
            .filter_map(|marker| lower.find(marker))
            .min()
            .unwrap_or(segment.len());
        let candidate = segment[..end]
            .trim()
            .trim_start_matches(|character: char| character.is_ascii_punctuation())
            .trim();
        let words = split_word_tokens(candidate).take(5).count();
        let confidence = match words {
            2..=4 => 2,
            1 => 1,
            _ => 0,
        };
        if confidence > best_confidence {
            best = Some(segment);
            best_confidence = confidence;
        }
    }
    best
}

fn combined_written_date(dom: &Dom, container: NodeId) -> Option<(usize, String)> {
    combined_written_date_value(&get_inner_text_owned(dom, container))
}

fn combined_written_date_value(value: &str) -> Option<(usize, String)> {
    if let Some(date) = written_date(value) {
        return Some(date);
    }
    const MONTHS: [(&str, &str); 11] = [
        ("Jan", "January"),
        ("Feb", "February"),
        ("Mar", "March"),
        ("Apr", "April"),
        ("Jun", "June"),
        ("Jul", "July"),
        ("Aug", "August"),
        ("Sep", "September"),
        ("Oct", "October"),
        ("Nov", "November"),
        ("Dec", "December"),
    ];
    let mut earliest = None;
    for (abbreviation, month) in MONTHS {
        for (start, _) in value.match_indices(abbreviation) {
            let normalized = normalize_whitespace(&value[start..]);
            let mut parts = normalized.split_ascii_whitespace();
            if !parts
                .next()
                .is_some_and(|part| part.trim_end_matches('.') == abbreviation)
            {
                continue;
            }
            let Some(day) = parts.next().map(|part| part.trim_end_matches(',')) else {
                continue;
            };
            let Some(year) = parts
                .next()
                .map(|part| part.trim_end_matches(|character: char| !character.is_ascii_digit()))
            else {
                continue;
            };
            if day.parse::<u8>().is_ok_and(|day| (1..=31).contains(&day))
                && year.len() == 4
                && year.bytes().all(|byte| byte.is_ascii_digit())
                && earliest
                    .as_ref()
                    .is_none_or(|(current, _): &(usize, String)| start < *current)
            {
                earliest = Some((start, format!("{month} {day}, {year}")));
            }
        }
    }
    earliest
}

fn is_non_author_metadata_segment(value: &str) -> bool {
    if combined_written_date_value(value).is_some() {
        return true;
    }
    let lower = value.trim().to_ascii_lowercase();
    lower.is_empty()
        || lower.split_ascii_whitespace().any(|word| {
            matches!(
                word.trim_matches(|character: char| !character.is_ascii_alphanumeric()),
                "category"
                    | "comment"
                    | "comments"
                    | "modified"
                    | "read"
                    | "replies"
                    | "reply"
                    | "section"
                    | "topic"
                    | "updated"
                    | "views"
            )
        })
        || clock_time_boundary(&lower).is_some()
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
    .any(|value| has_token(value, expected))
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

fn best_precomputed_heading(
    dom: &Dom,
    headings: impl Iterator<Item = NodeId>,
    visible: &[bool],
    context: &[u8],
) -> Option<NodeId> {
    headings
        .filter_map(|heading| {
            if !visible[heading.index()] {
                return None;
            }
            let text = get_inner_text_owned(dom, heading);
            normalize_text(&text)?;
            let title_bonus = u8::from(has_class_or_id_token(dom, heading, "p-name")) * 4
                + u8::from(split_word_tokens(&text).take(4).count() == 4) * 2;
            Some((heading, context[heading.index()] + title_bonus))
        })
        .fold(None, |best, candidate| match best {
            Some((_, confidence)) if confidence >= candidate.1 => best,
            _ => Some(candidate),
        })
        .map(|(heading, _)| heading)
}

fn is_primary_content_token(token: &str) -> bool {
    has_any_token(
        token,
        &[
            "article-content",
            "article-body",
            "entry-content",
            "e-content",
            "h-entry",
            "main-content",
            "post-content",
            "post-body",
        ],
    )
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
                    && (dom
                        .attr(node, AttrName::Rel)
                        .is_some_and(|rel| has_token(rel, "author"))
                        || dom
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
    if is_combined_date_author_container(dom, container)
        && let Some(value) = combined_date_author_name(dom, container)
    {
        return Some(value);
    }
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

fn heading_matches_document_title(dom: &Dom, heading: NodeId, document_title: &str) -> bool {
    let heading = get_inner_text_owned(dom, heading);
    let heading = heading.trim();
    let title = document_title.trim();
    if heading.is_empty() || title.is_empty() {
        return false;
    }
    title.eq_ignore_ascii_case(heading)
        || title.get(..heading.len()).is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case(heading)
                && [" | ", " - ", " — ", " – ", " :: ", " · "]
                    .iter()
                    .any(|separator| title[heading.len()..].starts_with(separator))
        })
}

fn is_safe_h2_metadata_heading(
    heading_position: usize,
    inside_metadata_sidebar: bool,
    combined_byline_positions: &[usize],
) -> bool {
    if inside_metadata_sidebar {
        return false;
    }
    combined_byline_positions
        .get(
            combined_byline_positions
                .partition_point(|&position| position < heading_position.saturating_sub(12)),
        )
        .is_some_and(|&position| heading_position.abs_diff(position) <= 12)
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
        has_any_token(
            token,
            &[
                "author-name",
                "byline-name",
                "byline__author",
                "byline__author-name",
                "p-author",
                "p-name",
            ],
        )
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
        has_any_token(
            token,
            &[
                "author-bio",
                "author-role",
                "author-title",
                "author__bio",
                "author__role",
                "author__title",
                "byline-role",
                "byline-title",
                "byline__role",
                "byline__title",
            ],
        )
    })
}

/// Returns the one section link in a byline.
///
/// Category evidence comes first: a section or category path, `rel="category"`,
/// or a link after a rule. A `rel="tag"` link names a topic, so it is the
/// section only when the byline has no category evidence. More than one link
/// of the selected kind is ambiguous.
fn preferred_byline_section_node(
    dom: &Dom,
    container: NodeId,
    author_node: Option<NodeId>,
) -> Option<NodeId> {
    let links = || {
        dom.descendants(container).filter(|&node| {
            dom.tag(node) == Some(Tag::A)
                && author_node.is_none_or(|author| node != author)
                && !dom
                    .attr(node, AttrName::Rel)
                    .is_some_and(|rel| has_token(rel, "author"))
                && !dom
                    .attr(node, AttrName::Href)
                    .is_some_and(|href| href.to_ascii_lowercase().contains("/author/"))
        })
    };
    let only = |evidence: SectionEvidence| {
        let mut matches = links().filter(|&node| section_evidence(dom, node) == evidence);
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    };
    if links().any(|node| section_evidence(dom, node) == SectionEvidence::Category) {
        only(SectionEvidence::Category)
    } else {
        only(SectionEvidence::Tag)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionEvidence {
    None,
    /// A section or category link.
    Category,
    /// A topic tag link.
    Tag,
}

fn section_evidence(dom: &Dom, link: NodeId) -> SectionEvidence {
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
    let rel = dom.attr(link, AttrName::Rel).unwrap_or("");
    if section_path
        || has_token(rel, "category")
        || previous_element_sibling(dom, link) == Some(Tag::Hr)
    {
        SectionEvidence::Category
    } else if has_token(rel, "tag") {
        SectionEvidence::Tag
    } else {
        SectionEvidence::None
    }
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
    dom.attr(node, AttrName::ItemProp)
        .is_some_and(|value| has_token(value, expected))
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

fn is_byline_near_heading(
    dom: &Dom,
    node: NodeId,
    heading: NodeId,
    element_positions: &[usize],
) -> bool {
    is_near_heading(dom, node, heading)
        || (nearest_ancestor_with_tag(dom, node, Tag::Aside).is_none()
            && element_positions[node.index()] != usize::MAX
            && element_positions[heading.index()] != usize::MAX
            && element_positions[node.index()].abs_diff(element_positions[heading.index()]) <= 12)
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
    mut candidates: MetadataCandidates,
    base_url: Option<&Url>,
    source_url: Option<&Url>,
    retain_diagnostics: bool,
) -> (Metadata, Option<MetadataDiagnostics>) {
    normalize_all(&mut candidates.title, normalize_title);
    normalize_all(&mut candidates.description, normalize_description);
    normalize_all(&mut candidates.authors, normalize_person);
    split_declared_author_lists(&mut candidates);
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
    let source_host = source_url.and_then(Url::host_str);
    candidates
        .tags
        .retain(|tag| !names_publisher(&tag.value, site_name.as_deref(), source_host));
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

/// Returns whether a tag only names the publication. Keyword lists often
/// repeat the site name, such as `Example News` on `example.com`. Such a value
/// does not describe the page topic. A tag names the publication when its
/// letters and digits equal the resolved site name. A tag that adds one
/// generic publication word, such as `News`, also matches the site name or
/// the first label of the source host. A bare host label such as `Python` on
/// `python.org` stays because it can name the topic. Site-name candidates that
/// lost resolution are not publication names.
fn names_publisher(tag: &str, site_name: Option<&str>, source_host: Option<&str>) -> bool {
    const PUBLICATION_WORDS: [&str; 6] = ["blog", "daily", "magazine", "media", "news", "online"];
    fn compact(value: &str) -> String {
        value
            .chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }
    let tag_name = compact(tag);
    if tag_name.is_empty() {
        return false;
    }
    let site_name = site_name.map(compact).filter(|name| !name.is_empty());
    let tag_base = tag
        .rsplit_once(char::is_whitespace)
        .filter(|(_, last)| {
            PUBLICATION_WORDS
                .iter()
                .any(|word| last.eq_ignore_ascii_case(word))
        })
        .map(|(base, _)| compact(base))
        .filter(|base| !base.is_empty());
    let host_label = source_host
        .map(|host| host.strip_prefix("www.").unwrap_or(host))
        .and_then(|host| host.split_once('.'))
        .map(|(label, _)| compact(label));
    site_name.as_ref() == Some(&tag_name)
        || tag_base.as_ref().is_some_and(|base| {
            site_name.as_ref() == Some(base) || host_label.as_ref() == Some(base)
        })
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
    // A value that the reader sees and a value that the page declares are
    // independent evidence. Their agreement outweighs one conflicting
    // declaration with a slightly higher confidence.
    const VISIBLE_CORROBORATION_BONUS: u16 = 7;
    if candidate.evidence == CandidateEvidence::DerivedFromHeading {
        return u16::from(candidate.confidence);
    }
    let mut agreements = 0_u16;
    let mut visible_corroboration = false;
    for other in candidates {
        if other.evidence == CandidateEvidence::DerivedFromHeading
            || (other.source == candidate.source && other.evidence == candidate.evidence)
            || !metadata_values_equal(&other.value, &candidate.value)
        {
            continue;
        }
        agreements += 1;
        visible_corroboration |= other.evidence != candidate.evidence;
    }
    u16::from(candidate.confidence)
        + agreements.min(3) * 5
        + u16::from(visible_corroboration) * VISIBLE_CORROBORATION_BONUS
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
    fn heading_controls_do_not_pollute_the_title() {
        let result = metadata(
            r#"<title>Buffer in crate - Docs</title><main><section id="main-content"><h1>Struct <span>Buffer</span><button>Copy item path</button><span aria-hidden="true">Hidden status</span></h1><p>Reference text.</p></section></main>"#,
            Some("https://docs.example.test/buffer"),
            false,
        );

        assert_eq!(result.title.as_deref(), Some("Struct Buffer"));
    }

    #[test]
    fn heading_text_preserves_inline_punctuation_boundaries() {
        let dom = Dom::parse_document(
            r#"<main><h1><code>Vec</code>&lt;T&gt;: <span>methods</span><button>Copy</button></h1></main>"#,
        )
        .unwrap();
        let heading = dom.first_descendant_by_tag(dom.root(), Tag::H1).unwrap();

        assert_eq!(visible_heading_text(&dom, heading), "Vec<T>: methods");
    }

    #[test]
    fn heading_text_keeps_button_wrapped_authored_text() {
        let dom = Dom::parse_document(
            r#"<main><h1><button aria-expanded="true">Installation guide</button></h1></main>"#,
        )
        .unwrap();
        let heading = dom.first_descendant_by_tag(dom.root(), Tag::H1).unwrap();

        assert_eq!(visible_heading_text(&dom, heading), "Installation guide");
    }

    #[test]
    fn heading_text_handles_deep_markup_iteratively() {
        let mut html = String::from("<h1>");
        html.push_str(&"<span>".repeat(4_000));
        html.push_str("Deep title");
        html.push_str(&"</span>".repeat(4_000));
        html.push_str("</h1>");
        let dom = Dom::parse_fragment(&html, Tag::Div).unwrap();
        let heading = dom.first_descendant_by_tag(dom.root(), Tag::H1).unwrap();

        assert_eq!(visible_heading_text(&dom, heading), "Deep title");
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
    fn extracts_a_combined_date_and_author_byline() {
        let result = metadata(
            r#"<title>Page | Project blog</title><article><header><h2>Page</h2></header><div class="publish-date-author">Aug. 19, 2026 · teor on behalf of <a href="/team">The Language Team</a></div><p>Article body.</p></article>"#,
            None,
            false,
        );

        assert_eq!(result.authors, ["teor"]);
        assert_eq!(result.published_time.as_deref(), Some("August 19, 2026"));
    }

    #[test]
    fn combined_byline_skips_reading_time_before_the_author() {
        let result = metadata(
            r#"<title>Page</title><article><h2>Page</h2><div class="published-date-author">Aug. 19, 2026 · 5 min read · By Jane Doe</div><p>Article body.</p></article>"#,
            None,
            false,
        );

        assert_eq!(result.authors, ["Jane Doe"]);
    }

    #[test]
    fn combined_byline_skips_discussion_and_category_segments() {
        for byline in [
            "Aug. 19, 2026 · 12 comments · Jane Doe",
            "Aug. 19, 2026 · Technology · Jane Doe",
            "Aug. 19, 2026 · Jane Doe · Technology",
        ] {
            let html = format!(
                r#"<title>Page</title><article><h2>Page</h2><div class="published-date-author">{byline}</div><p>Article body.</p></article>"#,
            );
            let result = metadata(&html, None, false);

            assert_eq!(result.authors, ["Jane Doe"], "{byline}");
        }
    }

    #[test]
    fn hidden_combined_byline_does_not_enable_metadata_inference() {
        let result = metadata(
            r#"<title>Page</title><article><h2>Page</h2><div hidden class="published-date-author">Aug. 19, 2026 · Hidden Writer</div><p>Article body.</p></article>"#,
            None,
            false,
        );

        assert!(result.authors.is_empty());
        assert!(result.published_time.is_none());
    }

    #[test]
    fn title_matching_sidebar_h2_does_not_enable_byline_inference() {
        let result = metadata(
            r#"<title>News</title><aside><h2>News</h2><div class="published-date-author">Aug. 19, 2026 · Sidebar Writer</div></aside><main><p>The primary page is an index without an author.</p></main>"#,
            None,
            false,
        );

        assert!(result.authors.is_empty());
        assert!(result.published_time.is_none());
    }

    #[test]
    fn abbreviated_written_date_uses_document_order() {
        assert_eq!(
            combined_written_date_value("Dec. 3, 2026 · Jane Doe · reviewed Aug. 1, 2027"),
            Some((0, "December 3, 2026".to_owned()))
        );
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
            written_date("Published July 29, 2026 Updated January 2, 2027")
                .map(|(_, date)| date)
                .as_deref(),
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
    fn structured_document_evidence_ignores_unrelated_products() {
        let dom = Dom::parse_document(
            r#"<script type="application/ld+json">[
                {"@context":"https://schema.org","@type":"SoftwareApplication","name":"Reference model","description":"Provider and pricing reference."},
                {"@context":"https://schema.org","@type":"Product","name":"Unrelated recommendation","description":"A recommended product card."}
            ]</script>"#,
        )
        .unwrap();
        let data = StructuredData::parse(&dom, &ParseBudget::default()).unwrap();
        let url = Url::parse("https://example.test/reference/model").unwrap();
        assert_eq!(
            data.document_evidence_count("Reference model", Some(&url)),
            1
        );
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
    fn job_salary_units_are_not_prefixed_twice() {
        assert_eq!(
            format_job_salary("USD", "150000", "", Some("per year")),
            "USD 150000 per year"
        );
        assert_eq!(
            format_job_salary("USD", "12500", "", Some("MONTHLY")),
            "USD 12500 per month"
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

    fn article_page(head: &str, article: &str) -> String {
        format!(
            "<html lang=\"en\"><head>{head}</head><body><nav>Home Topics</nav><main><article>\
             {article}<p>The article body has enough text to describe the event in detail.</p>\
             </article></main></body></html>"
        )
    }

    #[test]
    fn a_visible_heading_and_document_title_outweigh_a_stale_og_title() {
        let html = article_page(
            "<title>Archive opens downtown</title>\
             <meta property=\"og:title\" content=\"Plans announced for an archive\">",
            "<h1>Archive opens downtown</h1>",
        );
        assert_eq!(
            metadata(&html, None, false).title.as_deref(),
            Some("Archive opens downtown")
        );
    }

    #[test]
    fn a_disagreeing_heading_does_not_override_an_agreed_og_title() {
        let html = article_page(
            "<title>Archive opens downtown</title>\
             <meta property=\"og:title\" content=\"Archive opens downtown\">",
            "<h1>Readers share their archive memories</h1>",
        );
        assert_eq!(
            metadata(&html, None, false).title.as_deref(),
            Some("Archive opens downtown")
        );
    }

    #[test]
    fn a_masthead_title_yields_to_the_article_heading() {
        let html = "<html><head><title>Regional Desk</title></head><body>\
            <header><h1>Regional Desk</h1></header>\
            <main><article><h1>Wetland board approves restoration</h1>\
            <p>The approved work reconnects two channels.</p></article></main>\
            <footer><h1>More reporting</h1></footer></body></html>";
        assert_eq!(
            metadata(html, None, false).title.as_deref(),
            Some("Wetland board approves restoration")
        );
    }

    #[test]
    fn a_nested_component_heading_is_not_a_masthead() {
        // A title that matches a heading in a nested component header keeps
        // its confidence. A long caption heading must not replace it.
        let html = "<html><head><title>Cargo ship breaks up over Siberia</title></head><body>\
            <div><div><div><header><h1>Cargo ship breaks up over Siberia</h1></header></div></div></div>\
            <div><article><h1>In this photo the rocket booster is on the launch pad</h1>\
            <p>The ship broke up minutes after launch.</p></article></div></body></html>";
        assert_eq!(
            metadata(html, None, false).title.as_deref(),
            Some("Cargo ship breaks up over Siberia")
        );
    }

    #[test]
    fn a_content_root_language_overrides_a_conflicting_document_language() {
        let html = "<html lang=\"en\"><head><meta property=\"og:locale\" content=\"fr_CA\"></head>\
            <body><main><article lang=\"fr\"><h1>Le réseau ajoute trois stations</h1>\
            <p>Les nouvelles stations ouvrent lundi.</p></article></main></body></html>";
        assert_eq!(metadata(html, None, false).language.as_deref(), Some("fr"));
    }

    #[test]
    fn a_compatible_or_inline_language_keeps_the_document_language() {
        let regional = "<html lang=\"en\"><body><article lang=\"en-GB\"><h1>Rail plans</h1>\
            <p>Text.</p></article></body></html>";
        assert_eq!(
            metadata(regional, None, false).language.as_deref(),
            Some("en")
        );
        let inline = "<html lang=\"en\"><body><article><h1><span lang=\"fr\">Bonjour</span> \
            readers</h1><p>Text.</p></article></body></html>";
        assert_eq!(
            metadata(inline, None, false).language.as_deref(),
            Some("en")
        );
        let meta_only = "<html lang=\"en\"><head><meta http-equiv=\"content-language\" \
            content=\"fr\"></head><body><article><h1>Title</h1></article></body></html>";
        assert_eq!(
            metadata(meta_only, None, false).language.as_deref(),
            Some("en")
        );
    }

    #[test]
    fn splits_a_declared_list_of_full_person_names() {
        for value in ["Dana Chen; Priya Rao", "By Dana Chen; Priya Rao"] {
            let html = format!("<head><meta name=\"author\" content=\"{value}\"></head>");
            assert_eq!(
                metadata(&html, None, false).authors,
                ["Dana Chen", "Priya Rao"],
                "{value}"
            );
        }
        let html = "<head><meta name=\"author\" \
            content=\"Dana Chen, Priya Rao, and Ana de la Cruz\"></head>";
        assert_eq!(
            metadata(html, None, false).authors,
            ["Dana Chen", "Priya Rao", "Ana de la Cruz"]
        );
    }

    #[test]
    fn keeps_inverted_names_roles_and_organizations_as_one_author() {
        for (value, expected) in [
            ("Smith, John", "Smith, John"),
            ("Jane Doe, PhD", "Jane Doe, PhD"),
            ("Johnson & Johnson", "Johnson & Johnson"),
            ("Simon and Schuster", "Simon and Schuster"),
            (
                "Jane Doe and Associated Press",
                "Jane Doe and Associated Press",
            ),
            ("Jane Doe, Senior Editor", "Jane Doe"),
        ] {
            let html = format!("<head><meta name=\"author\" content=\"{value}\"></head>");
            assert_eq!(metadata(&html, None, false).authors, [expected], "{value}");
        }
    }

    #[test]
    fn a_two_name_comma_list_splits_only_with_byline_corroboration() {
        // One comma can join two people or invert one name.
        let corroborated = article_page(
            "<meta name=\"author\" content=\"Dana Chen, Priya Rao\">",
            "<h1>Joint survey maps urban tree cover</h1><p class=\"byline\">By \
             <span class=\"author\">Dana Chen</span> and <span class=\"author\">Priya Rao</span></p>",
        );
        assert_eq!(
            metadata(&corroborated, None, false).authors,
            ["Dana Chen", "Priya Rao"]
        );
        let alone = "<head><meta name=\"author\" content=\"García Márquez, Gabriel José\"></head>";
        assert_eq!(
            metadata(alone, None, false).authors,
            ["García Márquez, Gabriel José"]
        );
    }

    #[test]
    fn per_author_citation_and_dublin_core_fields_never_split() {
        let html = "<head>\
            <meta name=\"citation_author\" content=\"García Márquez, Gabriel José\">\
            <meta name=\"citation_author\" content=\"Doe, Jane, Roe, Richard, Poe, Edgar\">\
            <meta name=\"dc.creator\" content=\"Dana Chen and Priya Rao\"></head>";
        let authors = metadata(html, None, false).authors;
        assert!(authors.contains(&"García Márquez, Gabriel José".to_owned()));
        assert!(!authors.iter().any(|author| author == "Gabriel José"));
        assert!(!authors.iter().any(|author| author == "Priya Rao"));
        let general = "<head><meta name=\"author\" \
            content=\"Dana Chen, Priya Rao, Ana Ruiz\"></head>";
        assert_eq!(
            metadata(general, None, false).authors,
            ["Dana Chen", "Priya Rao", "Ana Ruiz"]
        );
    }

    #[test]
    fn collects_a_labelled_visible_dateline_near_the_heading() {
        let html = article_page(
            "<title>Audit published</title>",
            "<h1>Audit published</h1><p class=\"published\">Published August 13, 2026</p>",
        );
        assert_eq!(
            metadata(&html, None, false).published_time.as_deref(),
            Some("August 13, 2026")
        );
        let unlabelled = article_page(
            "<title>Audit published</title>",
            "<h1>Audit published</h1><div class=\"post-date\">Aug 13, 2026</div>",
        );
        assert_eq!(
            metadata(&unlabelled, None, false).published_time.as_deref(),
            Some("August 13, 2026")
        );
    }

    #[test]
    fn ignores_body_dates_update_lines_and_unlabelled_dates() {
        let far = format!(
            "<html><body><article><h1>Audit published</h1>{}\
             <p class=\"published\">Published August 13, 2026</p></article></body></html>",
            "<p>Body paragraph with enough words.</p>".repeat(8)
        );
        assert_eq!(metadata(&far, None, false).published_time, None);
        for line in [
            "<p>Updated August 13, 2026</p>",
            "<p>August 13, 2026 was the day the audit began.</p>",
            "<p>Published in 1998, the original report on August 13, 2026 remains open.</p>",
            "<p class=\"date\">Last reviewed on August 13, 2026</p>",
        ] {
            let html = article_page("", &format!("<h1>Audit published</h1>{line}"));
            assert_eq!(metadata(&html, None, false).published_time, None, "{line}");
        }
    }

    #[test]
    fn a_byline_time_keeps_its_machine_readable_value() {
        let html = article_page(
            "",
            "<h1>Cooling upgrade cuts energy use</h1><p class=\"byline\">By \
             <span rel=\"author\">Dana Chen</span> · <time datetime=\"2026-08-13\">August 13, \
             2026</time> · <a rel=\"tag\">Engineering</a></p>",
        );
        let result = metadata(&html, None, false);
        assert_eq!(result.published_time.as_deref(), Some("2026-08-13"));
        assert_eq!(result.section.as_deref(), Some("Engineering"));
    }

    #[test]
    fn several_byline_tag_links_do_not_choose_a_section() {
        let html = article_page(
            "",
            "<h1>Cooling upgrade cuts energy use</h1><p class=\"byline\">By Dana Chen · \
             <time datetime=\"2026-08-13\">August 13, 2026</time> · \
             <a rel=\"tag\">Engineering</a> <a rel=\"tag\">Energy</a></p>",
        );
        assert_eq!(metadata(&html, None, false).section, None);
    }

    #[test]
    fn rejects_placeholder_and_publication_name_tags() {
        let html = "<head><meta property=\"og:site_name\" content=\"Daily Planet\">\
            <meta property=\"article:tag\" content=\"none\">\
            <meta property=\"article:tag\" content=\"Wearables\">\
            <meta name=\"keywords\" content=\"undefined, Health, Example News, Daily Planet, \
            Example, Planet\"></head>";
        assert_eq!(
            metadata(html, Some("https://www.example.com/story"), false).tags,
            ["Wearables", "Health", "Example", "Planet"]
        );
    }

    #[test]
    fn an_update_time_does_not_replace_the_publication_date() {
        let update_label = article_page(
            "",
            "<h1>Audit published</h1><p class=\"byline\">By Dana Chen · Published August 13, \
             2026 · Updated <time datetime=\"2026-08-14\">August 14, 2026</time></p>",
        );
        assert_eq!(
            metadata(&update_label, None, false)
                .published_time
                .as_deref(),
            Some("August 13, 2026")
        );
        let update_class = article_page(
            "",
            "<h1>Audit published</h1><p class=\"byline\">By Dana Chen · \
             <time class=\"updated\" datetime=\"2026-08-14\">Aug 14</time> \
             <time class=\"published\" datetime=\"2026-08-13\">Aug 13</time></p>",
        );
        assert_eq!(
            metadata(&update_class, None, false)
                .published_time
                .as_deref(),
            Some("2026-08-13")
        );
        let publication = article_page(
            "",
            "<h1>Audit published</h1><p class=\"byline\">By Dana Chen · Published \
             <time datetime=\"2026-08-13\">August 13, 2026</time></p>",
        );
        assert_eq!(
            metadata(&publication, None, false)
                .published_time
                .as_deref(),
            Some("2026-08-13")
        );
    }

    #[test]
    fn modification_tokens_reject_a_dateline() {
        for line in [
            "<p class=\"date updated\">August 14, 2026</p>",
            "<p class=\"date modified\">August 14, 2026</p>",
            "<p class=\"published\" itemprop=\"dateModified\">Published August 14, 2026</p>",
            "<p class=\"date\">Revised August 14, 2026</p>",
        ] {
            let html = article_page("", &format!("<h1>Audit published</h1>{line}"));
            assert_eq!(metadata(&html, None, false).published_time, None, "{line}");
        }
        let script_hook = article_page(
            "",
            "<h1>Audit published</h1><span id=\"update_posted_time\" class=\"buzz_datetime\">\
             posted on April 21, 2015, at 11:29 a.m.</span>",
        );
        assert_eq!(
            metadata(&script_hook, None, false)
                .published_time
                .as_deref(),
            Some("April 21, 2015")
        );
        let html = article_page(
            "",
            "<h1>Audit published</h1><p class=\"date published\">August 13, 2026</p>",
        );
        assert_eq!(
            metadata(&html, None, false).published_time.as_deref(),
            Some("August 13, 2026")
        );
    }

    #[test]
    fn category_evidence_outranks_a_topic_tag_for_the_section() {
        let html = article_page(
            "",
            "<h1>Cooling upgrade cuts energy use</h1><p class=\"byline\">By Dana Chen · \
             <time datetime=\"2026-08-13\">August 13, 2026</time> · \
             <a href=\"/category/science\">Science</a> \
             <a rel=\"tag\" href=\"/tag/climate\">Climate</a></p>",
        );
        assert_eq!(
            metadata(&html, None, false).section.as_deref(),
            Some("Science")
        );
    }

    #[test]
    fn tags_are_compared_only_with_the_resolved_site_name() {
        let html = "<head><meta property=\"og:site_name\" content=\"Example Journal\">\
            <meta name=\"application-name\" content=\"Science\">\
            <meta name=\"keywords\" content=\"Science, Physics, Example Journal\"></head>";
        let result = metadata(html, Some("https://journal.test/story"), false);
        assert_eq!(result.site_name.as_deref(), Some("Example Journal"));
        assert_eq!(result.tags, ["Science", "Physics"]);
    }

    #[test]
    fn a_dateline_modification_time_is_not_restored_by_its_text() {
        for line in [
            "<p class=\"date\"><time class=\"updated\" datetime=\"2026-08-14\">August 14, \
             2026</time></p>",
            "<p class=\"date\"><span itemprop=\"dateModified\">August 14, 2026</span></p>",
        ] {
            let html = article_page("", &format!("<h1>Audit published</h1>{line}"));
            assert_eq!(metadata(&html, None, false).published_time, None, "{line}");
        }
        let html = article_page(
            "",
            "<h1>Audit published</h1><p class=\"published\"><time datetime=\"2026-08-13\">\
             August 13, 2026</time></p>",
        );
        assert_eq!(
            metadata(&html, None, false).published_time.as_deref(),
            Some("2026-08-13")
        );
    }

    #[test]
    fn a_styled_update_label_before_whitespace_marks_the_time() {
        let html = article_page(
            "",
            "<h1>Audit published</h1><p class=\"byline\">By Dana Chen · Published August 13, \
             2026 · <span>Updated</span> <time datetime=\"2026-08-14\">August 14, 2026</time></p>",
        );
        assert_eq!(
            metadata(&html, None, false).published_time.as_deref(),
            Some("August 13, 2026")
        );
        let published = article_page(
            "",
            "<h1>Audit published</h1><p class=\"byline\">By Dana Chen · <span>Published</span> \
             <time datetime=\"2026-08-13\">August 13, 2026</time></p>",
        );
        assert_eq!(
            metadata(&published, None, false).published_time.as_deref(),
            Some("2026-08-13")
        );
    }

    #[test]
    fn hidden_times_do_not_override_visible_dates() {
        for hidden in ["hidden", "aria-hidden=\"true\"", "style=\"display:none\""] {
            let html = article_page(
                "",
                &format!(
                    "<h1>Audit published</h1><p class=\"byline\">By Dana Chen · Published \
                     August 13, 2026 <time {hidden} datetime=\"2026-08-14\">August 14, \
                     2026</time></p>"
                ),
            );
            assert_eq!(
                metadata(&html, None, false).published_time.as_deref(),
                Some("August 13, 2026"),
                "{hidden}"
            );
        }
        let html = article_page(
            "",
            "<h1>Audit published</h1><p class=\"byline\">By Dana Chen · <span hidden>\
             <time datetime=\"2026-08-14\">August 14, 2026</time></span> \
             <time datetime=\"2026-08-13\">August 13, 2026</time></p>",
        );
        assert_eq!(
            metadata(&html, None, false).published_time.as_deref(),
            Some("2026-08-13")
        );
    }

    #[test]
    fn a_sectioned_article_header_is_not_a_masthead() {
        let html = "<html><head><title>Wetland board approves restoration</title></head><body>\
            <section><header><h1>Wetland board approves restoration</h1></header>\
            <article><h1>How the restoration will work</h1>\
            <p>The approved work reconnects two channels.</p></article></section></body></html>";
        assert_eq!(
            metadata(html, None, false).title.as_deref(),
            Some("Wetland board approves restoration")
        );
    }

    #[test]
    fn a_duration_or_clock_datetime_is_not_a_publication_date() {
        for datetime in ["PT5M", "10:30"] {
            let html = article_page(
                "",
                &format!(
                    "<h1>Audit published</h1><p class=\"byline\">By Dana Chen · \
                     <time datetime=\"{datetime}\">5 min read</time> · Published August 13, \
                     2026</p>"
                ),
            );
            assert_eq!(
                metadata(&html, None, false).published_time.as_deref(),
                Some("August 13, 2026"),
                "{datetime}"
            );
        }
        let html = article_page(
            "",
            "<h1>Audit published</h1><p class=\"byline\">By Dana Chen · \
             <time datetime=\"2026-08-13T10:30:00Z\">August 13, 2026</time></p>",
        );
        assert_eq!(
            metadata(&html, None, false).published_time.as_deref(),
            Some("2026-08-13T10:30:00Z")
        );
    }

    #[test]
    fn a_modification_wrapper_rejects_a_nested_dateline() {
        let wrapped = "<html><body><article><header><h1>Audit published</h1>\
            <div class=\"updated\"><span class=\"date\">August 14, 2026</span></div>\
            </header><p>The audit records completed improvements.</p></article></body></html>";
        assert_eq!(metadata(wrapped, None, false).published_time, None);
        let published = "<html><body><article class=\"updated\"><header><h1>Audit published</h1>\
            <div class=\"meta\"><span class=\"entry-date\">August 13, 2026</span></div>\
            </header><p>The audit records completed improvements.</p></article></body></html>";
        assert_eq!(
            metadata(published, None, false).published_time.as_deref(),
            Some("August 13, 2026")
        );
    }

    #[test]
    fn a_repeated_joined_author_list_does_not_remain_as_one_author() {
        let html = article_page(
            "<meta name=\"author\" content=\"Dana Chen, Priya Rao and Ana Ruiz\">",
            "<h1>Joint survey maps tree cover</h1>\
             <p><a rel=\"author\" href=\"/people\">Dana Chen, Priya Rao and Ana Ruiz</a></p>",
        );
        assert_eq!(
            metadata(&html, None, false).authors,
            ["Dana Chen", "Priya Rao", "Ana Ruiz"]
        );
        let third = article_page(
            "<meta name=\"author\" content=\"Dana Chen, Priya Rao and Ana Ruiz\">",
            "<h1>Joint survey maps tree cover</h1>\
             <p><a rel=\"author\" href=\"/people/lee\">Lee Park</a></p>",
        );
        assert_eq!(
            metadata(&third, None, false).authors,
            ["Dana Chen", "Priya Rao", "Ana Ruiz", "Lee Park"]
        );
    }

    #[test]
    fn a_two_part_list_needs_corroboration_and_no_organization_word() {
        for value in [
            "Davis Polk &amp; Wardwell LLP",
            "Dana Chen and Priya Rao",
            "Dana Chen & Priya Rao",
        ] {
            let html = format!("<head><meta name=\"author\" content=\"{value}\"></head>");
            assert_eq!(metadata(&html, None, false).authors.len(), 1, "{value}");
        }
        let corroborated = article_page(
            "<meta name=\"author\" content=\"Dana Chen &amp; Priya Rao\">",
            "<h1>Joint survey maps tree cover</h1><p class=\"byline\">By \
             <span class=\"author\">Dana Chen</span></p>",
        );
        assert_eq!(
            metadata(&corroborated, None, false).authors,
            ["Dana Chen", "Priya Rao"]
        );
        // A corroborated part does not split a firm name.
        let firm = article_page(
            "<meta name=\"author\" content=\"Davis Polk &amp; Wardwell Partners\">",
            "<h1>Merger guidance</h1><p class=\"byline\">By \
             <span class=\"author\">Davis Polk</span></p>",
        );
        assert!(
            !metadata(&firm, None, false)
                .authors
                .iter()
                .any(|author| author == "Wardwell Partners")
        );
        for value in [
            "Dana Chen, Priya Rao, Acme Group",
            "Dana Chen; Northwind Media",
        ] {
            let html = format!("<head><meta name=\"author\" content=\"{value}\"></head>");
            assert_eq!(metadata(&html, None, false).authors.len(), 1, "{value}");
        }
    }

    #[test]
    fn a_title_derived_from_a_heading_does_not_corroborate_it() {
        let html = "<html><head><title>News</title>\
            <meta property=\"og:title\" content=\"New train services to begin in May\"></head>\
            <body><main><h1>All the latest news and updates</h1><article>\
            <h2>New train services to begin in May</h2><p>The services begin on the coast line.</p>\
            </article></main></body></html>";
        assert_eq!(
            metadata(html, None, false).title.as_deref(),
            Some("New train services to begin in May")
        );
        // A declared title with a site segment still corroborates the heading.
        let declared = "<html><head><title>Community archive opens downtown today | City Paper</title>\
            <meta property=\"og:title\" content=\"Plans announced for an archive\"></head>\
            <body><main><article><h1>Community archive opens downtown today</h1><p>Text.</p>\
            </article></main></body></html>";
        assert_eq!(
            metadata(declared, None, false).title.as_deref(),
            Some("Community archive opens downtown today")
        );
    }

    fn byline_date(byline: &str) -> Option<String> {
        let html = article_page(
            "",
            &format!("<h1>Audit published</h1><p class=\"byline\">{byline}</p>"),
        );
        metadata(&html, None, false).published_time
    }

    #[test]
    fn the_end_of_a_long_preceding_label_marks_an_update_time() {
        assert_eq!(
            byline_date(
                "By Alexandra Montgomery and Christopher Wellington · Published August 13, 2026 \
                 · Updated <time datetime=\"2026-08-14\">August 14, 2026</time>"
            )
            .as_deref(),
            Some("August 13, 2026")
        );
        // The label is at the end of a long sibling, beyond a prefix window.
        assert_eq!(
            byline_date(
                "<span>By Alexandra Montgomery and Christopher Wellington of the Regional \
                 Environment Desk · Updated</span> <time datetime=\"2026-08-14\">Aug 14</time> \
                 · <time datetime=\"2026-08-13\">Aug 13</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
    }

    #[test]
    fn a_connector_after_an_update_label_still_marks_the_time() {
        for connector in ["at", "on", "at:"] {
            assert_eq!(
                byline_date(&format!(
                    "By Dana Chen · Updated {connector} <time datetime=\"2026-08-14T10:00:00Z\">\
                     10:00 on August 14, 2026</time> · <time datetime=\"2026-08-13\">Aug \
                     13</time>"
                ))
                .as_deref(),
                Some("2026-08-13"),
                "{connector}"
            );
        }
    }

    #[test]
    fn a_labelled_publication_date_outranks_an_unlabelled_time() {
        assert_eq!(
            byline_date(
                "By Dana Chen · <time datetime=\"2026-08-14\">Aug 14</time> · Published \
                 August 13, 2026"
            )
            .as_deref(),
            Some("August 13, 2026")
        );
        assert_eq!(
            byline_date(
                "By Dana Chen · <time datetime=\"2026-08-14\">Aug 14</time> · \
                 <time itemprop=\"datePublished\" datetime=\"2026-08-13\">Aug 13</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
        assert_eq!(
            byline_date("By Dana Chen · First published August 12, 2026").as_deref(),
            Some("August 12, 2026")
        );
        // Without a publication label, the first date in source order wins.
        assert_eq!(
            byline_date(
                "By Dana Chen · <time datetime=\"2026-08-14\">August 14, 2026</time> · \
                 August 13, 2026"
            )
            .as_deref(),
            Some("2026-08-14")
        );
        assert_eq!(
            byline_date(
                "By Dana Chen · August 13, 2026 · <time datetime=\"2026-08-14\">August 14, \
                 2026</time>"
            )
            .as_deref(),
            Some("August 13, 2026")
        );
    }

    #[test]
    fn a_label_assembled_across_connector_nodes_marks_an_update() {
        assert_eq!(
            byline_date(
                "By Dana Chen · August 13, 2026 · <span>Updated</span> on \
                 <time datetime=\"2026-08-14\">August 14, 2026</time>"
            )
            .as_deref(),
            Some("August 13, 2026")
        );
        assert_eq!(
            byline_date(
                "By Dana Chen · <span>Updated</span><span>:</span> <span>at</span> \
                 <time datetime=\"2026-08-14\">Aug 14</time> · \
                 <time datetime=\"2026-08-13\">Aug 13</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
        // A separator ends the label, so an earlier update label does not
        // reach a later date.
        assert_eq!(
            byline_date(
                "By Dana Chen · <span>Updated</span> · <time datetime=\"2026-08-13\">Aug \
                 13</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
    }

    #[test]
    fn a_publication_marker_wins_over_a_modification_marker_on_one_time() {
        assert_eq!(
            byline_date(
                "By Dana Chen · <time class=\"entry-date published updated\" \
                 datetime=\"2026-08-13\">August 13, 2026</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
        assert_eq!(
            byline_date(
                "By Dana Chen · <time class=\"updated\" datetime=\"2026-08-14\">August 14, \
                 2026</time><time class=\"entry-date published\" datetime=\"2026-08-13\">\
                 August 13, 2026</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
        assert_eq!(
            byline_date(
                "By Dana Chen · <time class=\"entry-date published\" datetime=\"2026-08-13\">\
                 August 13, 2026</time><time class=\"updated\" datetime=\"2026-08-14\">\
                 August 14, 2026</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
        let dateline = article_page(
            "",
            "<h1>Audit published</h1><p class=\"date published updated\">August 13, 2026</p>",
        );
        assert_eq!(
            metadata(&dateline, None, false).published_time.as_deref(),
            Some("August 13, 2026")
        );
        assert_eq!(
            byline_date(
                "By Dana Chen · <time class=\"updated\" datetime=\"2026-08-14\">August 14, \
                 2026</time>"
            ),
            None
        );
    }

    #[test]
    fn a_dateline_needs_publication_evidence() {
        for line in [
            "<p class=\"event-date\" itemprop=\"startDate\">September 12, 2026</p>",
            "<p class=\"event-date\">September 12, 2026</p>",
            "<p class=\"date\">September 12, 2026</p>",
            "<div itemprop=\"endDate\"><span class=\"published\">September 12, 2026</span></div>",
            "<p class=\"published\" itemprop=\"startDate\">September 12, 2026</p>",
        ] {
            let html = article_page("", &format!("<h1>Harvest festival</h1>{line}"));
            assert_eq!(metadata(&html, None, false).published_time, None, "{line}");
        }
        for line in [
            "<p class=\"date\">Published September 12, 2026</p>",
            "<p>First published September 12, 2026</p>",
            "<p class=\"entry-date\">September 12, 2026</p>",
            "<p class=\"postdate\">September 12, 2026</p>",
            "<p itemprop=\"datePublished\">September 12, 2026</p>",
        ] {
            let html = article_page("", &format!("<h1>Harvest festival</h1>{line}"));
            assert_eq!(
                metadata(&html, None, false).published_time.as_deref(),
                Some("September 12, 2026"),
                "{line}"
            );
        }
    }

    #[test]
    fn credentials_do_not_become_authors() {
        for value in [
            "Jane Smith, MD FACC",
            "Jane Smith, MD",
            "Jane Smith, M.D., Ph.D.",
        ] {
            let html = article_page(
                &format!("<meta name=\"author\" content=\"{value}\">"),
                "<h1>Heart study</h1><p><a rel=\"author\" href=\"/jane\">Jane Smith</a></p>",
            );
            // The value stays whole, as on `main`. No credential becomes an author.
            let authors = metadata(&html, None, false).authors;
            assert!(
                authors
                    .iter()
                    .all(|author| author.starts_with("Jane Smith")),
                "{value}: {authors:?}"
            );
        }
        let html = "<head><meta name=\"author\" content=\"Dana Chen, PhD, Priya Rao\"></head>";
        assert!(
            !metadata(html, None, false)
                .authors
                .iter()
                .any(|author| author == "PhD")
        );
        let capitals =
            "<head><meta name=\"author\" content=\"DANA CHEN, PRIYA RAO, ANA RUIZ\"></head>";
        assert_eq!(
            metadata(capitals, None, false).authors,
            ["DANA CHEN", "PRIYA RAO", "ANA RUIZ"]
        );
    }

    #[test]
    fn every_date_in_one_text_node_is_classified() {
        for byline in [
            "By Dana Chen · Updated August 14, 2026 · Published August 13, 2026",
            "By Dana Chen · Published August 13, 2026 · Updated August 14, 2026",
            "Published August 13, 2026 · Updated August 14, 2026",
        ] {
            assert_eq!(
                byline_date(byline).as_deref(),
                Some("August 13, 2026"),
                "{byline}"
            );
        }
        // An unlabelled date after an update stays available.
        assert_eq!(
            byline_date("By Dana Chen · Updated August 14, 2026 · August 13, 2026").as_deref(),
            Some("August 13, 2026")
        );
        assert_eq!(
            byline_date("By Dana Chen · Updated August 14, 2026").as_deref(),
            None
        );
    }

    #[test]
    fn hidden_label_text_is_skipped() {
        assert_eq!(
            byline_date(
                "By Dana Chen · Published <span hidden>Updated</span> \
                 <time datetime=\"2026-08-13\">August 13, 2026</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
        assert_eq!(
            byline_date(
                "By Dana Chen · <time datetime=\"2026-08-12\">Aug 12</time> · <span>Updated \
                 <b style=\"display:none\">never</b></span> <time datetime=\"2026-08-14\">Aug \
                 14</time>"
            )
            .as_deref(),
            Some("2026-08-12")
        );
    }

    #[test]
    fn a_derived_title_never_corroborates_any_heading() {
        for heading in [
            "<header><h1>All the latest news and updates</h1></header>",
            "<div class=\"top\"><h1>All the latest news and updates</h1></div>",
            "<main><h1>All the latest news and updates</h1></main>",
        ] {
            let html = format!(
                "<html><head><title>News</title><meta property=\"og:title\" \
                 content=\"New train services to begin in May\"></head><body>{heading}\
                 <article><h2>New train services to begin in May</h2>\
                 <p>The services begin on the coast line.</p></article></body></html>"
            );
            assert_eq!(
                metadata(&html, None, false).title.as_deref(),
                Some("New train services to begin in May"),
                "{heading}"
            );
        }
        // Without a competing declaration the heading is still the title.
        let html = "<html><head><title>News</title></head><body>\
            <header><h1>All the latest news and updates</h1></header>\
            <article><p>The services begin on the coast line.</p></article></body></html>";
        assert_eq!(
            metadata(html, None, false).title.as_deref(),
            Some("All the latest news and updates")
        );
    }

    #[test]
    fn a_visible_label_outranks_a_generic_post_date_class() {
        assert_eq!(
            byline_date(
                "By Dana Chen · August 13, 2026 · Updated <time class=\"entry-date\" \
                 datetime=\"2026-08-14\">August 14, 2026</time>"
            )
            .as_deref(),
            Some("August 13, 2026")
        );
        // A schema.org property still outranks a visible label.
        assert_eq!(
            byline_date(
                "By Dana Chen · Updated <time itemprop=\"datePublished\" \
                 datetime=\"2026-08-13\">August 13, 2026</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
        // Without a label, the generic class still names the publication date.
        assert_eq!(
            byline_date(
                "By Dana Chen · <time datetime=\"2026-08-14\">Aug 14</time> \
                 <time class=\"entry-date\" datetime=\"2026-08-13\">Aug 13</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
    }

    #[test]
    fn the_strongest_publication_evidence_wins_in_any_order() {
        assert_eq!(
            byline_date(
                "By Dana Chen · <time class=\"entry-date\" datetime=\"2026-08-14\">Aug 14, \
                 2026</time> · First published August 13, 2026"
            )
            .as_deref(),
            Some("August 13, 2026")
        );
        assert_eq!(
            byline_date(
                "By Dana Chen · First published August 13, 2026 · \
                 <time class=\"entry-date\" datetime=\"2026-08-14\">Aug 14, 2026</time>"
            )
            .as_deref(),
            Some("August 13, 2026")
        );
        // A schema.org property outranks a visible label in either order.
        assert_eq!(
            byline_date(
                "By Dana Chen · Published August 12, 2026 · \
                 <time itemprop=\"datePublished\" datetime=\"2026-08-13\">Aug 13</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
        // Equal evidence keeps source order.
        assert_eq!(
            byline_date("By Dana Chen · Posted August 12, 2026 · Published August 13, 2026")
                .as_deref(),
            Some("August 12, 2026")
        );
    }

    #[test]
    fn a_label_segment_contains_the_date_role() {
        for (label, expected) in [
            ("Last updated as of", Some("August 13, 2026")),
            ("Last updated date:", Some("August 13, 2026")),
            ("Updated on:", Some("August 13, 2026")),
            ("Published, last updated", Some("August 13, 2026")),
            ("First published as of:", Some("2026-08-14")),
        ] {
            assert_eq!(
                byline_date(&format!(
                    "August 13, 2026 · {label} <time class=\"entry-date\" \
                     datetime=\"2026-08-14\">August 14, 2026</time>"
                ))
                .as_deref(),
                expected,
                "{label}"
            );
        }
        // An earlier date ends the segment, so its label does not reach the
        // next date.
        assert_eq!(
            byline_date("By Dana Chen. Updated August 14, 2026 August 13, 2026").as_deref(),
            Some("August 13, 2026")
        );
        // An editor credit is not a modification label.
        assert_eq!(
            byline_date(
                "By Dana Chen, edited by Sam Roe <time datetime=\"2026-08-13\">Aug 13</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
    }

    #[test]
    fn the_label_scan_stops_at_an_earlier_time_element() {
        assert_eq!(
            byline_date(
                "By Dana Chen <span>Updated <time datetime=\"2026-08-14\">Aug 14</time></span> \
                 <span>Published <time datetime=\"2026-08-13\">August 13, 2026</time></span>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
        assert_eq!(
            byline_date(
                "By Dana Chen <span>Updated <time>yesterday</time></span> \
                 <time datetime=\"2026-08-13\">August 13, 2026</time>"
            )
            .as_deref(),
            Some("2026-08-13")
        );
    }
}
