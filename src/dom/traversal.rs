//! DOM tree traversal utilities.

use dom_query::Node;
use std::borrow::Cow;

/// Check if a node has an ancestor with the given tag name.
///
/// # Arguments
/// * `node` - The starting node
/// * `tag_name` - The tag name to look for (case-insensitive)
/// * `max_depth` - Maximum depth to search (negative means unlimited)
/// * `filter` - Optional filter function that must return true for the ancestor to match
pub fn has_ancestor_tag<'a, F>(
    node: &Node<'a>,
    tag_name: &str,
    max_depth: i32,
    filter: Option<F>,
) -> bool
where
    F: Fn(&Node<'a>) -> bool,
{
    let mut depth = 0;
    let mut current = node.parent();

    while let Some(parent) = current {
        if max_depth > 0 && depth > max_depth {
            return false;
        }

        // Use case-insensitive comparison to avoid allocation
        if let Some(parent_tag) = parent.node_name()
            && parent_tag.eq_ignore_ascii_case(tag_name)
        {
            if let Some(ref f) = filter {
                if f(&parent) {
                    return true;
                }
            } else {
                return true;
            }
        }

        current = parent.parent();
        depth += 1;
    }

    false
}

/// Get the tag name of a node in uppercase.
/// Uses Cow to avoid allocation for common HTML tags.
pub fn get_tag_name(node: &Node<'_>) -> Option<Cow<'static, str>> {
    node.node_name().map(|n| intern_tag_name(n.as_ref()))
}

/// Intern common HTML tag names to avoid repeated allocations.
/// Returns a static string reference for known tags, or allocates for unknown ones.
#[inline]
fn intern_tag_name(name: &str) -> Cow<'static, str> {
    // Fast path: check for common tags using case-insensitive comparison
    // These are the most frequently encountered tags in readability processing
    match name.len() {
        1 => match_tag_1(name),
        2 => match_tag_2(name),
        3 => match_tag_3(name),
        4 => match_tag_4(name),
        5 => match_tag_5(name),
        6 => match_tag_6(name),
        7 => match_tag_7(name),
        8 => match_tag_8(name),
        10 => match_tag_10(name),
        _ => Cow::Owned(name.to_ascii_uppercase()),
    }
}

#[inline]
fn match_tag_1(name: &str) -> Cow<'static, str> {
    if name.eq_ignore_ascii_case("a") {
        Cow::Borrowed("A")
    } else if name.eq_ignore_ascii_case("b") {
        Cow::Borrowed("B")
    } else if name.eq_ignore_ascii_case("i") {
        Cow::Borrowed("I")
    } else if name.eq_ignore_ascii_case("p") {
        Cow::Borrowed("P")
    } else if name.eq_ignore_ascii_case("q") {
        Cow::Borrowed("Q")
    } else if name.eq_ignore_ascii_case("s") {
        Cow::Borrowed("S")
    } else if name.eq_ignore_ascii_case("u") {
        Cow::Borrowed("U")
    } else {
        Cow::Owned(name.to_ascii_uppercase())
    }
}

#[inline]
fn match_tag_2(name: &str) -> Cow<'static, str> {
    if name.eq_ignore_ascii_case("br") {
        Cow::Borrowed("BR")
    } else if name.eq_ignore_ascii_case("dd") {
        Cow::Borrowed("DD")
    } else if name.eq_ignore_ascii_case("dl") {
        Cow::Borrowed("DL")
    } else if name.eq_ignore_ascii_case("dt") {
        Cow::Borrowed("DT")
    } else if name.eq_ignore_ascii_case("em") {
        Cow::Borrowed("EM")
    } else if name.eq_ignore_ascii_case("h1") {
        Cow::Borrowed("H1")
    } else if name.eq_ignore_ascii_case("h2") {
        Cow::Borrowed("H2")
    } else if name.eq_ignore_ascii_case("h3") {
        Cow::Borrowed("H3")
    } else if name.eq_ignore_ascii_case("h4") {
        Cow::Borrowed("H4")
    } else if name.eq_ignore_ascii_case("h5") {
        Cow::Borrowed("H5")
    } else if name.eq_ignore_ascii_case("h6") {
        Cow::Borrowed("H6")
    } else if name.eq_ignore_ascii_case("hr") {
        Cow::Borrowed("HR")
    } else if name.eq_ignore_ascii_case("li") {
        Cow::Borrowed("LI")
    } else if name.eq_ignore_ascii_case("ol") {
        Cow::Borrowed("OL")
    } else if name.eq_ignore_ascii_case("td") {
        Cow::Borrowed("TD")
    } else if name.eq_ignore_ascii_case("th") {
        Cow::Borrowed("TH")
    } else if name.eq_ignore_ascii_case("tr") {
        Cow::Borrowed("TR")
    } else if name.eq_ignore_ascii_case("ul") {
        Cow::Borrowed("UL")
    } else {
        Cow::Owned(name.to_ascii_uppercase())
    }
}

