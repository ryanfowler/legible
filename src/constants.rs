//! Constants and regex patterns used by Readability.

use crate::dom::{AttrName, Tag};
use regex::{Regex, RegexSet};
use std::sync::LazyLock;

/// Parsing flags that control the behavior of the algorithm.
pub mod flags {
    pub const FLAG_WEIGHT_CLASSES: u32 = 0x2;
    pub const FLAG_CLEAN_CONDITIONALLY: u32 = 0x4;
}

/// Default configuration values.
pub mod defaults {
    /// The default number of chars an article must have to return a result.
    pub const DEFAULT_CHAR_THRESHOLD: usize = 500;
}

#[inline]
pub fn is_default_tag_to_score(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::Section
            | Tag::H2
            | Tag::H3
            | Tag::H4
            | Tag::H5
            | Tag::H6
            | Tag::P
            | Tag::Td
            | Tag::Pre
    )
}

/// Regular expressions used throughout the parser.
pub mod regexps {
    use super::*;

    /// Matches unlikely candidates for main content.
    pub static UNLIKELY_CANDIDATES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)-ad-|ai2html|banner|breadcrumbs|combx|comment|community|cover-wrap|disqus|extra|footer|gdpr|header|legends|menu|related|remark|replies|rss|shoutbox|sidebar|skyscraper|social|sponsor|supplemental|ad-break|agegate|pagination|pager|popup|yom-remote").unwrap()
    });

    /// Matches positive indicators for content.
    pub static POSITIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)article|body|content|entry|hentry|h-entry|main|page|pagination|post|text|blog|story").unwrap()
    });

    /// Matches negative indicators for content.
    pub static NEGATIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)-ad-|hidden|^hid$| hid$| hid |^hid |banner|combx|comment|com-|contact|footer|gdpr|masthead|media|meta|outbrain|promo|related|scroll|share|shoutbox|sidebar|skyscraper|sponsor|shopping|tags|widget").unwrap()
    });

    /// Matches video hosting URLs.
    pub static VIDEOS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)//(www\.)?((dailymotion|youtube|youtube-nocookie|player\.vimeo|v\.qq|bilibili|live.bilibili)\.com|(archive|upload\.wikimedia)\.org|player\.twitch\.tv)").unwrap()
    });

    /// Matches share-related elements.
    pub static SHARE_ELEMENTS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(\b|_)(share|sharedaddy)(\b|_)").unwrap());

    /// Tokenizes text on word boundaries.
    pub static TOKENIZE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\W+").unwrap());

    /// Matches srcset URL patterns.
    pub static SRCSET_URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\S+)(\s+[\d.]+[xw])?(\s*(?:,|$))").unwrap());

    /// Matches ad-related words.
    pub static AD_WORDS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?iu)^(ad(vertising|vertisement)?|pub(licité)?|werb(ung)?|广告|Реклама|Anuncio)$",
        )
        .unwrap()
    });

    /// Matches loading indicator words.
    pub static LOADING_WORDS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?iu)^((loading|正在加载|Загрузка|chargement|cargando)(…|\.\.\.)?)$").unwrap()
    });

    /// RegexSet for class weight scoring - combines NEGATIVE (index 0) and POSITIVE (index 1).
    /// Allows single-pass matching instead of 4 separate regex calls.
    pub static CLASS_WEIGHT_SET: LazyLock<RegexSet> = LazyLock::new(|| {
        RegexSet::new([
            NEGATIVE.as_str(), // Index 0 - negative patterns
            POSITIVE.as_str(), // Index 1 - positive patterns
        ])
        .unwrap()
    });

    /// RegexSet for ad/loading word detection - combines AD_WORDS (index 0)
    /// and LOADING_WORDS (index 1) for single-pass matching.
    pub static AD_LOADING_SET: LazyLock<RegexSet> = LazyLock::new(|| {
        RegexSet::new([
            AD_WORDS.as_str(),      // Index 0 - ad-related words
            LOADING_WORDS.as_str(), // Index 1 - loading indicator words
        ])
        .unwrap()
    });
}

