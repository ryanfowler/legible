//! Explicit article result types and rendering options.
use url::Url;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArticleMetadata {
    title: Option<String>,
    byline: Option<String>,
    authors: Vec<Author>,
    excerpt: Option<String>,
    site_name: Option<String>,
    publisher: Option<String>,
    canonical_url: Option<Url>,
    lead_image: Option<ImageMetadata>,
    published_time: Option<String>,
    modified_time: Option<String>,
    section: Option<String>,
    tags: Vec<String>,
    language: Option<String>,
    direction: Option<TextDirection>,
}
impl ArticleMetadata {
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    pub fn byline(&self) -> Option<&str> {
        self.byline.as_deref()
    }
    pub fn authors(&self) -> &[Author] {
        &self.authors
    }
    pub fn excerpt(&self) -> Option<&str> {
        self.excerpt.as_deref()
    }
    pub fn site_name(&self) -> Option<&str> {
        self.site_name.as_deref()
    }
    pub fn publisher(&self) -> Option<&str> {
        self.publisher.as_deref()
    }
    pub fn canonical_url(&self) -> Option<&Url> {
        self.canonical_url.as_ref()
    }
    pub fn lead_image(&self) -> Option<&ImageMetadata> {
        self.lead_image.as_ref()
    }
    pub fn published_time(&self) -> Option<&str> {
        self.published_time.as_deref()
    }
    pub fn modified_time(&self) -> Option<&str> {
        self.modified_time.as_deref()
    }
    pub fn section(&self) -> Option<&str> {
        self.section.as_deref()
    }
    pub fn tags(&self) -> &[String] {
        &self.tags
    }
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }
    pub fn direction(&self) -> Option<TextDirection> {
        self.direction
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Author {
    name: String,
    url: Option<Url>,
}
impl Author {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn url(&self) -> Option<&Url> {
        self.url.as_ref()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageMetadata {
    url: Url,
    alt: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
}
impl ImageMetadata {
    pub fn url(&self) -> &Url {
        &self.url
    }
    pub fn alt(&self) -> Option<&str> {
        self.alt.as_deref()
    }
    pub fn width(&self) -> Option<u32> {
        self.width
    }
    pub fn height(&self) -> Option<u32> {
        self.height
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TextDirection {
    LeftToRight,
    RightToLeft,
    Auto,
}

macro_rules! article_result {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug)]
        pub struct $name {
            pub(crate) metadata: ArticleMetadata,
            pub(crate) content: String,
            pub(crate) text_char_count: usize,
        }
        impl $name {
            /// Returns the extracted metadata.
            pub fn metadata(&self) -> &ArticleMetadata {
                &self.metadata
            }
            /// Returns the rendered article content.
            pub fn content(&self) -> &str {
                &self.content
            }
            /// Consumes the result and returns the rendered article content.
            pub fn into_content(self) -> String {
                self.content
            }
            /// Returns the normalized source text character count.
            pub fn text_char_count(&self) -> usize {
                self.text_char_count
            }
        }
    };
}

article_result!(
    HtmlArticle,
    "An extracted article rendered as an HTML fragment. The HTML is not sanitized."
);
article_result!(
    MarkdownArticle,
    "An extracted article rendered as CommonMark Markdown."
);
article_result!(
    TextArticle,
    "An extracted article rendered as normalized text."
);
#[derive(Clone, Debug)]
pub struct MarkdownOptions {
    heading_style: HeadingStyle,
    bullet_marker: BulletMarker,
    include_links: bool,
    include_images: bool,
}
impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            heading_style: HeadingStyle::Atx,
            bullet_marker: BulletMarker::Dash,
            include_links: true,
            include_images: true,
        }
    }
}
impl MarkdownOptions {
    pub fn heading_style(mut self, value: HeadingStyle) -> Self {
        self.heading_style = value;
        self
    }
    pub fn bullet_marker(mut self, value: BulletMarker) -> Self {
        self.bullet_marker = value;
        self
    }
    pub fn images(mut self, value: bool) -> Self {
        self.include_images = value;
        self
    }
    pub fn links(mut self, value: bool) -> Self {
        self.include_links = value;
        self
    }
    pub(crate) fn include_links(&self) -> bool {
        self.include_links
    }
    pub(crate) fn include_images(&self) -> bool {
        self.include_images
    }
    pub(crate) fn apply(&self, mut text: String) -> String {
        if matches!(self.heading_style, HeadingStyle::Setext) {
            text = map_markdown_outside_fences(&text, |line| {
                if let Some(title) = line.strip_prefix("# ") {
                    format!("{title}\n{}", "=".repeat(title.chars().count().max(1)))
                } else if let Some(title) = line.strip_prefix("## ") {
                    format!("{title}\n{}", "-".repeat(title.chars().count().max(1)))
                } else {
                    line.into()
                }
            });
        }
        let marker = match self.bullet_marker {
            BulletMarker::Dash => '-',
            BulletMarker::Asterisk => '*',
            BulletMarker::Plus => '+',
        };
        if marker != '-' {
            text = map_markdown_outside_fences(&text, |line| {
                let content = line.trim_start();
                let indent = &line[..line.len() - content.len()];
                if let Some(rest) = content.strip_prefix("- ") {
                    format!("{indent}{marker} {rest}")
                } else {
                    line.into()
                }
            });
        }
        text
    }
}
#[derive(Clone, Copy, Debug)]
pub enum HeadingStyle {
    Atx,
    Setext,
}
#[derive(Clone, Copy, Debug)]
pub enum BulletMarker {
    Dash,
    Asterisk,
    Plus,
}
#[derive(Clone, Debug)]
pub struct TextOptions {
    block_separator: TextSeparator,
    preserve_line_breaks: bool,
}
impl Default for TextOptions {
    fn default() -> Self {
        Self {
            block_separator: TextSeparator::Space,
            preserve_line_breaks: false,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum TextSeparator {
    Space,
    Newline,
}
impl TextOptions {
    pub fn block_separator(mut self, value: TextSeparator) -> Self {
        self.block_separator = value;
        self
    }
    pub fn preserve_line_breaks(mut self, value: bool) -> Self {
        self.preserve_line_breaks = value;
        self
    }
    pub(crate) fn block_newlines(&self) -> bool {
        matches!(self.block_separator, TextSeparator::Newline)
    }
    pub(crate) fn preserves_line_breaks(&self) -> bool {
        self.preserve_line_breaks
    }
}
fn map_markdown_outside_fences(text: &str, mut map: impl FnMut(&str) -> String) -> String {
    let mut fence: Option<(char, usize)> = None;
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if let Some((marker, length)) = fence {
                if trimmed.chars().take_while(|&c| c == marker).count() >= length {
                    fence = None;
                }
                line.into()
            } else {
                let marker = trimmed.chars().next();
                let length = marker.map_or(0, |m| trimmed.chars().take_while(|&c| c == m).count());
                if length >= 3 && matches!(marker, Some('`' | '~')) {
                    fence = Some((marker.unwrap(), length));
                    line.into()
                } else {
                    map(line)
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|v| v.eq_ignore_ascii_case(value)) {
        values.push(value.into())
    }
}

impl ArticleMetadata {
    pub(crate) fn merge_missing(&mut self, mut other: Self) {
        if self.title.is_none() {
            self.title = other.title.take()
        }
        if self.byline.is_none() {
            self.byline = other.byline.take()
        }
        if self.excerpt.is_none() {
            self.excerpt = other.excerpt.take()
        }
        if self.site_name.is_none() {
            self.site_name = other.site_name.take()
        }
        if self.publisher.is_none() {
            self.publisher = other.publisher.take()
        }
        if self.canonical_url.is_none() {
            self.canonical_url = other.canonical_url.take()
        }
        if self.lead_image.is_none() {
            self.lead_image = other.lead_image.take()
        }
        if self.published_time.is_none() {
            self.published_time = other.published_time.take()
        }
        self.modified_time = self.modified_time.take().or(other.modified_time);
        self.section = self.section.take().or(other.section);
        if self.authors.is_empty() {
            self.authors = other.authors
        }
        if self.tags.is_empty() {
            self.tags = other.tags
        }
    }
    pub(crate) fn merge_json(&mut self, mut json: Self) {
        if !json.authors.is_empty() {
            self.authors = std::mem::take(&mut json.authors)
        }
        self.merge_missing(json)
    }
    pub(crate) fn from_dom(dom: &crate::dom::Dom, base: Option<&Url>, sources: u8) -> Self {
        use crate::dom::{AttrName, Tag};
        let mut out = Self::default();
        let mut image_group_open = false;
        let mut pending_image_alt = None;
        let mut pending_image_width = None;
        let mut pending_image_height = None;
        let resolve = |s: &str| {
            Url::parse(s)
                .ok()
                .or_else(|| base.and_then(|b| b.join(s).ok()))
        };
        for id in dom.descendants(dom.root()) {
            match dom.tag(id) {
                Some(Tag::Meta) => {
                    let raw_key = dom
                        .attr(id, AttrName::Property)
                        .or_else(|| dom.attr(id, AttrName::Name))
                        .unwrap_or("");
                    let key = if raw_key.bytes().any(|byte| byte.is_ascii_uppercase()) {
                        std::borrow::Cow::Owned(raw_key.to_ascii_lowercase())
                    } else {
                        std::borrow::Cow::Borrowed(raw_key)
                    };
                    if !metadata_source_enabled(&key, sources) {
                        continue;
                    }
                    let Some(value) = dom
                        .attr(id, AttrName::Content)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    else {
                        continue;
                    };
                    match key.as_ref() {
                        "og:url" => out.canonical_url = resolve(value),
                        "og:image" | "twitter:image" => {
                            if out.lead_image.is_none() {
                                if let Some(url) = resolve(value) {
                                    out.lead_image = Some(ImageMetadata {
                                        url,
                                        alt: pending_image_alt.take(),
                                        width: pending_image_width.take(),
                                        height: pending_image_height.take(),
                                    });
                                    image_group_open = true
                                }
                            } else {
                                image_group_open = false;
                            }
                        }
                        "og:image:alt" | "twitter:image:alt" => {
                            if image_group_open {
                                if let Some(image) = &mut out.lead_image {
                                    image.alt = Some(value.into())
                                }
                            } else if out.lead_image.is_none() {
                                pending_image_alt = Some(value.into())
                            }
                        }
                        "og:image:width" => {
                            let width = value.parse().ok();
                            if image_group_open {
                                if let Some(image) = &mut out.lead_image {
                                    image.width = width
                                }
                            } else if out.lead_image.is_none() {
                                pending_image_width = width
                            }
                        }
                        "og:image:height" => {
                            let height = value.parse().ok();
                            if image_group_open {
                                if let Some(image) = &mut out.lead_image {
                                    image.height = height
                                }
                            } else if out.lead_image.is_none() {
                                pending_image_height = height
                            }
                        }
                        "article:modified_time" => out.modified_time = Some(value.into()),
                        "article:section" => out.section = Some(value.into()),
                        "article:tag" => push_unique(&mut out.tags, value),
                        "keywords" => {
                            for tag in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                                push_unique(&mut out.tags, tag)
                            }
                        }
                        "publisher" => out.publisher = Some(value.into()),
                        "author" | "dc:creator" | "twitter:creator" => {
                            let name = value.trim_start_matches('@');
                            if !out
                                .authors
                                .iter()
                                .any(|a| a.name.eq_ignore_ascii_case(name))
                            {
                                out.authors.push(Author {
                                    name: name.into(),
                                    url: None,
                                })
                            }
                        }
                        _ => {}
                    }
                }
                Some(Tag::Link)
                    if sources & 0b1000 != 0
                        && dom.attr(id, AttrName::Rel).is_some_and(|r| {
                            r.split_whitespace()
                                .any(|x| x.eq_ignore_ascii_case("canonical"))
                        }) =>
                {
                    if let Some(h) = dom.attr(id, AttrName::Href) {
                        out.canonical_url = resolve(h)
                    }
                }
                _ => {}
            }
        }
        out
    }
    pub(crate) fn from_json(metadata: &crate::metadata::Metadata, base: Option<&Url>) -> Self {
        let resolve = |value: &str| {
            Url::parse(value)
                .ok()
                .or_else(|| base.and_then(|base| base.join(value).ok()))
        };
        let authors = metadata
            .authors
            .iter()
            .map(|(name, url)| Author {
                name: name.clone(),
                url: url.as_deref().and_then(&resolve),
            })
            .collect();
        let lead_image = metadata.image.as_ref().and_then(|image| {
            resolve(&image.url).map(|url| ImageMetadata {
                url,
                alt: image.alt.clone(),
                width: image.width,
                height: image.height,
            })
        });
        Self {
            authors,
            publisher: metadata.publisher.clone(),
            canonical_url: metadata.canonical_url.as_deref().and_then(resolve),
            lead_image,
            modified_time: metadata.modified_time.clone(),
            section: metadata.section.clone(),
            tags: metadata.tags.clone(),
            ..Default::default()
        }
    }
    pub(crate) fn from_parts(
        title: String,
        byline: Option<String>,
        direction: Option<String>,
        language: Option<String>,
        excerpt: Option<String>,
        site_name: Option<String>,
        published_time: Option<String>,
    ) -> Self {
        let direction = match direction.as_deref() {
            Some("rtl") => Some(TextDirection::RightToLeft),
            Some("ltr") => Some(TextDirection::LeftToRight),
            Some("auto") => Some(TextDirection::Auto),
            _ => None,
        };
        let authors = byline
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| {
                vec![Author {
                    name: name.into(),
                    url: None,
                }]
            })
            .unwrap_or_default();
        Self {
            title: (!title.is_empty()).then_some(title),
            byline,
            authors,
            excerpt,
            site_name,
            published_time,
            language,
            direction,
            ..Default::default()
        }
    }
}

fn metadata_source_enabled(name: &str, sources: u8) -> bool {
    if name.starts_with("og:") || name.starts_with("article:") {
        sources & 0b0010 != 0
    } else if name.starts_with("twitter:") {
        sources & 0b0100 != 0
    } else {
        sources & 0b1000 != 0
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn result_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<super::HtmlArticle>();
        assert_send_sync::<super::MarkdownArticle>();
        assert_send_sync::<super::TextArticle>();
    }
}
