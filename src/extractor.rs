use crate::{
    article::{HtmlArticle, MarkdownArticle, MarkdownOptions, TextArticle, TextOptions},
    document::Document,
    error::{Error, Result},
    options::Options,
    readability::ExtractedArticle,
};
use url::Url;

/// Reusable extraction configuration.
///
/// Each extraction method renders one requested format from the cleaned DOM. Requesting
/// multiple public formats requires separate extraction calls.
#[derive(Clone, Debug, Default)]
pub struct Extractor {
    config: Options,
    max_input_bytes: Option<usize>,
}
impl Extractor {
    pub fn builder() -> ExtractorBuilder {
        ExtractorBuilder {
            extractor: Self::default(),
            embed_policy: None,
        }
    }

    /// Extracts an HTML fragment. The returned HTML is not sanitized.
    pub fn extract_html(&self, html: &str) -> Result<HtmlArticle> {
        render_html_article(self.extract_source_from_html(html, None)?)
    }
    pub fn extract_html_with_url(&self, html: &str, url: &Url) -> Result<HtmlArticle> {
        render_html_article(self.extract_source_from_html(html, Some(url))?)
    }
    pub fn extract_document_html(&self, document: Document<'_>) -> Result<HtmlArticle> {
        render_html_article(self.extract_source_from_document(document, None)?)
    }
    pub fn extract_document_html_with_url(
        &self,
        document: Document<'_>,
        url: &Url,
    ) -> Result<HtmlArticle> {
        render_html_article(self.extract_source_from_document(document, Some(url))?)
    }

    /// Extracts CommonMark Markdown with default format options.
    pub fn extract_markdown(&self, html: &str) -> Result<MarkdownArticle> {
        self.extract_markdown_with(html, &MarkdownOptions::default())
    }
    /// Extracts CommonMark Markdown with explicit format options.
    pub fn extract_markdown_with(
        &self,
        html: &str,
        options: &MarkdownOptions,
    ) -> Result<MarkdownArticle> {
        render_markdown_article(self.extract_source_from_html(html, None)?, options)
    }
    pub fn extract_markdown_with_url(&self, html: &str, url: &Url) -> Result<MarkdownArticle> {
        self.extract_markdown_with_url_and_options(html, url, &MarkdownOptions::default())
    }
    pub fn extract_markdown_with_url_and_options(
        &self,
        html: &str,
        url: &Url,
        options: &MarkdownOptions,
    ) -> Result<MarkdownArticle> {
        render_markdown_article(self.extract_source_from_html(html, Some(url))?, options)
    }
    pub fn extract_document_markdown(&self, document: Document<'_>) -> Result<MarkdownArticle> {
        self.extract_document_markdown_with(document, &MarkdownOptions::default())
    }
    pub fn extract_document_markdown_with(
        &self,
        document: Document<'_>,
        options: &MarkdownOptions,
    ) -> Result<MarkdownArticle> {
        render_markdown_article(self.extract_source_from_document(document, None)?, options)
    }
    pub fn extract_document_markdown_with_url(
        &self,
        document: Document<'_>,
        url: &Url,
    ) -> Result<MarkdownArticle> {
        self.extract_document_markdown_with_url_and_options(
            document,
            url,
            &MarkdownOptions::default(),
        )
    }
    pub fn extract_document_markdown_with_url_and_options(
        &self,
        document: Document<'_>,
        url: &Url,
        options: &MarkdownOptions,
    ) -> Result<MarkdownArticle> {
        render_markdown_article(
            self.extract_source_from_document(document, Some(url))?,
            options,
        )
    }