// ---------------------------------------------------------------------------
// Regex replacements: custom parsing functions replacing simple regex patterns.
// ---------------------------------------------------------------------------

/// Title separator characters: `|`, `-`, `\u{2013}` (en-dash), `\u{2014}` (em-dash),
/// `\`, `/`, `>`, `\u{00BB}` (right-pointing double angle).
#[inline]
pub fn is_title_separator_char(c: char) -> bool {
    matches!(
        c,
        '|' | '-' | '\u{2013}' | '\u{2014}' | '\\' | '/' | '>' | '\u{00BB}'
    )
}

/// Hierarchical title separator characters: `\`, `/`, `>`, `\u{00BB}`.
#[inline]
pub fn is_hierarchical_title_separator_char(c: char) -> bool {
    matches!(c, '\\' | '/' | '>' | '\u{00BB}')
}

/// Check if `s` contains a title separator character surrounded by whitespace.
/// Replaces `regexps::TITLE_SEPARATOR.is_match`.
pub fn has_title_separator(s: &str) -> bool {
    has_surrounded_separator(s, is_title_separator_char)
}

/// Check if `s` contains a hierarchical title separator surrounded by whitespace.
/// Replaces `regexps::TITLE_HIERARCHICAL.is_match`.
pub fn has_hierarchical_title_separator(s: &str) -> bool {
    has_surrounded_separator(s, is_hierarchical_title_separator_char)
}

fn has_surrounded_separator(s: &str, is_separator: fn(char) -> bool) -> bool {
    let mut chars = s.chars();
    let (Some(mut first), Some(mut second)) = (chars.next(), chars.next()) else {
        return false;
    };
    for third in chars {
        if first.is_whitespace() && is_separator(second) && third.is_whitespace() {
            return true;
        }
        first = second;
        second = third;
    }
    false
}

/// Find the byte position of the last title separator match (whitespace+sep+whitespace).
/// Returns the byte index of the leading whitespace.
/// Replaces `regexps::TITLE_SEPARATOR.find_iter(&orig).last().map(|m| m.start())`.
pub fn find_last_title_separator_start(s: &str) -> Option<usize> {
    let mut chars = s.char_indices();
    let mut last = None;
    while let Some((start, first)) = chars.next() {
        let mut following = chars.clone();
        if first.is_whitespace()
            && following
                .next()
                .is_some_and(|(_, second)| is_title_separator_char(second))
            && following
                .next()
                .is_some_and(|(_, third)| third.is_whitespace())
        {
            last = Some(start);
            // Regex matches do not overlap. Continue after the full match.
            chars = following;
        }
    }
    last
}

/// Remove all title separator triples (whitespace+sep+whitespace) from `s`.
/// Replaces `regexps::TITLE_SEPARATOR.replace_all(s, "")`.
pub fn remove_title_separators(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(first) = chars.next() {
        let mut following = chars.clone();
        if first.is_whitespace()
            && following.next().is_some_and(is_title_separator_char)
            && following.next().is_some_and(char::is_whitespace)
        {
            chars = following;
        } else {
            result.push(first);
        }
    }
    result
}

/// Remove everything from the start of `s` through (and including) the first
/// title separator character. If no separator is found, returns the input unchanged.
/// Replaces `regexps::TITLE_FIRST_PART.replace(s, "")`.
pub fn remove_title_first_part(s: &str) -> String {
    if let Some(pos) = s.find(is_title_separator_char) {
        let after = pos + s[pos..].chars().next().unwrap().len_utf8();
        s[after..].to_string()
    } else {
        s.to_string()
    }
}

/// Collapse runs of two or more whitespace characters into a single space.
/// Replaces `regexps::NORMALIZE.replace_all(s, " ").into_owned()`.
pub fn normalize_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if !c.is_whitespace() {
            result.push(c);
            continue;
        }
        if chars.peek().is_some_and(|next| next.is_whitespace()) {
            while chars.peek().is_some_and(|next| next.is_whitespace()) {
                chars.next();
            }
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}

