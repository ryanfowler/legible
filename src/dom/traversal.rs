//! DOM tree traversal utilities.

use dom_query::Node;
use std::borrow::Cow;

/// Get the tag name of a node in uppercase.
/// Uses Cow to avoid allocation for common HTML tags.
pub fn get_tag_name(node: &Node<'_>) -> Option<Cow<'static, str>> {
    node.node_name().map(|n| intern_tag_name(n.as_ref()))
}

/// Intern common HTML tag names to avoid repeated allocations.
/// Returns a static string reference for known tags, or allocates for unknown ones.
/// Lowercases the input into a stack buffer and matches against known tags.
#[inline]
fn intern_tag_name(name: &str) -> Cow<'static, str> {
    // Stack buffer for lowercased tag name (max HTML tag is 10 chars)
    let mut buf = [0u8; 16];
    let len = name.len();
    if len > buf.len() {
        return Cow::Owned(name.to_ascii_uppercase());
    }
    for (i, &b) in name.as_bytes().iter().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    let lower = &buf[..len];

    match lower {
        // 1-char tags
        b"a" => Cow::Borrowed("A"),
        b"b" => Cow::Borrowed("B"),
        b"i" => Cow::Borrowed("I"),
        b"p" => Cow::Borrowed("P"),
        b"q" => Cow::Borrowed("Q"),
        b"s" => Cow::Borrowed("S"),
        b"u" => Cow::Borrowed("U"),
        // 2-char tags
        b"br" => Cow::Borrowed("BR"),
        b"dd" => Cow::Borrowed("DD"),
        b"dl" => Cow::Borrowed("DL"),
        b"dt" => Cow::Borrowed("DT"),
        b"em" => Cow::Borrowed("EM"),
        b"h1" => Cow::Borrowed("H1"),
        b"h2" => Cow::Borrowed("H2"),
        b"h3" => Cow::Borrowed("H3"),
        b"h4" => Cow::Borrowed("H4"),
        b"h5" => Cow::Borrowed("H5"),
        b"h6" => Cow::Borrowed("H6"),
        b"hr" => Cow::Borrowed("HR"),
        b"li" => Cow::Borrowed("LI"),
        b"ol" => Cow::Borrowed("OL"),
        b"td" => Cow::Borrowed("TD"),
        b"th" => Cow::Borrowed("TH"),
        b"tr" => Cow::Borrowed("TR"),
        b"ul" => Cow::Borrowed("UL"),
        // 3-char tags
        b"bdi" => Cow::Borrowed("BDI"),
        b"bdo" => Cow::Borrowed("BDO"),
        b"col" => Cow::Borrowed("COL"),
        b"dfn" => Cow::Borrowed("DFN"),
        b"div" => Cow::Borrowed("DIV"),
        b"img" => Cow::Borrowed("IMG"),
        b"kbd" => Cow::Borrowed("KBD"),
        b"nav" => Cow::Borrowed("NAV"),
        b"pre" => Cow::Borrowed("PRE"),
        b"sub" => Cow::Borrowed("SUB"),
        b"sup" => Cow::Borrowed("SUP"),
        b"svg" => Cow::Borrowed("SVG"),
        b"var" => Cow::Borrowed("VAR"),
        b"wbr" => Cow::Borrowed("WBR"),
        // 4-char tags
        b"abbr" => Cow::Borrowed("ABBR"),
        b"area" => Cow::Borrowed("AREA"),
        b"base" => Cow::Borrowed("BASE"),
        b"body" => Cow::Borrowed("BODY"),
        b"cite" => Cow::Borrowed("CITE"),
        b"code" => Cow::Borrowed("CODE"),
        b"data" => Cow::Borrowed("DATA"),
        b"font" => Cow::Borrowed("FONT"),
        b"form" => Cow::Borrowed("FORM"),
        b"head" => Cow::Borrowed("HEAD"),
        b"html" => Cow::Borrowed("HTML"),
        b"link" => Cow::Borrowed("LINK"),
        b"main" => Cow::Borrowed("MAIN"),
        b"mark" => Cow::Borrowed("MARK"),
        b"meta" => Cow::Borrowed("META"),
        b"ruby" => Cow::Borrowed("RUBY"),
        b"samp" => Cow::Borrowed("SAMP"),
        b"slot" => Cow::Borrowed("SLOT"),
        b"span" => Cow::Borrowed("SPAN"),
        b"time" => Cow::Borrowed("TIME"),
        // 5-char tags
        b"aside" => Cow::Borrowed("ASIDE"),
        b"audio" => Cow::Borrowed("AUDIO"),
        b"embed" => Cow::Borrowed("EMBED"),
        b"input" => Cow::Borrowed("INPUT"),
        b"label" => Cow::Borrowed("LABEL"),
        b"meter" => Cow::Borrowed("METER"),
        b"small" => Cow::Borrowed("SMALL"),
        b"style" => Cow::Borrowed("STYLE"),
        b"table" => Cow::Borrowed("TABLE"),
        b"tbody" => Cow::Borrowed("TBODY"),
        b"tfoot" => Cow::Borrowed("TFOOT"),
        b"thead" => Cow::Borrowed("THEAD"),
        b"title" => Cow::Borrowed("TITLE"),
        b"video" => Cow::Borrowed("VIDEO"),
        // 6-char tags
        b"button" => Cow::Borrowed("BUTTON"),
        b"canvas" => Cow::Borrowed("CANVAS"),
        b"figure" => Cow::Borrowed("FIGURE"),
        b"footer" => Cow::Borrowed("FOOTER"),
        b"header" => Cow::Borrowed("HEADER"),
        b"iframe" => Cow::Borrowed("IFRAME"),
        b"object" => Cow::Borrowed("OBJECT"),
        b"option" => Cow::Borrowed("OPTION"),
        b"output" => Cow::Borrowed("OUTPUT"),
        b"script" => Cow::Borrowed("SCRIPT"),
        b"select" => Cow::Borrowed("SELECT"),
        b"source" => Cow::Borrowed("SOURCE"),
        b"strong" => Cow::Borrowed("STRONG"),
        // 7-char tags
        b"address" => Cow::Borrowed("ADDRESS"),
        b"article" => Cow::Borrowed("ARTICLE"),
        b"caption" => Cow::Borrowed("CAPTION"),
        b"details" => Cow::Borrowed("DETAILS"),
        b"picture" => Cow::Borrowed("PICTURE"),
        b"section" => Cow::Borrowed("SECTION"),
        b"summary" => Cow::Borrowed("SUMMARY"),
        // 8-char tags
        b"colgroup" => Cow::Borrowed("COLGROUP"),
        b"datalist" => Cow::Borrowed("DATALIST"),
        b"fieldset" => Cow::Borrowed("FIELDSET"),
        b"noscript" => Cow::Borrowed("NOSCRIPT"),
        b"optgroup" => Cow::Borrowed("OPTGROUP"),
        b"progress" => Cow::Borrowed("PROGRESS"),
        b"template" => Cow::Borrowed("TEMPLATE"),
        b"textarea" => Cow::Borrowed("TEXTAREA"),
        // 10-char tags
        b"blockquote" => Cow::Borrowed("BLOCKQUOTE"),
        b"figcaption" => Cow::Borrowed("FIGCAPTION"),
        _ => Cow::Owned(name.to_ascii_uppercase()),
    }
}
