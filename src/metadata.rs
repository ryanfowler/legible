//! Metadata extraction from HTML documents.

use crate::constants::regexps;
use crate::scoring::get_inner_text;
use crate::selectors::Selectors;
use dom_query::Document;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashSet;

/// Metadata extracted from an article.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub title: Option<String>,
    pub byline: Option<String>,
    pub excerpt: Option<String>,
    pub site_name: Option<String>,
    pub published_time: Option<String>,
}

/// Unescape common HTML entities in a string using byte-scanning for speed.
/// Handles both named entities (&lt;, &gt;, etc.) and numeric entities (&#123;, &#xABC;).
pub fn unescape_html_entities<'a>(s: &'a str) -> Cow<'a, str> {
    if s.is_empty() || !s.contains('&') {
        return Cow::Borrowed(s);
    }

    let bytes = s.as_bytes();
    let mut result = String::with_capacity(s.len());
    let mut pos = 0;

    while pos < bytes.len() {
        let remaining = &bytes[pos..];
        let amp_pos = match remaining.iter().position(|&b| b == b'&') {
            Some(p) => p,
            None => {
                // Copy the rest and we're done
                result.push_str(&s[pos..]);
                break;
            }
        };

        // Copy everything before the &
        if amp_pos > 0 {
            result.push_str(&s[pos..pos + amp_pos]);
        }
        pos += amp_pos;

        // Try to parse an entity starting at &
        if let Some(semi_offset) = s[pos..].find(';') {
            let entity_content = &s[pos + 1..pos + semi_offset];
            let entity_end = pos + semi_offset + 1;

            let replacement = if entity_content.starts_with('#') {
                parse_numeric_entity(entity_content)
            } else {
                match entity_content {
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "amp" => Some('&'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => None,
                }
            };

            if let Some(replacement_char) = replacement {
                result.push(replacement_char);
                pos = entity_end;
                continue;
            }
        }

        // Not a valid entity, keep the &
        result.push('&');
        pos += 1;
    }

    Cow::Owned(result)
}

/// Parse a numeric entity like "#123" or "#xABCD" (without the & and ;).
/// Returns the replacement character for invalid entities per HTML spec.
fn parse_numeric_entity(content: &str) -> Option<char> {
    if !content.starts_with('#') || content.len() < 2 {
        return None;
    }

    let num_str = &content[1..];
    let num = if num_str.starts_with('x') || num_str.starts_with('X') {
        if num_str.len() < 2 {
            return None;
        }
        // Try to parse, return replacement char if invalid hex
        match u32::from_str_radix(&num_str[1..], 16) {
            Ok(n) => n,
            Err(_) => return None, // Invalid hex like &#xg;
        }
    } else {
        match num_str.parse::<u32>() {
            Ok(n) => n,
            Err(_) => return None, // Invalid decimal
        }
    };

    // Handle invalid character references as per HTML spec:
    // - Code point 0 or > 0x10FFFF or surrogate range -> replacement char
    if num == 0 || num > 0x10FFFF || (0xD800..=0xDFFF).contains(&num) {
        return Some('\u{FFFD}');
    }

    char::from_u32(num).or(Some('\u{FFFD}'))
}