/// Split a string into tokens on non-word characters (not
/// `[a-zA-Z0-9_]` / Unicode alphanumeric). Non-empty runs are yielded.
/// Replaces `regexps::TOKENIZE.split(s).filter(|s| !s.is_empty())`.
pub fn split_word_tokens(s: &str) -> impl Iterator<Item = &str> {
    regexps::TOKENIZE.split(s).filter(|token| !token.is_empty())
}

/// Check if `s` is a Schema.org URL (http or https, with optional trailing `/`).
/// Replaces the `SCHEMA` regex in metadata.rs.
#[inline]
pub fn is_schema_org_url(s: &str) -> bool {
    matches!(
        s,
        "https://schema.org" | "http://schema.org" | "https://schema.org/" | "http://schema.org/"
    )
}

/// Known JSON-LD article types from schema.org.
const JSON_LD_ARTICLE_TYPES: &[&str] = &[
    "APIReference",
    "AdvertiserContentArticle",
    "AnalysisNewsArticle",
    "Article",
    "AskPublicNewsArticle",
    "BackgroundNewsArticle",
    "BlogPosting",
    "DiscussionForumPosting",
    "LiveBlogPosting",
    "MedicalScholarlyArticle",
    "NewsArticle",
    "OpinionNewsArticle",
    "Report",
    "ReportageNewsArticle",
    "ReviewNewsArticle",
    "SatiricalArticle",
    "ScholarlyArticle",
    "SocialMediaPosting",
    "TechArticle",
];

/// Check if `s` is a known JSON-LD article type.
/// Replaces `regexps::JSON_LD_ARTICLE_TYPES.is_match`.
pub fn is_json_ld_article_type(s: &str) -> bool {
    let name = s
        .strip_prefix("https://schema.org/")
        .or_else(|| s.strip_prefix("http://schema.org/"))
        .or_else(|| {
            let (prefix, name) = s.split_once(':')?;
            (!prefix.is_empty() && !name.is_empty()).then_some(name)
        })
        .unwrap_or(s);
    JSON_LD_ARTICLE_TYPES.binary_search(&name).is_ok()
}

/// Check if `s` contains a byline keyword as a substring (case-insensitive ASCII).
/// The original regex `(?i)byline|author|dateline|writtenby|p-author` has no word
/// boundaries, so we match substrings anywhere.
pub fn has_byline(s: &str) -> bool {
    let b = s.as_bytes();
    let len = b.len();
    // byline (6)
    if len >= 6 {
        for i in 0..=len - 6 {
            if b[i..i + 6].eq_ignore_ascii_case(b"byline") {
                return true;
            }
        }
    }
    // author (6)
    if len >= 6 {
        for i in 0..=len - 6 {
            if b[i..i + 6].eq_ignore_ascii_case(b"author") {
                return true;
            }
        }
    }
    // dateline (8)
    if len >= 8 {
        for i in 0..=len - 8 {
            if b[i..i + 8].eq_ignore_ascii_case(b"dateline") {
                return true;
            }
        }
    }
    // writtenby (9)
    if len >= 9 {
        for i in 0..=len - 9 {
            if b[i..i + 9].eq_ignore_ascii_case(b"writtenby") {
                return true;
            }
        }
    }
    // p-author (8)
    if len >= 8 {
        for i in 0..=len - 8 {
            if b[i..i + 8].eq_ignore_ascii_case(b"p-author") {
                return true;
            }
        }
    }
    false
}

