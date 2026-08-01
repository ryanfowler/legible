//! Metadata extraction from HTML documents.
#![allow(clippy::collapsible_if, clippy::field_reassign_with_default)]
use crate::constants::regexps;
use crate::dom::{AttrName, Dom, Tag};
use crate::scoring::get_inner_text;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashSet;
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub title: Option<String>,
    pub byline: Option<String>,
    pub excerpt: Option<String>,
    pub site_name: Option<String>,
    pub published_time: Option<String>,
}
pub fn unescape_html_entities<'a>(s: &'a str) -> Cow<'a, str> {
    if s.is_empty() || !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let Some(a) = rest.find('&') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..a]);
        i += a;
        if let Some(semi) = s[i..].find(';') {
            let c = &s[i + 1..i + semi];
            let r = match c {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => parse_numeric(c),
            };
            if let Some(ch) = r {
                out.push(ch);
                i += semi + 1;
                continue;
            }
        }
        out.push('&');
        i += 1
    }
    Cow::Owned(out)
}
fn parse_numeric(s: &str) -> Option<char> {
    if !s.starts_with('#') {
        return None;
    }
    let n = if s[1..].starts_with(['x', 'X']) {
        u32::from_str_radix(&s[2..], 16).ok()?
    } else {
        s[1..].parse().ok()?
    };
    if n == 0 || n > 0x10ffff || (0xd800..=0xdfff).contains(&n) {
        Some('\u{fffd}')
    } else {
        char::from_u32(n).or(Some('\u{fffd}'))
    }
}
pub fn get_json_ld(dom: &Dom, title: &str) -> Metadata {
    let mut out = Metadata::default();
    let mut scripts = Vec::new();
    dom.collect_tag_attr_eq(
        dom.root(),
        Tag::Script,
        AttrName::Type,
        "application/ld+json",
        false,
        &mut scripts,
    );
    for id in scripts {
        let content = dom.text(id);
        let content = content
            .trim()
            .trim_start_matches("<![CDATA[")
            .trim_end_matches("]]>")
            .trim();
        let Ok(mut parsed) = serde_json::from_str::<Value>(content) else {
            continue;
        };
        if let Value::Array(a) = parsed {
            parsed = match a.into_iter().find(|v| {
                v.get("@type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| regexps::JSON_LD_ARTICLE_TYPES.is_match(t))
            }) {
                Some(v) => v,
                None => continue,
            }
        }
        let Some(obj) = parsed.as_object() else {
            continue;
        };
        let schema = match obj.get("@context") {
            Some(Value::String(s)) => SCHEMA.is_match(s),
            Some(Value::Object(o)) => o
                .get("@vocab")
                .and_then(Value::as_str)
                .is_some_and(|s| SCHEMA.is_match(s)),
            _ => false,
        };
        if !schema {
            continue;
        }
        let mut value = parsed;
        if value.get("@type").is_none() {
            if let Some(g) = value.get_mut("@graph").and_then(|v| v.as_array_mut()) {
                if let Some(v) = g.iter().find(|v| {
                    v.get("@type")
                        .and_then(Value::as_str)
                        .is_some_and(|t| regexps::JSON_LD_ARTICLE_TYPES.is_match(t))
                }) {
                    value = v.clone()
                } else {
                    continue;
                }
            }
        }
        let Some(o) = value.as_object() else { continue };
        if !o
            .get("@type")
            .and_then(Value::as_str)
            .is_some_and(|t| regexps::JSON_LD_ARTICLE_TYPES.is_match(t))
        {
            continue;
        }
        let name = o.get("name").and_then(Value::as_str);
        let headline = o.get("headline").and_then(Value::as_str);
        out.title = match (name, headline) {
            (Some(n), Some(h)) if n != h => {
                if text_similarity(h, title) > 0.75 && text_similarity(n, title) <= 0.75 {
                    Some(h.trim().into())
                } else {
                    Some(n.trim().into())
                }
            }
            (Some(n), _) => Some(n.trim().into()),
            (_, Some(h)) => Some(h.trim().into()),
            _ => None,
        };
        if let Some(a) = o.get("author") {
            if let Some(n) = a.get("name").and_then(Value::as_str) {
                if !n.trim().is_empty() {
                    out.byline = Some(n.trim().into())
                }
            } else if let Some(arr) = a.as_array() {
                let names: Vec<_> = arr
                    .iter()
                    .filter_map(|v| v.get("name").and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if !names.is_empty() {
                    out.byline = Some(names.join(", "))
                }
            }
        }
        out.excerpt = o
            .get("description")
            .and_then(Value::as_str)
            .map(|s| s.trim().into());
        out.site_name = o
            .get("publisher")
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
            .map(|s| s.trim().into());
        out.published_time = o
            .get("datePublished")
            .and_then(Value::as_str)
            .map(|s| s.trim().into());
        break;
    }
    out
}
static SCHEMA: Lazy<Regex> = Lazy::new(|| Regex::new(r"^https?://schema\.org/?$").unwrap());
pub fn get_article_title(dom: &Dom) -> String {
    let Some(id) = dom.first_descendant_by_tag(dom.root(), Tag::Title) else {
        return String::new();
    };
    let orig = get_inner_text(dom, id, false);
    if orig.is_empty() {
        return orig;
    }
    let mut cur = Cow::Borrowed(orig.as_str());
    let mut hierarchical = false;
    fn wc(s: &str) -> usize {
        s.split_whitespace().count()
    }
    if regexps::TITLE_SEPARATOR.is_match(&orig) {
        hierarchical = regexps::TITLE_HIERARCHICAL.is_match(&orig);
        if let Some(m) = regexps::TITLE_SEPARATOR.find_iter(&orig).last() {
            cur = Cow::Borrowed(&orig[..m.start()])
        }
        if wc(&cur) < 3 {
            cur = regexps::TITLE_FIRST_PART.replace(&orig, "")
        }
    } else if orig.contains(": ") {
        let has = dom
            .descendants(dom.root())
            .filter(|&x| matches!(dom.tag(x), Some(Tag::H1 | Tag::H2)))
            .any(|x| get_inner_text(dom, x, false).trim() == orig.trim());
        if !has {
            if let Some(p) = orig.rfind(": ") {
                cur = Cow::Borrowed(&orig[p + 2..]);
                if wc(&cur) < 3
                    && let Some(q) = orig.find(": ")
                {
                    cur = if wc(&orig[..q]) <= 5 {
                        Cow::Borrowed(&orig[q + 2..])
                    } else {
                        Cow::Borrowed(&orig)
                    }
                }
            }
        }
    } else if !(15..=150).contains(&orig.chars().count()) {
        let hs: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&x| dom.tag(x) == Some(Tag::H1))
            .collect();
        if hs.len() == 1 {
            cur = Cow::Owned(get_inner_text(dom, hs[0], true))
        }
    }
    let mut cur = regexps::NORMALIZE.replace_all(cur.trim(), " ").into_owned();
    if wc(&cur) <= 4 {
        let without = regexps::TITLE_SEPARATOR.replace_all(&orig, "");
        if !hierarchical || wc(&cur) != wc(&without).saturating_sub(1) {
            cur = orig
        }
    }
    cur
}
pub fn get_article_metadata(dom: &Dom, json: &Metadata, title: &str) -> Metadata {
    static PP: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\s*(article|dc|dcterm|og|twitter)\s*:\s*(author|creator|description|published_time|title|site_name)\s*").unwrap()
    });
    static NP: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^\s*(?:(dc|dcterm|og|twitter|parsely|weibo:(article|webpage))\s*[-\.:]?\s*)?(author|creator|pub-date|description|title|site_name)\s*$").unwrap()
    });
    let mut vals = std::collections::HashMap::new();
    for id in dom
        .descendants(dom.root())
        .filter(|&x| dom.tag(x) == Some(Tag::Meta))
    {
        let Some(c) = dom.attr(id, AttrName::Content).filter(|x| !x.is_empty()) else {
            continue;
        };
        if let Some(p) = dom
            .attr(id, AttrName::Property)
            .and_then(|p| PP.captures(p))
        {
            let n = p
                .get(0)
                .unwrap()
                .as_str()
                .chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(char::to_lowercase)
                .collect::<String>();
            vals.insert(n, c.trim().into());
        } else if let Some(n) = dom.attr(id, AttrName::Name).filter(|n| NP.is_match(n)) {
            let n = n
                .chars()
                .filter(|c| !c.is_whitespace())
                .map(|c| if c == '.' { ':' } else { c })
                .flat_map(char::to_lowercase)
                .collect::<String>();
            vals.insert(n, c.trim().into());
        }
    }
    let pick = |keys: &[&str]| keys.iter().find_map(|k| vals.get(*k).cloned());
    let mut m = Metadata::default();
    m.title = json
        .title
        .clone()
        .or_else(|| {
            pick(&[
                "dc:title",
                "dcterm:title",
                "og:title",
                "weibo:article:title",
                "weibo:webpage:title",
                "title",
                "twitter:title",
                "parsely-title",
            ])
        })
        .or_else(|| (!title.is_empty()).then(|| title.into()));
    let author = vals
        .get("article:author")
        .filter(|v| url::Url::parse(v).is_err())
        .cloned();
    m.byline = json
        .byline
        .clone()
        .or_else(|| pick(&["dc:creator", "dcterm:creator", "author", "parsely-author"]))
        .or(author);
    m.excerpt = json.excerpt.clone().or_else(|| {
        pick(&[
            "dc:description",
            "dcterm:description",
            "og:description",
            "weibo:article:description",
            "weibo:webpage:description",
            "description",
            "twitter:description",
        ])
    });
    m.site_name = json.site_name.clone().or_else(|| pick(&["og:site_name"]));
    m.published_time = json
        .published_time
        .clone()
        .or_else(|| pick(&["article:published_time", "parsely-pub-date"]));
    m.title = m.title.map(unescape_owned);
    m.byline = m.byline.map(unescape_owned);
    m.excerpt = m.excerpt.map(unescape_owned);
    m.site_name = m.site_name.map(unescape_owned);
    m.published_time = m.published_time.map(unescape_owned);
    m
}
fn unescape_owned(s: String) -> String {
    match unescape_html_entities(&s) {
        Cow::Borrowed(_) => s,
        Cow::Owned(x) => x,
    }
}
pub fn text_similarity(a: &str, b: &str) -> f64 {
    let aa = a.to_lowercase();
    let bb = b.to_lowercase();
    let set: HashSet<_> = regexps::TOKENIZE
        .split(&aa)
        .filter(|s| !s.is_empty())
        .collect();
    let tokens: Vec<_> = regexps::TOKENIZE
        .split(&bb)
        .filter(|s| !s.is_empty())
        .collect();
    if set.is_empty() || tokens.is_empty() {
        return 0.;
    }
    let total =
        tokens.iter().map(|s| s.chars().count()).sum::<usize>() + tokens.len().saturating_sub(1);
    let unique = tokens
        .iter()
        .filter(|s| !set.contains(*s))
        .map(|s| s.chars().count())
        .sum::<usize>()
        + tokens
            .iter()
            .filter(|s| !set.contains(*s))
            .count()
            .saturating_sub(1);
    1. - unique as f64 / total as f64
}