/// Extract JSON-LD metadata from the document.
pub fn get_json_ld(doc: &Document, article_title: &str, selectors: &Selectors) -> Metadata {
    let mut metadata = Metadata::default();

    let scripts = doc.select_matcher(&selectors.json_ld_script);

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
        let parsed = if let Value::Array(arr) = parsed {
            arr.into_iter().find(|it| {
                if let Some(type_val) = it.get("@type").and_then(|t| t.as_str()) {
                    regexps::JSON_LD_ARTICLE_TYPES.is_match(type_val)
                } else {
                    false
                }
            })
        } else {
            Some(parsed)
        };

        let parsed = match parsed {
            Some(p) => p,
            None => continue,
        };

        // Verify schema.org context
        static SCHEMA_ORG: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"^https?://schema\.org/?$").unwrap());

        let context = parsed.get("@context");
        let is_schema_org = match context {
            Some(Value::String(s)) => SCHEMA_ORG.is_match(s),
            Some(Value::Object(obj)) => {
                if let Some(Value::String(vocab)) = obj.get("@vocab") {
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

        // Handle @graph structure - destructure to take ownership and avoid cloning
        let parsed = if parsed.get("@type").is_none() {
            if let Value::Object(mut map) = parsed {
                if let Some(Value::Array(graph)) = map.remove("@graph") {
                    graph.into_iter().find(|it| {
                        if let Some(type_val) = it.get("@type").and_then(|t| t.as_str()) {
                            regexps::JSON_LD_ARTICLE_TYPES.is_match(type_val)
                        } else {
                            false
                        }
                    })
                } else {
                    None
                }
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
                // Both exist and differ - check which one matches the HTML title better.
                // Some sites put their site name in "name" and the article title in "headline"
                // (e.g., aktualne.cz), while others like Wikipedia put the article title in
                // "name" and a description in "headline".
                let name_matches = text_similarity(n, article_title) > 0.75;
                let headline_matches = text_similarity(h, article_title) > 0.75;

                if headline_matches && !name_matches {
                    Some(h.trim().to_string())
                } else {
                    Some(n.trim().to_string())
                }
            }
            (Some(n), _) => Some(n.trim().to_string()),
            (_, Some(h)) => Some(h.trim().to_string()),
            _ => None,
        };

        // Extract author/byline
        if let Some(author) = parsed.get("author") {
            if let Some(author_name) = author.get("name").and_then(|v| v.as_str()) {
                let trimmed = author_name.trim();
                if !trimmed.is_empty() {
                    metadata.byline = Some(trimmed.to_string());
                }
            } else if let Value::Array(authors) = author {
                let mut byline = String::new();
                for a in authors {
                    if let Some(name) = a.get("name").and_then(|v| v.as_str()) {
                        let trimmed = name.trim();
                        if !trimmed.is_empty() {
                            if !byline.is_empty() {
                                byline.push_str(", ");
                            }
                            byline.push_str(trimmed);
                        }
                    }
                }
                if !byline.is_empty() {
                    metadata.byline = Some(byline);
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
pub fn get_article_title(doc: &Document, selectors: &Selectors) -> String {
    let title_elem = doc.select_matcher(&selectors.title);
    let orig_title = title_elem.text().trim().to_string();

    if orig_title.is_empty() {
        return String::new();
    }

    let mut title_had_hierarchical_separators = false;

    fn word_count(s: &str) -> usize {
        s.split_whitespace().count()
    }

    // Use Cow to avoid cloning orig_title in the common case where cur_title
    // is just a slice of orig_title.
    let mut cur_title: Cow<str> = Cow::Borrowed(&orig_title);
    let orig_title_len = orig_title.chars().count();

    if regexps::TITLE_SEPARATOR.is_match(&orig_title) {
        // Check for hierarchical separators
        title_had_hierarchical_separators = regexps::TITLE_HIERARCHICAL.is_match(&orig_title);

        // Find all separators and split at the last one
        if let Some(last_match) = regexps::TITLE_SEPARATOR.find_iter(&orig_title).last() {
            cur_title = Cow::Borrowed(&orig_title[..last_match.start()]);
        }

        // If the resulting title is too short, remove the first part instead
        if word_count(&cur_title) < 3 {
            cur_title = regexps::TITLE_FIRST_PART.replace(&orig_title, "");
        }
    } else if orig_title.contains(": ") {
        // Check if we have a heading containing this exact string
        let headings = doc.select("h1, h2");
        let trimmed_title = orig_title.trim();
        let has_match = headings.iter().any(|h| h.text().trim() == trimmed_title);

        if !has_match {
            // Extract title after the last colon
            if let Some(pos) = orig_title.rfind(": ") {
                cur_title = Cow::Borrowed(&orig_title[pos + 2..]);

                // If too short, try first colon
                if word_count(&cur_title) < 3
                    && let Some(pos) = orig_title.find(": ")
                {
                    let before_colon = &orig_title[..pos];
                    if word_count(before_colon) <= 5 {
                        cur_title = Cow::Borrowed(&orig_title[pos + 2..]);
                    } else {
                        cur_title = Cow::Borrowed(&orig_title);
                    }
                }
            }
        }
    } else if !(15..=150).contains(&orig_title_len) {
        // Title too long or short, try H1
        let h1s = doc.select("h1");
        if h1s.length() == 1
            && let Some(h1) = h1s.nodes().first()
        {
            cur_title = Cow::Owned(get_inner_text(h1, true));
        }
    }

    // Normalize whitespace
    let mut cur_title = regexps::NORMALIZE
        .replace_all(cur_title.trim(), " ")
        .into_owned();

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
pub fn get_article_metadata(
    doc: &Document,
    json_ld: &Metadata,
    article_title: &str,
    selectors: &Selectors,
) -> Metadata {
    let mut metadata = Metadata::default();

    // Property pattern: article:author, og:title, etc.
    static PROPERTY_PATTERN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\s*(article|dc|dcterm|og|twitter)\s*:\s*(author|creator|description|published_time|title|site_name)\s*").unwrap()
    });

    // Name pattern for meta name attributes
    static NAME_PATTERN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^\s*(?:(dc|dcterm|og|twitter|parsely|weibo:(article|webpage))\s*[-\.:]?\s*)?(author|creator|pub-date|description|title|site_name)\s*$").unwrap()
    });

    let mut values: hashbrown::HashMap<String, String> = hashbrown::HashMap::new();

    let metas = doc.select_matcher(&selectors.meta);
    for meta in metas.iter() {
        let content = match meta.attr("content") {
            Some(c) if !c.is_empty() => c,
            _ => continue,
        };

        // Check property attribute
        if let Some(property) = meta.attr("property")
            && let Some(caps) = PROPERTY_PATTERN.captures(property.as_ref())
        {
            let name: String = caps
                .get(0)
                .unwrap()
                .as_str()
                .chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(|c| c.to_lowercase())
                .collect();
            values.insert(name, content.trim().to_string());
            continue;
        }

        // Check name attribute
        if let Some(name_attr) = meta.attr("name")
            && NAME_PATTERN.is_match(name_attr.as_ref())
        {
            let name: String = name_attr
                .as_ref()
                .chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(|c| {
                    let c = if c == '.' { ':' } else { c };
                    c.to_lowercase()
                })
                .collect();
            values.insert(name, content.trim().to_string());
        }
    }

    // Get title from various sources - use as_ref() to avoid cloning until we find the value
    metadata.title = json_ld
        .title
        .as_ref()
        .or_else(|| values.get("dc:title"))
        .or_else(|| values.get("dcterm:title"))
        .or_else(|| values.get("og:title"))
        .or_else(|| values.get("weibo:article:title"))
        .or_else(|| values.get("weibo:webpage:title"))
        .or_else(|| values.get("title"))
        .or_else(|| values.get("twitter:title"))
        .or_else(|| values.get("parsely-title"))
        .cloned();

    if metadata.title.is_none() && !article_title.is_empty() {
        metadata.title = Some(article_title.to_string());
    }

    // Get author/byline
    let article_author = values.get("article:author").filter(|v| !is_url(v));

    metadata.byline = json_ld
        .byline
        .as_ref()
        .or_else(|| values.get("dc:creator"))
        .or_else(|| values.get("dcterm:creator"))
        .or_else(|| values.get("author"))
        .or_else(|| values.get("parsely-author"))
        .or(article_author)
        .cloned();

    // Get excerpt/description
    metadata.excerpt = json_ld
        .excerpt
        .as_ref()
        .or_else(|| values.get("dc:description"))
        .or_else(|| values.get("dcterm:description"))
        .or_else(|| values.get("og:description"))
        .or_else(|| values.get("weibo:article:description"))
        .or_else(|| values.get("weibo:webpage:description"))
        .or_else(|| values.get("description"))
        .or_else(|| values.get("twitter:description"))
        .cloned();

    // Get site name
    metadata.site_name = json_ld
        .site_name
        .as_ref()
        .or_else(|| values.get("og:site_name"))
        .cloned();

    // Get published time
    metadata.published_time = json_ld
        .published_time
        .as_ref()
        .or_else(|| values.get("article:published_time"))
        .or_else(|| values.get("parsely-pub-date"))
        .cloned();

    // Unescape HTML entities in metadata, reusing the original string when no entities exist
    metadata.title = metadata.title.map(unescape_owned);
    metadata.byline = metadata.byline.map(unescape_owned);
    metadata.excerpt = metadata.excerpt.map(unescape_owned);
    metadata.site_name = metadata.site_name.map(unescape_owned);
    metadata.published_time = metadata.published_time.map(unescape_owned);

    metadata
}

/// Unescape HTML entities in an owned string, reusing the original allocation
/// when no entities are present (i.e., when `unescape_html_entities` returns `Cow::Borrowed`).
fn unescape_owned(s: String) -> String {
    match unescape_html_entities(&s) {
        Cow::Borrowed(_) => s,
        Cow::Owned(unescaped) => unescaped,
    }
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

    let tokens_a: HashSet<&str> = regexps::TOKENIZE
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

    let tokens_b_len: usize = tokens_b.iter().map(|s| s.chars().count()).sum::<usize>()
        + tokens_b.len().saturating_sub(1);

    // Compute unique_b stats in single pass without intermediate Vec
    let (unique_count, unique_len_sum): (usize, usize) = tokens_b
        .iter()
        .filter(|t| !tokens_a.contains(*t))
        .fold((0, 0), |(count, len), t| {
            (count + 1, len + t.chars().count())
        });
    let unique_b_len: usize = unique_len_sum + unique_count.saturating_sub(1);

    if tokens_b_len == 0 {
        return 0.0;
    }

    let distance_b = unique_b_len as f64 / tokens_b_len as f64;
    1.0 - distance_b
}