/// Parse a base64 data URL and return `(end_of_match_byte_index, media_type)`.
/// The end index points one past the final `,`.
/// Replaces `regexps::B64_DATA_URL.captures`.
pub fn parse_b64_data_url(s: &str) -> Option<(usize, &str)> {
    fn skip_whitespace(s: &str, pos: &mut usize) {
        while let Some(c) = s[*pos..].chars().next()
            && c.is_whitespace()
        {
            *pos += c.len_utf8();
        }
    }

    let b = s.as_bytes();
    let len = b.len();
    // 1. "data:" prefix (case-insensitive)
    if len < 5 || !b[..5].eq_ignore_ascii_case(b"data:") {
        return None;
    }
    let mut pos = 5;
    // 2. Skip whitespace
    skip_whitespace(s, &mut pos);
    // 3. Media type: one or more non-whitespace, non-';', non-',' chars
    let media_start = pos;
    while let Some(c) = s[pos..].chars().next()
        && !c.is_whitespace()
        && !matches!(c, ';' | ',')
    {
        pos += c.len_utf8();
    }
    if pos == media_start {
        return None;
    }
    let media_type = &s[media_start..pos];
    // 4. Skip whitespace
    skip_whitespace(s, &mut pos);
    // 5. ';'
    if pos >= len || b[pos] != b';' {
        return None;
    }
    pos += 1;
    // 6. Skip whitespace
    skip_whitespace(s, &mut pos);
    // 7. "base64" (case-insensitive)
    if pos + 6 > len || !b[pos..pos + 6].eq_ignore_ascii_case(b"base64") {
        return None;
    }
    pos += 6;
    // 8. Skip whitespace
    skip_whitespace(s, &mut pos);
    // 9. ','
    if pos >= len || b[pos] != b',' {
        return None;
    }
    pos += 1;
    Some((pos, media_type))
}

/// Check if `s` contains "share" or "sharedaddy" surrounded by word boundaries
/// or underscores (case-insensitive).
/// Replaces `regexps::SHARE_ELEMENTS.is_match`.
#[inline]
pub fn has_share_element(s: &str) -> bool {
    s.as_bytes()
        .windows(5)
        .any(|window| window.eq_ignore_ascii_case(b"share"))
        && regexps::SHARE_ELEMENTS.is_match(s)
}

#[inline]
pub fn is_unlikely_role(roles: &str) -> bool {
    roles.split_whitespace().any(|role| {
        [
            "menu",
            "menubar",
            "banner",
            "complementary",
            "navigation",
            "alert",
            "alertdialog",
            "dialog",
        ]
        .iter()
        .any(|unlikely| role.eq_ignore_ascii_case(unlikely))
    })
}

#[inline]
pub fn is_div_to_p_elem(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::Blockquote
            | Tag::Dl
            | Tag::Div
            | Tag::Img
            | Tag::Ol
            | Tag::P
            | Tag::Pre
            | Tag::Table
            | Tag::Ul
    )
}

#[inline]
pub fn is_alter_to_div_exception(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::Div | Tag::Article | Tag::Section | Tag::P | Tag::Ol | Tag::Ul
    )
}

/// Presentational attributes to remove.
pub static PRESENTATIONAL_ATTRIBUTES: &[AttrName] = &[
    AttrName::Align,
    AttrName::Background,
    AttrName::BgColor,
    AttrName::Border,
    AttrName::CellPadding,
    AttrName::CellSpacing,
    AttrName::Frame,
    AttrName::HSpace,
    AttrName::Rules,
    AttrName::Style,
    AttrName::VAlign,
    AttrName::VSpace,
];

#[inline]
pub fn is_deprecated_size_attribute_elem(tag: Tag) -> bool {
    matches!(tag, Tag::Table | Tag::Th | Tag::Td | Tag::Hr | Tag::Pre)
}

/// Check for phrasing content elements. CANVAS, IFRAME, SVG, VIDEO are excluded
/// as they tend to be removed.
#[inline]
pub fn is_phrasing_elem(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::Abbr
            | Tag::Audio
            | Tag::B
            | Tag::Bdo
            | Tag::Br
            | Tag::Button
            | Tag::Cite
            | Tag::Code
            | Tag::Data
            | Tag::Datalist
            | Tag::Dfn
            | Tag::Em
            | Tag::Embed
            | Tag::I
            | Tag::Img
            | Tag::Input
            | Tag::Kbd
            | Tag::Label
            | Tag::Mark
            | Tag::Math
            | Tag::Meter
            | Tag::Noscript
            | Tag::Object
            | Tag::Output
            | Tag::Progress
            | Tag::Q
            | Tag::Ruby
            | Tag::Samp
            | Tag::Script
            | Tag::Select
            | Tag::Small
            | Tag::Span
            | Tag::Strong
            | Tag::Sub
            | Tag::Sup
            | Tag::Textarea
            | Tag::Time
            | Tag::Var
            | Tag::Wbr
    )
}

