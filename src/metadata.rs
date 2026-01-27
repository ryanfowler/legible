//! Metadata extraction from HTML documents.

use crate::constants::{HTML_ESCAPE_MAP, regexps};
use crate::scoring::get_inner_text;
use dom_query::Document;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

/// Metadata extracted from an article.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub title: Option<String>,
    pub byline: Option<String>,
    pub excerpt: Option<String>,
    pub site_name: Option<String>,
    pub published_time: Option<String>,
}

/// Unescape common HTML entities in a string.
pub fn unescape_html_entities(s: &str) -> String {
    if s.is_empty() {
        return s.to_string();
    }

    // First handle named entities
    let mut result = s.to_string();
    for (entity, char) in HTML_ESCAPE_MAP.iter() {
        let pattern = format!("&{};", entity);
        result = result.replace(&pattern, char);
    }

    // Then handle numeric entities (both hex and decimal)
    static NUMERIC_ENTITY: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"&#(?:x([0-9a-fA-F]+)|([0-9]+));").unwrap());

    let mut output = String::new();
    let mut last_end = 0;

    for caps in NUMERIC_ENTITY.captures_iter(&result) {
        let m = caps.get(0).unwrap();
        output.push_str(&result[last_end..m.start()]);

        let num = if let Some(hex) = caps.get(1) {
            u32::from_str_radix(hex.as_str(), 16).unwrap_or(0xFFFD)
        } else if let Some(dec) = caps.get(2) {
            dec.as_str().parse::<u32>().unwrap_or(0xFFFD)
        } else {
            0xFFFD
        };

        // Handle invalid character references as per HTML spec
        let num = if num == 0 || num > 0x10FFFF || (0xD800..=0xDFFF).contains(&num) {
            0xFFFD
        } else {
            num
        };

        if let Some(c) = char::from_u32(num) {
            output.push(c);
        } else {
            output.push('\u{FFFD}');
        }

        last_end = m.end();
    }

    output.push_str(&result[last_end..]);
    output
}

/// Extract JSON-LD metadata from the document.
pub fn get_json_ld(doc: &Document) -> Metadata {
    let mut metadata = Metadata::default();

    let scripts = doc.select("script[type='application/ld+json']");

    for script in scripts.iter() {
        let content = script.text();
        if content.is_empty() {
            continue;
        }

        // Strip CDATA markers if present
        let content = content
            .trim()
            .trim_start_matches("<![CDATA[")
            .trim_end_matches("]]>")
            .trim();

        let parsed: Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Handle array of JSON-LD objects
        let parsed = if let Value::Array(arr) = &parsed {
            arr.iter()
                .find(|it| {
                    if let Some(type_val) = it.get("@type").and_then(|t| t.as_str()) {
                        regexps::JSON_LD_ARTICLE_TYPES.is_match(type_val)
                    } else {
                        false
                    }
                })
                .cloned()
        } else {
            Some(parsed)
        };

        let parsed = match parsed {
            Some(p) => p,
            None => continue,
        };

        // Verify schema.org context
        let context = parsed.get("@context");
        let is_schema_org = match context {
            Some(Value::String(s)) => {
                static SCHEMA_ORG: Lazy<Regex> =
                    Lazy::new(|| Regex::new(r"^https?://schema\.org/?$").unwrap());
                SCHEMA_ORG.is_match(s)
            }
            Some(Value::Object(obj)) => {
                if let Some(Value::String(vocab)) = obj.get("@vocab") {
                    static SCHEMA_ORG: Lazy<Regex> =
                        Lazy::new(|| Regex::new(r"^https?://schema\.org/?$").unwrap());
                    SCHEMA_ORG.is_match(vocab)
                } else {
                    false
                }
            }
            _ => false,
        };

        if !is_schema_org {
            continue;
        }

        // Handle @graph structure
        let parsed = if parsed.get("@type").is_none() {
            if let Some(Value::Array(graph)) = parsed.get("@graph") {
                graph
                    .iter()
                    .find(|it| {
                        if let Some(type_val) = it.get("@type").and_then(|t| t.as_str()) {
                            regexps::JSON_LD_ARTICLE_TYPES.is_match(type_val)
                        } else {
                            false
                        }
                    })
                    .cloned()
            } else {
                None
            }
        } else {
            Some(parsed)
        };

        let parsed = match parsed {
            Some(p) => p,
            None => continue,
        };

        // Verify it's an article type
        let type_val = parsed.get("@type").and_then(|t| t.as_str());
        if let Some(t) = type_val {
            if !regexps::JSON_LD_ARTICLE_TYPES.is_match(t) {
                continue;
            }
        } else {
            continue;
        }

        // Extract title
        let name = parsed.get("name").and_then(|v| v.as_str());
        let headline = parsed.get("headline").and_then(|v| v.as_str());

        metadata.title = match (name, headline) {
            (Some(n), Some(h)) if n != h => {
                // Both exist and differ - prefer headline as it's usually the article title
                Some(h.trim().to_string())
            }
            (Some(n), _) => Some(n.trim().to_string()),
            (_, Some(h)) => Some(h.trim().to_string()),
            _ => None,
        };

        // Extract author/byline
        if let Some(author) = parsed.get("author") {
            if let Some(author_name) = author.get("name").and_then(|v| v.as_str()) {
                metadata.byline = Some(author_name.trim().to_string());
            } else if let Value::Array(authors) = author {
                let names: Vec<String> = authors
                    .iter()
                    .filter_map(|a| a.get("name").and_then(|v| v.as_str()))
                    .map(|s| s.trim().to_string())
                    .collect();
                if !names.is_empty() {
                    metadata.byline = Some(names.join(", "));
                }
            }
        }

        // Extract description/excerpt
        if let Some(desc) = parsed.get("description").and_then(|v| v.as_str()) {
            metadata.excerpt = Some(desc.trim().to_string());
        }

        // Extract site name
        if let Some(publisher) = parsed.get("publisher")
            && let Some(pub_name) = publisher.get("name").and_then(|v| v.as_str())
        {
            metadata.site_name = Some(pub_name.trim().to_string());
        }

        // Extract published time
        if let Some(date) = parsed.get("datePublished").and_then(|v| v.as_str()) {
            metadata.published_time = Some(date.trim().to_string());
        }

        // Found valid JSON-LD, stop looking
        break;
    }

    metadata
}

