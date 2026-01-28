//! Constants and regex patterns used by Readability.

use once_cell::sync::Lazy;
use regex::{Regex, RegexSet};
use std::collections::HashSet;

/// Parsing flags that control the behavior of the algorithm.
pub mod flags {
    pub const FLAG_STRIP_UNLIKELYS: u32 = 0x1;
    pub const FLAG_WEIGHT_CLASSES: u32 = 0x2;
    pub const FLAG_CLEAN_CONDITIONALLY: u32 = 0x4;
}

/// Default configuration values.
pub mod defaults {
    /// The default number of chars an article must have to return a result.
    pub const DEFAULT_CHAR_THRESHOLD: usize = 500;
}

/// Default tags to score.
pub static DEFAULT_TAGS_TO_SCORE: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    ["SECTION", "H2", "H3", "H4", "H5", "H6", "P", "TD", "PRE"]
        .iter()
        .copied()
        .collect()
});

/// Regular expressions used throughout the parser.
pub mod regexps {
    use super::*;

    /// Matches unlikely candidates for main content.
    pub static UNLIKELY_CANDIDATES: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)-ad-|ai2html|banner|breadcrumbs|combx|comment|community|cover-wrap|disqus|extra|footer|gdpr|header|legends|menu|related|remark|replies|rss|shoutbox|sidebar|skyscraper|social|sponsor|supplemental|ad-break|agegate|pagination|pager|popup|yom-remote").unwrap()
    });

    /// Matches elements that might be candidates even if they look unlikely.
    pub static OK_MAYBE_ITS_A_CANDIDATE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)and|article|body|column|content|main|mathjax|shadow").unwrap()
    });

    /// Matches positive indicators for content.
    pub static POSITIVE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)article|body|content|entry|hentry|h-entry|main|page|pagination|post|text|blog|story").unwrap()
    });

    /// Matches negative indicators for content.
    pub static NEGATIVE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)-ad-|hidden|^hid$| hid$| hid |^hid |banner|combx|comment|com-|contact|footer|gdpr|masthead|media|meta|outbrain|promo|related|scroll|share|shoutbox|sidebar|skyscraper|sponsor|shopping|tags|widget").unwrap()
    });

    /// Matches byline patterns.
    pub static BYLINE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)byline|author|dateline|writtenby|p-author").unwrap());

    /// Matches multiple whitespace characters.
    pub static NORMALIZE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s{2,}").unwrap());

    /// Matches video hosting URLs.
    pub static VIDEOS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)//(www\.)?((dailymotion|youtube|youtube-nocookie|player\.vimeo|v\.qq|bilibili|live.bilibili)\.com|(archive|upload\.wikimedia)\.org|player\.twitch\.tv)").unwrap()
    });

    /// Matches share-related elements.
    pub static SHARE_ELEMENTS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(\b|_)(share|sharedaddy)(\b|_)").unwrap());

    /// Tokenizes text on word boundaries.
    pub static TOKENIZE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\W+").unwrap());

    /// Matches whitespace-only strings.
    pub static WHITESPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*$").unwrap());

    /// Matches strings with content (non-whitespace at end).
    pub static HAS_CONTENT: Lazy<Regex> = Lazy::new(|| Regex::new(r"\S$").unwrap());

    /// Matches hash URLs.
    pub static HASH_URL: Lazy<Regex> = Lazy::new(|| Regex::new(r"^#.+").unwrap());

    /// Matches srcset URL patterns.
    pub static SRCSET_URL: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(\S+)(\s+[\d.]+[xw])?(\s*(?:,|$))").unwrap());

    /// Matches base64 data URLs.
    pub static B64_DATA_URL: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)^data:\s*([^\s;,]+)\s*;\s*base64\s*,").unwrap());

    /// Matches commas in various scripts.
    /// See: https://en.wikipedia.org/wiki/Comma#Comma_variants
    pub static COMMAS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"[\u002C\u060C\uFE50\uFE10\uFE11\u2E41\u2E34\u2E32\uFF0C]").unwrap()
    });

    /// Matches JSON-LD article types.
    /// See: https://schema.org/Article
    pub static JSON_LD_ARTICLE_TYPES: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^Article|AdvertiserContentArticle|NewsArticle|AnalysisNewsArticle|AskPublicNewsArticle|BackgroundNewsArticle|OpinionNewsArticle|ReportageNewsArticle|ReviewNewsArticle|Report|SatiricalArticle|ScholarlyArticle|MedicalScholarlyArticle|SocialMediaPosting|BlogPosting|LiveBlogPosting|DiscussionForumPosting|TechArticle|APIReference$").unwrap()
    });

    /// Matches ad-related words.
    pub static AD_WORDS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?iu)^(ad(vertising|vertisement)?|pub(licité)?|werb(ung)?|广告|Реклама|Anuncio)$",
        )
        .unwrap()
    });

    /// Matches loading indicator words.
    pub static LOADING_WORDS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?iu)^((loading|正在加载|Загрузка|chargement|cargando)(…|\.\.\.)?)$").unwrap()
    });

    /// Matches sentence-ending periods.
    pub static SENTENCE_END: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.( |$)").unwrap());

    /// Matches image file extensions in strings.
    pub static IMAGE_EXTENSION: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\.(jpg|jpeg|png|webp)").unwrap());

    /// Matches image srcset patterns.
    pub static IMAGE_SRCSET: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\.(jpg|jpeg|png|webp)\s+\d").unwrap());

    /// Matches single image URL patterns.
    pub static IMAGE_SRC: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)^\s*\S+\.(jpg|jpeg|png|webp)\S*\s*$").unwrap());

    /// Matches title separators surrounded by whitespace.
    pub static TITLE_SEPARATOR: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\s[\|\-–—\\\/>»]\s").unwrap());

    /// Matches hierarchical title separators (/, >, »).
    pub static TITLE_HIERARCHICAL: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\s[\\/>\u{00BB}]\s").unwrap());

    /// Matches the first part of a title up to and including a separator.
    pub static TITLE_FIRST_PART: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^[^\|\-–—\\\/>»]*[\|\-–—\\\/>»]").unwrap());

    /// RegexSet for class weight scoring - combines NEGATIVE (index 0) and POSITIVE (index 1).
    /// Allows single-pass matching instead of 4 separate regex calls.
    pub static CLASS_WEIGHT_SET: Lazy<RegexSet> = Lazy::new(|| {
        RegexSet::new([
            NEGATIVE.as_str(), // Index 0 - negative patterns
            POSITIVE.as_str(), // Index 1 - positive patterns
        ])
        .unwrap()
    });

    /// RegexSet for candidate filtering - combines UNLIKELY_CANDIDATES (index 0)
    /// and OK_MAYBE_ITS_A_CANDIDATE (index 1).
    pub static CANDIDATE_FILTER_SET: Lazy<RegexSet> = Lazy::new(|| {
        RegexSet::new([
            UNLIKELY_CANDIDATES.as_str(),      // Index 0 - unlikely patterns
            OK_MAYBE_ITS_A_CANDIDATE.as_str(), // Index 1 - maybe ok patterns
        ])
        .unwrap()
    });
}