/// Image extensions to check (without the dot, for suffix matching).
const IMAGE_EXTS: [&[u8]; 5] = [b"jpg", b"jpeg", b"png", b"webp", b"avif"];

/// Check if the bytes starting at `start` match an image extension (case-insensitive).
/// Returns the length of the matched extension, or None if no match.
#[inline]
fn match_image_ext(bytes: &[u8], start: usize) -> Option<usize> {
    for ext in IMAGE_EXTS {
        if start + ext.len() <= bytes.len()
            && bytes[start..start + ext.len()]
                .iter()
                .zip(ext.iter())
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Some(ext.len());
        }
    }
    None
}

/// Check if a string contains an image file extension (.jpg, .jpeg, .png, .webp, .avif).
#[inline]
pub fn has_image_extension(s: &str) -> bool {
    let bytes = s.as_bytes();
    // Find each '.' and check if an image extension follows
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'.' && match_image_ext(bytes, i + 1).is_some() {
            return true;
        }
    }
    false
}

/// Check if a string matches the srcset pattern: image extension followed by whitespace and digit.
#[inline]
pub fn has_image_srcset(s: &str) -> bool {
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'.'
            && let Some(ext_len) = match_image_ext(bytes, i + 1)
        {
            let after = i + 1 + ext_len;
            // Check for whitespace followed by digit
            if after < bytes.len()
                && bytes[after].is_ascii_whitespace()
                && let Some(pos) = bytes[after..]
                    .iter()
                    .position(|&c| !c.is_ascii_whitespace())
                && bytes[after + pos].is_ascii_digit()
            {
                return true;
            }
        }
    }
    false
}