/// Get the article title from the document.
pub fn get_article_title(doc: &Document) -> String {
    let title_elem = doc.select("title");
    let mut cur_title = title_elem.text().trim().to_string();
    let orig_title = cur_title.clone();

    if cur_title.is_empty() {
        return String::new();
    }

    let mut title_had_hierarchical_separators = false;

    fn word_count(s: &str) -> usize {
        s.split_whitespace().count()
    }

    if regexps::TITLE_SEPARATOR.is_match(&cur_title) {
        // Check for hierarchical separators
        title_had_hierarchical_separators = regexps::TITLE_HIERARCHICAL.is_match(&cur_title);

        // Find all separators and split at the last one
        let matches: Vec<_> = regexps::TITLE_SEPARATOR.find_iter(&orig_title).collect();
        if let Some(last_match) = matches.last() {
            cur_title = orig_title[..last_match.start()].to_string();
        }

        // If the resulting title is too short, remove the first part instead
        if word_count(&cur_title) < 3 {
            cur_title = regexps::TITLE_FIRST_PART.replace(&orig_title, "").to_string();
        }
    } else if cur_title.contains(": ") {
        // Check if we have a heading containing this exact string
        let headings = doc.select("h1, h2");
        let trimmed_title = cur_title.trim();
        let has_match = headings.iter().any(|h| h.text().trim() == trimmed_title);

        if !has_match {
            // Extract title after the last colon
            if let Some(pos) = orig_title.rfind(": ") {
                cur_title = orig_title[pos + 2..].to_string();

                // If too short, try first colon
                if word_count(&cur_title) < 3
                    && let Some(pos) = orig_title.find(": ")
                {
                    let before_colon = &orig_title[..pos];
                    if word_count(before_colon) <= 5 {
                        cur_title = orig_title[pos + 2..].to_string();
                    } else {
                        cur_title = orig_title.clone();
                    }
                }
            }
        }
    } else if cur_title.len() > 150 || cur_title.len() < 15 {
        // Title too long or short, try H1
        let h1s = doc.select("h1");
        if h1s.length() == 1
            && let Some(h1) = h1s.nodes().first()
        {
            cur_title = get_inner_text(h1, true);
        }
    }

    // Normalize whitespace
    cur_title = regexps::NORMALIZE
        .replace_all(cur_title.trim(), " ")
        .to_string();

    // If we now have 4 words or fewer and conditions are met, use original title
    let cur_title_word_count = word_count(&cur_title);
    if cur_title_word_count <= 4 {
        let orig_without_separators = regexps::TITLE_SEPARATOR.replace_all(&orig_title, "");
        let orig_word_count = word_count(&orig_without_separators);

        if !title_had_hierarchical_separators || cur_title_word_count != orig_word_count - 1 {
            cur_title = orig_title;
        }
    }

    cur_title
}