    /// Extracts normalized text with default format options.
    pub fn extract_text(&self, html: &str) -> Result<TextArticle> {
        self.extract_text_with(html, &TextOptions::default())
    }
    /// Extracts normalized text with explicit format options.
    pub fn extract_text_with(&self, html: &str, options: &TextOptions) -> Result<TextArticle> {
        render_text_article(self.extract_source_from_html(html, None)?, options)
    }
    pub fn extract_text_with_url(&self, html: &str, url: &Url) -> Result<TextArticle> {
        self.extract_text_with_url_and_options(html, url, &TextOptions::default())
    }
    pub fn extract_text_with_url_and_options(
        &self,
        html: &str,
        url: &Url,
        options: &TextOptions,
    ) -> Result<TextArticle> {
        render_text_article(self.extract_source_from_html(html, Some(url))?, options)
    }
    pub fn extract_document_text(&self, document: Document<'_>) -> Result<TextArticle> {
        self.extract_document_text_with(document, &TextOptions::default())
    }
    pub fn extract_document_text_with(
        &self,
        document: Document<'_>,
        options: &TextOptions,
    ) -> Result<TextArticle> {
        render_text_article(self.extract_source_from_document(document, None)?, options)
    }
    pub fn extract_document_text_with_url(
        &self,
        document: Document<'_>,
        url: &Url,
    ) -> Result<TextArticle> {
        self.extract_document_text_with_url_and_options(document, url, &TextOptions::default())
    }
    pub fn extract_document_text_with_url_and_options(
        &self,
        document: Document<'_>,
        url: &Url,
        options: &TextOptions,
    ) -> Result<TextArticle> {
        render_text_article(
            self.extract_source_from_document(document, Some(url))?,
            options,
        )
    }

    fn extract_source_from_html(&self, html: &str, url: Option<&Url>) -> Result<ExtractedArticle> {
        self.check_size(html)?;
        let document = Document::parse(html)?;
        self.extract_source(document, url)
    }
    fn extract_source_from_document(
        &self,
        document: Document<'_>,
        url: Option<&Url>,
    ) -> Result<ExtractedArticle> {
        self.check_size(document.html)?;
        self.extract_source(document, url)
    }
    fn extract_source(
        &self,
        document: Document<'_>,
        url: Option<&Url>,
    ) -> Result<ExtractedArticle> {
        crate::readability::Readability::from_document(
            document.doc,
            document.html,
            url.map(Url::as_str),
            &self.config,
        )
        .extract_source()
    }
    fn check_size(&self, html: &str) -> Result<()> {
        if let Some(limit) = self.max_input_bytes
            && html.len() > limit
        {
            return Err(Error::InputTooLarge {
                actual: html.len(),
                limit,
            });
        }
        Ok(())
    }
}

fn render_html_article(source: ExtractedArticle) -> Result<HtmlArticle> {
    let (dom, root, metadata, text_char_count) = source.into_parts();
    let content = crate::dom::render_html(&dom, root, text_char_count);
    drop(dom);
    Ok(HtmlArticle {
        metadata,
        content,
        text_char_count,
    })
}

fn render_markdown_article(
    source: ExtractedArticle,
    options: &MarkdownOptions,
) -> Result<MarkdownArticle> {
    let (dom, root, metadata, text_char_count) = source.into_parts();
    let content = crate::markdown::render_markdown(
        &dom,
        root,
        text_char_count,
        options.include_links(),
        options.include_images(),
    );
    let content = options.apply(content);
    drop(dom);
    Ok(MarkdownArticle {
        metadata,
        content,
        text_char_count,
    })
}