/// Roles that indicate unlikely content areas.
pub static UNLIKELY_ROLES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "menu",
        "menubar",
        "complementary",
        "navigation",
        "alert",
        "alertdialog",
        "dialog",
    ]
    .iter()
    .copied()
    .collect()
});

/// Block-level elements that cause DIV to P conversion.
pub static DIV_TO_P_ELEMS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "BLOCKQUOTE",
        "DL",
        "DIV",
        "IMG",
        "OL",
        "P",
        "PRE",
        "TABLE",
        "UL",
    ]
    .iter()
    .copied()
    .collect()
});

/// Elements that should not be converted to DIV during sibling joining.
pub static ALTER_TO_DIV_EXCEPTIONS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    ["DIV", "ARTICLE", "SECTION", "P", "OL", "UL"]
        .iter()
        .copied()
        .collect()
});

/// Presentational attributes to remove.
pub static PRESENTATIONAL_ATTRIBUTES: Lazy<Vec<&'static str>> = Lazy::new(|| {
    vec![
        "align",
        "background",
        "bgcolor",
        "border",
        "cellpadding",
        "cellspacing",
        "frame",
        "hspace",
        "rules",
        "style",
        "valign",
        "vspace",
    ]
});

/// Elements with deprecated size attributes to remove.
pub static DEPRECATED_SIZE_ATTRIBUTE_ELEMS: Lazy<HashSet<&'static str>> =
    Lazy::new(|| ["TABLE", "TH", "TD", "HR", "PRE"].iter().copied().collect());

/// Phrasing content elements.
/// Note: CANVAS, IFRAME, SVG, VIDEO are excluded as they tend to be removed.
pub static PHRASING_ELEMS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "ABBR", "AUDIO", "B", "BDO", "BR", "BUTTON", "CITE", "CODE", "DATA", "DATALIST", "DFN",
        "EM", "EMBED", "I", "IMG", "INPUT", "KBD", "LABEL", "MARK", "MATH", "METER", "NOSCRIPT",
        "OBJECT", "OUTPUT", "PROGRESS", "Q", "RUBY", "SAMP", "SCRIPT", "SELECT", "SMALL", "SPAN",
        "STRONG", "SUB", "SUP", "TEXTAREA", "TIME", "VAR", "WBR",
    ]
    .iter()
    .copied()
    .collect()
});

/// HTML entity map for unescaping.
pub static HTML_ESCAPE_MAP: Lazy<std::collections::HashMap<&'static str, &'static str>> =
    Lazy::new(|| {
        let mut map = std::collections::HashMap::new();
        map.insert("lt", "<");
        map.insert("gt", ">");
        map.insert("amp", "&");
        map.insert("quot", "\"");
        map.insert("apos", "'");
        map
    });