/// Get article metadata from meta tags and JSON-LD.
pub fn get_article_metadata(doc: &Document, json_ld: &Metadata, article_title: &str) -> Metadata {
    let mut metadata = Metadata::default();

    // Property pattern: article:author, og:title, etc.
    static PROPERTY_PATTERN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\s*(article|dc|dcterm|og|twitter)\s*:\s*(author|creator|description|published_time|title|site_name)\s*").unwrap()
    });

    // Name pattern for meta name attributes
    static NAME_PATTERN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^\s*(?:(dc|dcterm|og|twitter|parsely|weibo:(article|webpage))\s*[-\.:]?\s*)?(author|creator|pub-date|description|title|site_name)\s*$").unwrap()
    });

    let mut values: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let metas = doc.select("meta");
    for meta in metas.iter() {
        let content = match meta.attr("content") {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => continue,
        };

        // Check property attribute
        if let Some(property) = meta.attr("property")
            && let Some(caps) = PROPERTY_PATTERN.captures(property.as_ref())
        {
            let name = caps
                .get(0)
                .unwrap()
                .as_str()
                .to_lowercase()
                .replace(char::is_whitespace, "");
            values.insert(name, content.trim().to_string());
            continue;
        }

        // Check name attribute
        if let Some(name_attr) = meta.attr("name")
            && NAME_PATTERN.is_match(name_attr.as_ref())
        {
            let name = name_attr
                .to_lowercase()
                .replace(char::is_whitespace, "")
                .replace('.', ":");
            values.insert(name, content.trim().to_string());
        }
    }

    // Get title from various sources
    metadata.title = json_ld
        .title
        .clone()
        .or_else(|| values.get("dc:title").cloned())
        .or_else(|| values.get("dcterm:title").cloned())
        .or_else(|| values.get("og:title").cloned())
        .or_else(|| values.get("weibo:article:title").cloned())
        .or_else(|| values.get("weibo:webpage:title").cloned())
        .or_else(|| values.get("title").cloned())
        .or_else(|| values.get("twitter:title").cloned())
        .or_else(|| values.get("parsely-title").cloned());

    if metadata.title.is_none() && !article_title.is_empty() {
        metadata.title = Some(article_title.to_string());
    }

    // Get author/byline
    let article_author = values.get("article:author").and_then(|v| {
        // Skip if it looks like a URL
        if is_url(v) { None } else { Some(v.clone()) }
    });

    metadata.byline = json_ld
        .byline
        .clone()
        .or_else(|| values.get("dc:creator").cloned())
        .or_else(|| values.get("dcterm:creator").cloned())
        .or_else(|| values.get("author").cloned())
        .or_else(|| values.get("parsely-author").cloned())
        .or(article_author);

    // Get excerpt/description
    metadata.excerpt = json_ld
        .excerpt
        .clone()
        .or_else(|| values.get("dc:description").cloned())
        .or_else(|| values.get("dcterm:description").cloned())
        .or_else(|| values.get("og:description").cloned())
        .or_else(|| values.get("weibo:article:description").cloned())
        .or_else(|| values.get("weibo:webpage:description").cloned())
        .or_else(|| values.get("description").cloned())
        .or_else(|| values.get("twitter:description").cloned());

    // Get site name
    metadata.site_name = json_ld
        .site_name
        .clone()
        .or_else(|| values.get("og:site_name").cloned());

    // Get published time
    metadata.published_time = json_ld
        .published_time
        .clone()
        .or_else(|| values.get("article:published_time").cloned())
        .or_else(|| values.get("parsely-pub-date").cloned());

    // Unescape HTML entities in metadata
    metadata.title = metadata.title.map(|s| unescape_html_entities(&s));
    metadata.byline = metadata.byline.map(|s| unescape_html_entities(&s));
    metadata.excerpt = metadata.excerpt.map(|s| unescape_html_entities(&s));
    metadata.site_name = metadata.site_name.map(|s| unescape_html_entities(&s));
    metadata.published_time = metadata.published_time.map(|s| unescape_html_entities(&s));

    metadata
}

/// Check if a string looks like a URL.
fn is_url(s: &str) -> bool {
    url::Url::parse(s).is_ok()
}

/// Calculate text similarity between two strings.
/// Returns a value between 0 (completely different) and 1 (identical).
pub fn text_similarity(text_a: &str, text_b: &str) -> f64 {
    let text_a_lower = text_a.to_lowercase();
    let text_b_lower = text_b.to_lowercase();

    let tokens_a: Vec<&str> = regexps::TOKENIZE
        .split(&text_a_lower)
        .filter(|s| !s.is_empty())
        .collect();

    let tokens_b: Vec<&str> = regexps::TOKENIZE
        .split(&text_b_lower)
        .filter(|s| !s.is_empty())
        .collect();

    if tokens_a.is_empty() || tokens_b.is_empty() {
        return 0.0;
    }

    let unique_tokens_b: Vec<&str> = tokens_b
        .iter()
        .filter(|t| !tokens_a.contains(t))
        .copied()
        .collect();

    let tokens_b_len: usize =
        tokens_b.iter().map(|s| s.len()).sum::<usize>() + tokens_b.len().saturating_sub(1);
    let unique_b_len: usize = unique_tokens_b.iter().map(|s| s.len()).sum::<usize>()
        + unique_tokens_b.len().saturating_sub(1);

    if tokens_b_len == 0 {
        return 0.0;
    }

    let distance_b = unique_b_len as f64 / tokens_b_len as f64;
    1.0 - distance_b
}