fn render_text_article(source: ExtractedArticle, options: &TextOptions) -> Result<TextArticle> {
    let (dom, root, metadata, text_char_count) = source.into_parts();
    let content = crate::text::render_text(&dom, root, text_char_count, options);
    drop(dom);
    Ok(TextArticle {
        metadata,
        content,
        text_char_count,
    })
}
pub struct ExtractorBuilder {
    extractor: Extractor,
    embed_policy: Option<EmbedPolicy>,
}
impl ExtractorBuilder {
    pub fn max_input_bytes(mut self, n: usize) -> Self {
        self.extractor.max_input_bytes = Some(n);
        self
    }
    pub fn max_elements(mut self, n: usize) -> Self {
        self.extractor.config.max_elems_to_parse = n;
        self
    }
    pub fn retry_length_threshold(mut self, n: usize) -> Self {
        self.extractor.config.char_threshold = n;
        self
    }
    pub fn class_policy(mut self, p: ClassPolicy) -> Self {
        match p {
            ClassPolicy::StripSourceClasses => {
                self.extractor.config.keep_classes = false;
                self.extractor.config.classes_to_preserve = vec!["page".into()]
            }
            ClassPolicy::Preserve(v) => {
                self.extractor.config.keep_classes = false;
                self.extractor.config.classes_to_preserve =
                    std::iter::once("page".to_owned()).chain(v).collect()
            }
            ClassPolicy::PreserveAll => self.extractor.config.keep_classes = true,
        };
        self
    }
    pub fn metadata_sources(mut self, sources: MetadataSources) -> Self {
        self.extractor.config.disable_json_ld = !sources.json_ld_enabled();
        self.extractor.config.metadata_sources = sources.bits;
        self
    }
    pub fn embed_policy(mut self, policy: EmbedPolicy) -> Self {
        self.embed_policy = Some(policy);
        self
    }
    pub fn heuristics(mut self, h: Heuristics) -> Self {
        self.extractor.config.nb_top_candidates = h.candidate_count;
        self.extractor.config.link_density_modifier = h.link_density_bias;
        self.extractor.config.char_threshold = h.retry_length_threshold;
        self
    }
    pub fn build(mut self) -> Result<Extractor> {
        self.extractor.config.allowed_video_regex = match self.embed_policy {
            None | Some(EmbedPolicy::KnownProviders) => None,
            Some(EmbedPolicy::RemoveAll) => Some(regex::Regex::new(r"a^").expect("valid regex")),
            Some(EmbedPolicy::AllowHosts(hosts)) => Some(allow_hosts_regex(hosts)?),
        };
        Ok(self.extractor)
    }
}
#[derive(Clone, Debug, Default)]
pub enum ClassPolicy {
    #[default]
    StripSourceClasses,
    Preserve(Vec<String>),
    PreserveAll,
}
#[derive(Clone, Copy, Debug)]
pub struct MetadataSources {
    bits: u8,
}
impl Default for MetadataSources {
    fn default() -> Self {
        Self::all()
    }
}
impl MetadataSources {
    pub const fn all() -> Self {
        Self { bits: 0b1111 }
    }
    pub const fn none() -> Self {
        Self { bits: 0 }
    }
    pub const fn json_ld(self, e: bool) -> Self {
        Self {
            bits: if e { self.bits | 1 } else { self.bits & !1 },
        }
    }
    pub const fn open_graph(self, enabled: bool) -> Self {
        Self {
            bits: if enabled {
                self.bits | 2
            } else {
                self.bits & !2
            },
        }
    }
    pub const fn twitter(self, enabled: bool) -> Self {
        Self {
            bits: if enabled {
                self.bits | 4
            } else {
                self.bits & !4
            },
        }
    }
    pub const fn standard_meta(self, enabled: bool) -> Self {
        Self {
            bits: if enabled {
                self.bits | 8
            } else {
                self.bits & !8
            },
        }
    }
    const fn json_ld_enabled(self) -> bool {
        self.bits & 1 != 0
    }
}
#[derive(Clone, Debug, Default)]
pub enum EmbedPolicy {
    RemoveAll,
    #[default]
    KnownProviders,
    AllowHosts(Vec<String>),
}
#[derive(Clone, Debug)]
pub struct Heuristics {
    candidate_count: usize,
    retry_length_threshold: usize,
    link_density_bias: f64,
}
impl Default for Heuristics {
    fn default() -> Self {
        Self {
            candidate_count: 5,
            retry_length_threshold: 500,
            link_density_bias: 0.0,
        }
    }
}
impl Heuristics {
    pub fn candidate_count(mut self, n: usize) -> Self {
        self.candidate_count = n;
        self
    }
    pub fn retry_length_threshold(mut self, n: usize) -> Self {
        self.retry_length_threshold = n;
        self
    }
    pub fn link_density_bias(mut self, n: f64) -> Self {
        self.link_density_bias = n;
        self
    }
}