#[inline]
fn match_tag_3(name: &str) -> Cow<'static, str> {
    if name.eq_ignore_ascii_case("div") {
        Cow::Borrowed("DIV")
    } else if name.eq_ignore_ascii_case("img") {
        Cow::Borrowed("IMG")
    } else if name.eq_ignore_ascii_case("pre") {
        Cow::Borrowed("PRE")
    } else if name.eq_ignore_ascii_case("svg") {
        Cow::Borrowed("SVG")
    } else if name.eq_ignore_ascii_case("col") {
        Cow::Borrowed("COL")
    } else if name.eq_ignore_ascii_case("nav") {
        Cow::Borrowed("NAV")
    } else if name.eq_ignore_ascii_case("sub") {
        Cow::Borrowed("SUB")
    } else if name.eq_ignore_ascii_case("sup") {
        Cow::Borrowed("SUP")
    } else if name.eq_ignore_ascii_case("wbr") {
        Cow::Borrowed("WBR")
    } else if name.eq_ignore_ascii_case("bdi") {
        Cow::Borrowed("BDI")
    } else if name.eq_ignore_ascii_case("bdo") {
        Cow::Borrowed("BDO")
    } else if name.eq_ignore_ascii_case("dfn") {
        Cow::Borrowed("DFN")
    } else if name.eq_ignore_ascii_case("kbd") {
        Cow::Borrowed("KBD")
    } else if name.eq_ignore_ascii_case("var") {
        Cow::Borrowed("VAR")
    } else {
        Cow::Owned(name.to_ascii_uppercase())
    }
}

#[inline]
fn match_tag_4(name: &str) -> Cow<'static, str> {
    if name.eq_ignore_ascii_case("body") {
        Cow::Borrowed("BODY")
    } else if name.eq_ignore_ascii_case("code") {
        Cow::Borrowed("CODE")
    } else if name.eq_ignore_ascii_case("form") {
        Cow::Borrowed("FORM")
    } else if name.eq_ignore_ascii_case("head") {
        Cow::Borrowed("HEAD")
    } else if name.eq_ignore_ascii_case("html") {
        Cow::Borrowed("HTML")
    } else if name.eq_ignore_ascii_case("link") {
        Cow::Borrowed("LINK")
    } else if name.eq_ignore_ascii_case("main") {
        Cow::Borrowed("MAIN")
    } else if name.eq_ignore_ascii_case("meta") {
        Cow::Borrowed("META")
    } else if name.eq_ignore_ascii_case("span") {
        Cow::Borrowed("SPAN")
    } else if name.eq_ignore_ascii_case("abbr") {
        Cow::Borrowed("ABBR")
    } else if name.eq_ignore_ascii_case("area") {
        Cow::Borrowed("AREA")
    } else if name.eq_ignore_ascii_case("base") {
        Cow::Borrowed("BASE")
    } else if name.eq_ignore_ascii_case("cite") {
        Cow::Borrowed("CITE")
    } else if name.eq_ignore_ascii_case("data") {
        Cow::Borrowed("DATA")
    } else if name.eq_ignore_ascii_case("font") {
        Cow::Borrowed("FONT")
    } else if name.eq_ignore_ascii_case("mark") {
        Cow::Borrowed("MARK")
    } else if name.eq_ignore_ascii_case("ruby") {
        Cow::Borrowed("RUBY")
    } else if name.eq_ignore_ascii_case("samp") {
        Cow::Borrowed("SAMP")
    } else if name.eq_ignore_ascii_case("slot") {
        Cow::Borrowed("SLOT")
    } else if name.eq_ignore_ascii_case("time") {
        Cow::Borrowed("TIME")
    } else {
        Cow::Owned(name.to_ascii_uppercase())
    }
}

