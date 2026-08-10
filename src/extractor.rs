//! Content extractor configuration and entry point.

use crate::dom::Dom;
use crate::error::Result;
use crate::page::ExtractedPage;
use crate::readability::Readability;

/// A reusable HTML content extractor.
#[derive(Debug, Clone)]
pub struct Extractor {
    pub(crate) config: ExtractorConfig,
}

/// Builds an [`Extractor`].
#[derive(Debug, Clone)]
pub struct ExtractorBuilder {
    config: ExtractorConfig,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtractorConfig {
    pub(crate) max_elements: usize,
    pub(crate) structured_data: bool,
    pub(crate) top_candidates: usize,
    pub(crate) classes_to_preserve: Vec<String>,
    pub(crate) keep_classes: bool,
    pub(crate) debug: bool,
}

impl Default for ExtractorConfig {
    fn default() -> Self {
        Self {
            max_elements: 0,
            structured_data: true,
            top_candidates: 5,
            classes_to_preserve: vec!["page".into(), "caption".into()],
            keep_classes: false,
            debug: false,
        }
    }
}

impl Extractor {
    /// Returns a builder with the default extraction configuration.
    pub fn builder() -> ExtractorBuilder {
        ExtractorBuilder {
            config: ExtractorConfig::default(),
        }
    }

    /// Extracts relevant content and metadata from an HTML document.
    ///
    /// `url` must be an absolute URL when present. Legible uses it to resolve
    /// relative links and media URLs.
    pub fn extract(&self, html: &str, url: Option<&str>) -> Result<ExtractedPage> {
        let dom = Dom::parse_document(html).expect("HTML DOM node limit exceeded");
        Readability::from_document(dom, url, &self.config).extract()
    }
}

impl Default for Extractor {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl ExtractorBuilder {
    /// Sets the maximum number of HTML elements. Use `0` for no limit.
    pub fn max_elements(mut self, max: usize) -> Self {
        self.config.max_elements = max;
        self
    }

    /// Controls whether JSON-LD participates in metadata extraction.
    pub fn structured_data(mut self, enabled: bool) -> Self {
        self.config.structured_data = enabled;
        self
    }

    /// Builds the extractor.
    pub fn build(self) -> Extractor {
        Extractor {
            config: self.config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[test]
    fn builder_defaults_match_extractor_defaults() {
        let built = Extractor::builder().build();
        let default = Extractor::default();
        assert_eq!(built.config.max_elements, default.config.max_elements);
        assert_eq!(built.config.structured_data, default.config.structured_data);
        assert_eq!(built.config.max_elements, 0);
        assert!(built.config.structured_data);
    }

    #[test]
    fn builder_sets_public_configuration() {
        let extractor = Extractor::builder()
            .max_elements(123)
            .structured_data(false)
            .build();
        assert_eq!(extractor.config.max_elements, 123);
        assert!(!extractor.config.structured_data);
    }

    #[test]
    fn max_elements_is_enforced() {
        let extractor = Extractor::builder().max_elements(1).build();
        assert!(matches!(
            extractor.extract("<main><p>Content</p></main>", None),
            Err(Error::TooManyElements(_, 1))
        ));
    }

    #[test]
    fn structured_data_can_be_disabled() {
        let html = r#"<html><head><title>Page</title>
            <script type="application/ld+json">
            {"@context":"https://schema.org","@type":"Article","author":[{"name":"Ada"},{"name":"Grace"}]}
            </script></head><body><main><p>Useful page content.</p></main></body></html>"#;
        let enabled = Extractor::default().extract(html, None).unwrap();
        let disabled = Extractor::builder()
            .structured_data(false)
            .build()
            .extract(html, None)
            .unwrap();

        assert_eq!(enabled.metadata().authors, ["Ada", "Grace"]);
        assert!(disabled.metadata().authors.is_empty());
    }
}