fn allow_hosts_regex(hosts: Vec<String>) -> Result<regex::Regex> {
    if hosts.is_empty() {
        return Err(Error::InvalidConfiguration {
            message: "embed host list cannot be empty".into(),
        });
    }
    let mut normalized = Vec::with_capacity(hosts.len());
    for host in hosts {
        let host = host.trim();
        let parsed = Url::parse(&format!("http://{host}/")).ok();
        let valid = parsed.as_ref().is_some_and(|url| {
            !host.is_empty()
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
        });
        if !valid {
            return Err(Error::InvalidConfiguration {
                message: format!("invalid embed host: {host:?}"),
            });
        }
        normalized.push(regex::escape(
            parsed.unwrap().host_str().expect("validated host"),
        ));
    }
    let hosts = normalized.join("|");
    regex::Regex::new(&format!(
        r"(?i)^https?://(?:[a-z0-9](?:[a-z0-9-]{{0,61}}[a-z0-9])?\.)*(?:{hosts})(?::\d+)?(?:/|$)"
    ))
    .map_err(|error| Error::InvalidConfiguration {
        message: error.to_string(),
    })
}

/// Extracts an HTML article with default extraction configuration.
///
/// The returned HTML fragment is not sanitized.
pub fn extract_html(html: &str) -> Result<HtmlArticle> {
    Extractor::default().extract_html(html)
}
/// Extracts an HTML article and resolves relative URLs against `url`.
pub fn extract_html_with_url(html: &str, url: &Url) -> Result<HtmlArticle> {
    Extractor::default().extract_html_with_url(html, url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_host_policy_validates_and_matches_host_boundaries() {
        for hosts in [vec![], vec!["   ".into()], vec!["example.com/path".into()]] {
            assert!(
                Extractor::builder()
                    .embed_policy(EmbedPolicy::AllowHosts(hosts))
                    .build()
                    .is_err()
            );
        }
        let extractor = Extractor::builder()
            .embed_policy(EmbedPolicy::AllowHosts(vec!["video.example.com".into()]))
            .build()
            .unwrap();
        let allowed = extractor.config.allowed_video_regex.as_ref().unwrap();
        assert!(allowed.is_match("https://video.example.com/embed/1"));
        assert!(allowed.is_match("https://cdn.video.example.com/embed/1"));
        assert!(!allowed.is_match("https://video.example.com.evil.test/embed/1"));
        assert!(!allowed.is_match("https://evil.test/video.example.com"));
        assert!(!allowed.is_match("https://evil.test?.video.example.com/"));
        assert!(!allowed.is_match("https://evil.test#.video.example.com/"));
        assert!(!allowed.is_match("https://user@video.example.com/"));
        assert!(!allowed.is_match("https://evil%2evideo.example.com/"));
    }

    #[test]
    fn limits_apply_without_counting_text_as_markup() {
        let extractor = Extractor::builder().max_input_bytes(8).build().unwrap();
        let document = Document::parse("<p>article text</p>").unwrap();
        assert!(matches!(
            extractor.extract_document_html(document),
            Err(Error::InputTooLarge { .. })
        ));

        let html = "<article><p>1 &lt; two. This is readable article text.</p></article>";
        let extractor = Extractor::builder()
            .max_elements(5)
            .retry_length_threshold(0)
            .build()
            .unwrap();
        assert!(!matches!(
            extractor.extract_html(html),
            Err(Error::TooManyElements { .. })
        ));
    }

    #[test]
    fn rich_json_ld_metadata_is_exposed() {
        let html = r#"<script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","headline":"JSON title","author":{"@type":"Person","name":"Doe, Jane","url":"/authors/ada"},"publisher":{"name":"Publisher"},"mainEntityOfPage":{"@id":"/article"},"image":{"url":"/lead.jpg","caption":"Lead","width":640,"height":480},"dateModified":"2024-02-03","articleSection":"Science","keywords":["rust","web"]}</script><article><p>Readable article content.</p></article>"#;
        let base = Url::parse("https://example.test/base").unwrap();
        let article = Extractor::builder()
            .retry_length_threshold(0)
            .build()
            .unwrap()
            .extract_html_with_url(html, &base)
            .unwrap();
        let metadata = article.metadata();
        assert_eq!(metadata.publisher(), Some("Publisher"));
        assert_eq!(
            metadata.canonical_url().unwrap().as_str(),
            "https://example.test/article"
        );
        assert_eq!(metadata.authors().len(), 1);
        assert_eq!(metadata.authors()[0].name(), "Doe, Jane");
        assert_eq!(
            metadata.authors()[0].url().unwrap().as_str(),
            "https://example.test/authors/ada"
        );
        let image = metadata.lead_image().unwrap();
        assert_eq!(image.url().as_str(), "https://example.test/lead.jpg");
        assert_eq!(image.alt(), Some("Lead"));
        assert_eq!(image.width(), Some(640));
        assert_eq!(metadata.modified_time(), Some("2024-02-03"));
        assert_eq!(metadata.section(), Some("Science"));
        assert_eq!(metadata.tags(), ["rust", "web"]);
    }

    #[test]
    fn open_graph_image_groups_do_not_mix() {
        let html = r#"<meta property="og:image:alt" content="First alt"><meta property="og:image" content="/first.jpg"><meta property="og:image:width" content="640"><meta property="og:image" content="/second.jpg"><meta property="og:image:alt" content="Second alt"><meta property="og:image:width" content="999"><article><p>Readable content.</p></article>"#;
        let base = Url::parse("https://example.test/").unwrap();
        let article = Extractor::builder()
            .retry_length_threshold(0)
            .build()
            .unwrap()
            .extract_html_with_url(html, &base)
            .unwrap();
        let image = article.metadata().lead_image().unwrap();
        assert_eq!(image.url().as_str(), "https://example.test/first.jpg");
        assert_eq!(image.alt(), Some("First alt"));
        assert_eq!(image.width(), Some(640));
    }

    #[test]
    fn metadata_source_selection_is_enforced() {
        let html = r#"<html><head><title>Fallback title</title>
            <meta property="og:site_name" content="OG site"><meta property="article:tag" content="news">
            <meta name="twitter:description" content="Twitter excerpt"><meta name="author" content="Standard author">
            <link rel="canonical" href="/canonical"></head><body><article><p>Readable article text repeated enough for extraction.</p></article></body></html>"#;
        let none = Extractor::builder()
            .metadata_sources(MetadataSources::none())
            .retry_length_threshold(0)
            .build()
            .unwrap()
            .extract_html(html)
            .unwrap();
        assert_eq!(none.metadata().title(), Some("Fallback title"));
        assert!(none.metadata().site_name().is_none());
        assert!(none.metadata().canonical_url().is_none());
        assert!(none.metadata().tags().is_empty());

        let twitter = Extractor::builder()
            .metadata_sources(MetadataSources::none().twitter(true))
            .retry_length_threshold(0)
            .build()
            .unwrap()
            .extract_html(html)
            .unwrap();
        assert_eq!(twitter.metadata().excerpt(), Some("Twitter excerpt"));
        assert!(twitter.metadata().site_name().is_none());
    }

    fn test_extractor() -> Extractor {
        Extractor::builder()
            .retry_length_threshold(0)
            .build()
            .unwrap()
    }

    #[test]
    fn explicit_results_and_formats_match_legacy_output() {
        let html = r#"<title>Example</title><article><h1>Example</h1><p>Hello <em>world</em>!</p><p>Second line.</p></article>"#;
        let extractor = test_extractor();
        let html_article = extractor.extract_html(html).unwrap();
        let markdown = extractor.extract_markdown(html).unwrap();
        let text = extractor.extract_text(html).unwrap();

        assert_eq!(html_article.metadata(), markdown.metadata());
        assert_eq!(markdown.metadata(), text.metadata());
        assert!(html_article.content().contains("Hello <em>world</em>!"));
        assert!(
            markdown.content().contains("Hello *world*\\!"),
            "{}",
            markdown.content()
        );
        assert_eq!(text.content(), "Hello world!Second line.");
        assert_eq!(text.content().chars().count(), text.text_char_count());
        assert_eq!(html_article.text_char_count(), text.text_char_count());
        assert!(markdown.into_content().contains("Second line."));
        assert!(html_article.into_content().contains("<p>"));

        #[allow(deprecated)]
        let legacy =
            crate::parse(html, None, Some(crate::Options::new().char_threshold(0))).unwrap();
        let html_article = extractor.extract_html(html).unwrap();
        let markdown = extractor.extract_markdown(html).unwrap();
        assert_eq!(legacy.content, html_article.content());
        assert_eq!(legacy.markdown_content, markdown.content());
        assert_eq!(legacy.text_content, text.content());
        assert_eq!(legacy.length, text.text_char_count());
    }

    #[test]
    fn format_options_and_url_variants_are_applied() {
        let html = r#"<article><h1>Heading</h1><ul><li>Item</li></ul><p>A<br>B <a href="/path">link</a><img src="/image.jpg" alt="photo"></p></article>"#;
        let extractor = test_extractor();
        let base = Url::parse("https://example.test/base/").unwrap();
        let html_article = extractor.extract_html_with_url(html, &base).unwrap();
        assert!(html_article.content().contains("https://example.test/path"));
        assert!(
            html_article
                .content()
                .contains("https://example.test/image.jpg")
        );

        let options = MarkdownOptions::default()
            .heading_style(crate::HeadingStyle::Setext)
            .bullet_marker(crate::BulletMarker::Plus)
            .images(false);
        let markdown = extractor
            .extract_markdown_with_url_and_options(html, &base, &options)
            .unwrap();
        assert!(
            markdown.content().contains("Heading\n-------"),
            "{}",
            markdown.content()
        );
        assert!(markdown.content().contains("+ Item"));
        assert!(markdown.content().contains("https://example.test/path"));
        assert!(!markdown.content().contains("!["));

        let without_links = extractor
            .extract_markdown_with(html, &MarkdownOptions::default().links(false))
            .unwrap();
        assert!(!without_links.content().contains("[link]("));
        let text = extractor
            .extract_text_with_url_and_options(
                html,
                &base,
                &TextOptions::default()
                    .block_separator(crate::TextSeparator::Newline)
                    .preserve_line_breaks(true),
            )
            .unwrap();
        assert!(text.content().contains("A\nB link"));
    }

    #[test]
    fn document_variants_work_after_readability_borrow() {
        let html = format!(
            "<article><p>{}</p></article>",
            "Readable article sentence. ".repeat(30)
        );
        let extractor = test_extractor();
        let document = Document::parse(&html).unwrap();
        assert!(document.is_probably_readable());
        assert!(
            extractor
                .extract_document_markdown(document)
                .unwrap()
                .content()
                .contains("Readable article")
        );

        let base = Url::parse("https://example.test/").unwrap();
        let document =
            Document::parse("<article><p><a href='/x'>Text link</a></p></article>").unwrap();
        let article = extractor
            .extract_document_markdown_with_url_and_options(
                document,
                &base,
                &MarkdownOptions::default(),
            )
            .unwrap();
        assert!(article.content().contains("https://example.test/x"));
    }

    #[test]
    fn deeply_nested_content_renders_without_recursion() {
        let depth = 1_000;
        let html = format!(
            "<article>{}deep text{}</article>",
            "<div>".repeat(depth),
            "</div>".repeat(depth)
        );
        let extractor = test_extractor();
        assert!(
            extractor
                .extract_html(&html)
                .unwrap()
                .content()
                .contains("deep text")
        );
        assert!(
            extractor
                .extract_markdown(&html)
                .unwrap()
                .content()
                .contains("deep text")
        );
        assert_eq!(
            extractor.extract_text(&html).unwrap().content(),
            "deep text"
        );
    }
}