/// Check if a string is a single image URL (matches IMAGE_SRC regex pattern).
/// Pattern: optional whitespace, non-whitespace chars ending with image extension, optional whitespace.
#[inline]
pub fn has_image_src(s: &str) -> bool {
    let trimmed = s.trim();
    // Must be non-empty and contain no internal whitespace
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) {
        return false;
    }
    let bytes = trimmed.as_bytes();
    // Find last '.' and check if it's followed by an image extension (possibly with ?/#)
    for (i, &b) in bytes.iter().enumerate().rev() {
        if b == b'.'
            && let Some(ext_len) = match_image_ext(bytes, i + 1)
        {
            let after = i + 1 + ext_len;
            // Valid if at end, or followed by ? or #
            if after >= bytes.len() || bytes[after] == b'?' || bytes[after] == b'#' {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- normalize_whitespace ----

    #[test]
    fn test_normalize_whitespace_no_change() {
        assert_eq!(normalize_whitespace("hello world"), "hello world");
        assert_eq!(normalize_whitespace("single"), "single");
        assert_eq!(normalize_whitespace(""), "");
    }

    #[test]
    fn test_normalize_whitespace_collapses() {
        assert_eq!(normalize_whitespace("hello   world"), "hello world");
        assert_eq!(normalize_whitespace("  hello  world  "), " hello world ");
        assert_eq!(normalize_whitespace("a  b  c"), "a b c");
    }

    #[test]
    fn test_normalize_whitespace_tabs_newlines() {
        assert_eq!(normalize_whitespace("hello\tworld"), "hello\tworld");
        assert_eq!(normalize_whitespace("hello\nworld"), "hello\nworld");
        assert_eq!(
            normalize_whitespace("hello\u{2003}world"),
            "hello\u{2003}world"
        );
        assert_eq!(normalize_whitespace("hello\t\tworld"), "hello world");
        assert_eq!(normalize_whitespace("a\n\nb\n\nc"), "a b c");
        assert_eq!(normalize_whitespace("a\r\n\r\nb"), "a b");
        assert_eq!(normalize_whitespace("a\u{2003}\u{2009}b"), "a b");
    }

    #[test]
    fn test_normalize_whitespace_mixed() {
        assert_eq!(normalize_whitespace("hello \t\n world"), "hello world");
    }

    // ---- split_word_tokens ----

    #[test]
    fn test_split_word_tokens_basic() {
        let v: Vec<&str> = split_word_tokens("hello world").collect();
        assert_eq!(v, vec!["hello", "world"]);
    }

    #[test]
    fn test_split_word_tokens_punctuation() {
        let v: Vec<&str> = split_word_tokens("hello, world!").collect();
        assert_eq!(v, vec!["hello", "world"]);
    }

    #[test]
    fn test_split_word_tokens_underscore() {
        let v: Vec<&str> = split_word_tokens("hello_world foo_bar").collect();
        assert_eq!(v, vec!["hello_world", "foo_bar"]);
    }

    #[test]
    fn test_split_word_tokens_empty() {
        let v: Vec<&str> = split_word_tokens("").collect();
        assert!(v.is_empty());
    }

    #[test]
    fn test_split_word_tokens_only_nonword() {
        let v: Vec<&str> = split_word_tokens("  !@#$ ").collect();
        assert!(v.is_empty());
    }

    #[test]
    fn test_split_word_tokens_unicode() {
        let v: Vec<&str> = split_word_tokens("cafe\u{0301} 東京").collect();
        assert_eq!(v, vec!["cafe\u{0301}", "東京"]);
    }

    // ---- title separators ----

    #[test]
    fn test_has_title_separator() {
        assert!(has_title_separator("hello - world"));
        assert!(has_title_separator("hello | world"));
        assert!(has_title_separator("hello / world"));
        assert!(has_title_separator("hello > world"));
        assert!(has_title_separator("hello \u{2013} world")); // en-dash
        assert!(has_title_separator("hello \u{2014} world")); // em-dash
        assert!(has_title_separator("hello \\ world"));
        assert!(has_title_separator("hello \u{00BB} world")); // right-pointing angle
        assert!(!has_title_separator("hello-world"));
        assert!(!has_title_separator("hello|world"));
        assert!(!has_title_separator(""));
        assert!(!has_title_separator("hello"));
    }

    #[test]
    fn test_has_hierarchical_title_separator() {
        assert!(has_hierarchical_title_separator("hello / world"));
        assert!(has_hierarchical_title_separator("hello > world"));
        assert!(has_hierarchical_title_separator("hello \\ world"));
        assert!(has_hierarchical_title_separator("hello \u{00BB} world"));
        assert!(!has_hierarchical_title_separator("hello - world"));
        assert!(!has_hierarchical_title_separator("hello | world"));
    }

    #[test]
    fn test_find_last_title_separator_start() {
        // "a - b | c" -> last separator " | " starts at byte 5
        assert_eq!(find_last_title_separator_start("a - b | c"), Some(5));
        // "a - b" -> only separator " - " starts at byte 1 (whitespace before '-')
        assert_eq!(find_last_title_separator_start("a - b"), Some(1));
        // Regex matches do not overlap, so only the first " - " matches.
        assert_eq!(find_last_title_separator_start("a - - b"), Some(1));
        assert_eq!(find_last_title_separator_start("no sep"), None);
    }

    #[test]
    fn test_remove_title_separators() {
        // Removing " - " and " | " from "a - b | c" leaves "abc"
        assert_eq!(remove_title_separators("a - b | c"), "abc");
        assert_eq!(remove_title_separators("a - b"), "ab");
        assert_eq!(remove_title_separators("no sep"), "no sep");
    }

    #[test]
    fn test_remove_title_first_part() {
        // First separator '-' at byte 5, remove up to and including '-' (6 bytes)
        assert_eq!(remove_title_first_part("hello - world"), " world");
        assert_eq!(remove_title_first_part("hello|world"), "world");
        assert_eq!(remove_title_first_part("hello / world"), " world");
        // "no-sep" has '-' which IS a title separator -> removes up to and including '-'
        // Use a string without any title separator char instead
        assert_eq!(remove_title_first_part("nosep"), "nosep");
        assert_eq!(remove_title_first_part(""), "");
    }

    // ---- is_schema_org_url ----

    #[test]
    fn test_is_schema_org_url() {
        assert!(is_schema_org_url("https://schema.org"));
        assert!(is_schema_org_url("http://schema.org"));
        assert!(is_schema_org_url("https://schema.org/"));
        assert!(is_schema_org_url("http://schema.org/"));
        assert!(!is_schema_org_url("https://schema.org/foo"));
        assert!(!is_schema_org_url("http://example.com"));
        assert!(!is_schema_org_url(""));
    }

    // ---- is_json_ld_article_type ----

    #[test]
    fn test_is_json_ld_article_type() {
        for article_type in JSON_LD_ARTICLE_TYPES {
            assert!(
                is_json_ld_article_type(article_type),
                "missing article type: {article_type}"
            );
        }
        assert!(is_json_ld_article_type("AdvertiserContentArticle"));
        assert!(is_json_ld_article_type("AnalysisNewsArticle"));
        assert!(is_json_ld_article_type("https://schema.org/NewsArticle"));
        assert!(is_json_ld_article_type("http://schema.org/BlogPosting"));
        assert!(is_json_ld_article_type("schema:NewsArticle"));
        assert!(is_json_ld_article_type("s:BlogPosting"));
        assert!(!is_json_ld_article_type("https://schema.org/WebPage"));
        assert!(!is_json_ld_article_type("WebPage"));
        assert!(!is_json_ld_article_type(""));
        assert!(!is_json_ld_article_type("NotAType"));
    }

    // ---- has_byline ----

    #[test]
    fn test_has_byline() {
        assert!(has_byline("byline"));
        assert!(has_byline("author"));
        assert!(has_byline("dateline"));
        assert!(has_byline("writtenby"));
        assert!(has_byline("p-author"));
        assert!(has_byline("Byline")); // case-insensitive
        assert!(has_byline("AUTHOR"));
        // Original regex matches substrings, so these now match
        assert!(has_byline("bylineextra"));
        assert!(has_byline("extraauthor"));
        assert!(!has_byline(""));
    }

    // ---- parse_b64_data_url ----

    #[test]
    fn test_parse_b64_data_url() {
        // Basic data URL: "data:image/png;base64," = 22 bytes before payload
        let r = parse_b64_data_url("data:image/png;base64,abc123");
        assert_eq!(r, Some((22, "image/png")));

        // With whitespace
        let r = parse_b64_data_url("data: image/png ; base64 ,abc123");
        // "data: " (6) + media "image/png" (9) = 15, then " " = 16, ";" = 17,
        // " " = 18, "base64" = 24, " " = 25, "," = 26, payload starts at 26
        assert_eq!(r, Some((26, "image/png")));

        // With Unicode whitespace, matching the regex `\s` behavior
        let input = "data:\u{2003}image/png\u{00a0};\u{2003}base64\u{00a0},abc123";
        let r = parse_b64_data_url(input);
        assert_eq!(r, Some((input.find(',').unwrap() + 1, "image/png")));

        // SVG (the one we check for exclusion)
        let r = parse_b64_data_url("data:image/svg+xml;base64,PHRlc3Q+PC90ZXN0Pg==");
        // "data:image/svg+xml;base64," = 26 bytes before payload
        assert_eq!(r, Some((26, "image/svg+xml")));

        // Invalid: no base64
        assert!(parse_b64_data_url("data:text/plain,hello").is_none());

        // Invalid: no data: prefix
        assert!(parse_b64_data_url("notadata").is_none());

        // Empty media type
        assert!(parse_b64_data_url("data:;base64,").is_none());
    }

    // ---- has_share_element ----

    #[test]
    fn test_has_share_element() {
        assert!(has_share_element("share"));
        assert!(has_share_element("sharedaddy"));
        assert!(has_share_element("_share_"));
        assert!(has_share_element("share_button"));
        assert!(has_share_element("some share text"));
        assert!(!has_share_element("shares")); // word char follows
        assert!(!has_share_element("noshare")); // word char precedes
        assert!(!has_share_element("éshare")); // Unicode word char precedes
        assert!(!has_share_element("share東京")); // Unicode word char follows
        assert!(!has_share_element(""));
        assert!(!has_share_element("unrelated"));
        // "share" inside "sharedaddy" should not double-match
        // (this only matters if we count matches; has_share_element returns bool so it's fine)
    }
}