#[inline]
fn match_tag_5(name: &str) -> Cow<'static, str> {
    if name.eq_ignore_ascii_case("aside") {
        Cow::Borrowed("ASIDE")
    } else if name.eq_ignore_ascii_case("embed") {
        Cow::Borrowed("EMBED")
    } else if name.eq_ignore_ascii_case("input") {
        Cow::Borrowed("INPUT")
    } else if name.eq_ignore_ascii_case("label") {
        Cow::Borrowed("LABEL")
    } else if name.eq_ignore_ascii_case("small") {
        Cow::Borrowed("SMALL")
    } else if name.eq_ignore_ascii_case("style") {
        Cow::Borrowed("STYLE")
    } else if name.eq_ignore_ascii_case("table") {
        Cow::Borrowed("TABLE")
    } else if name.eq_ignore_ascii_case("tbody") {
        Cow::Borrowed("TBODY")
    } else if name.eq_ignore_ascii_case("tfoot") {
        Cow::Borrowed("TFOOT")
    } else if name.eq_ignore_ascii_case("thead") {
        Cow::Borrowed("THEAD")
    } else if name.eq_ignore_ascii_case("title") {
        Cow::Borrowed("TITLE")
    } else if name.eq_ignore_ascii_case("video") {
        Cow::Borrowed("VIDEO")
    } else if name.eq_ignore_ascii_case("audio") {
        Cow::Borrowed("AUDIO")
    } else if name.eq_ignore_ascii_case("meter") {
        Cow::Borrowed("METER")
    } else {
        Cow::Owned(name.to_ascii_uppercase())
    }
}

#[inline]
fn match_tag_6(name: &str) -> Cow<'static, str> {
    if name.eq_ignore_ascii_case("button") {
        Cow::Borrowed("BUTTON")
    } else if name.eq_ignore_ascii_case("figure") {
        Cow::Borrowed("FIGURE")
    } else if name.eq_ignore_ascii_case("footer") {
        Cow::Borrowed("FOOTER")
    } else if name.eq_ignore_ascii_case("header") {
        Cow::Borrowed("HEADER")
    } else if name.eq_ignore_ascii_case("iframe") {
        Cow::Borrowed("IFRAME")
    } else if name.eq_ignore_ascii_case("object") {
        Cow::Borrowed("OBJECT")
    } else if name.eq_ignore_ascii_case("option") {
        Cow::Borrowed("OPTION")
    } else if name.eq_ignore_ascii_case("script") {
        Cow::Borrowed("SCRIPT")
    } else if name.eq_ignore_ascii_case("select") {
        Cow::Borrowed("SELECT")
    } else if name.eq_ignore_ascii_case("source") {
        Cow::Borrowed("SOURCE")
    } else if name.eq_ignore_ascii_case("strong") {
        Cow::Borrowed("STRONG")
    } else if name.eq_ignore_ascii_case("canvas") {
        Cow::Borrowed("CANVAS")
    } else if name.eq_ignore_ascii_case("output") {
        Cow::Borrowed("OUTPUT")
    } else {
        Cow::Owned(name.to_ascii_uppercase())
    }
}

#[inline]
fn match_tag_7(name: &str) -> Cow<'static, str> {
    if name.eq_ignore_ascii_case("address") {
        Cow::Borrowed("ADDRESS")
    } else if name.eq_ignore_ascii_case("article") {
        Cow::Borrowed("ARTICLE")
    } else if name.eq_ignore_ascii_case("caption") {
        Cow::Borrowed("CAPTION")
    } else if name.eq_ignore_ascii_case("picture") {
        Cow::Borrowed("PICTURE")
    } else if name.eq_ignore_ascii_case("section") {
        Cow::Borrowed("SECTION")
    } else if name.eq_ignore_ascii_case("details") {
        Cow::Borrowed("DETAILS")
    } else if name.eq_ignore_ascii_case("summary") {
        Cow::Borrowed("SUMMARY")
    } else {
        Cow::Owned(name.to_ascii_uppercase())
    }
}

#[inline]
fn match_tag_8(name: &str) -> Cow<'static, str> {
    if name.eq_ignore_ascii_case("colgroup") {
        Cow::Borrowed("COLGROUP")
    } else if name.eq_ignore_ascii_case("fieldset") {
        Cow::Borrowed("FIELDSET")
    } else if name.eq_ignore_ascii_case("noscript") {
        Cow::Borrowed("NOSCRIPT")
    } else if name.eq_ignore_ascii_case("optgroup") {
        Cow::Borrowed("OPTGROUP")
    } else if name.eq_ignore_ascii_case("datalist") {
        Cow::Borrowed("DATALIST")
    } else if name.eq_ignore_ascii_case("progress") {
        Cow::Borrowed("PROGRESS")
    } else if name.eq_ignore_ascii_case("template") {
        Cow::Borrowed("TEMPLATE")
    } else if name.eq_ignore_ascii_case("textarea") {
        Cow::Borrowed("TEXTAREA")
    } else {
        Cow::Owned(name.to_ascii_uppercase())
    }
}

#[inline]
fn match_tag_10(name: &str) -> Cow<'static, str> {
    if name.eq_ignore_ascii_case("blockquote") {
        Cow::Borrowed("BLOCKQUOTE")
    } else if name.eq_ignore_ascii_case("figcaption") {
        Cow::Borrowed("FIGCAPTION")
    } else {
        Cow::Owned(name.to_ascii_uppercase())
    }
}
