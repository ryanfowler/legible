//! Cached CSS selector matchers for performance optimization.
//!
//! This module provides pre-compiled `Matcher` objects for frequently-used CSS selectors.
//! The `dom_query` crate parses selectors on every `select()` call. By caching `Matcher`
//! objects in a static `Lazy<Selectors>`, we eliminate redundant parsing entirely.

use dom_query::Matcher;
use once_cell::sync::Lazy;

/// Global pre-compiled selectors, initialized once on first use.
pub static SELECTORS: Lazy<Selectors> = Lazy::new(Selectors::new);

/// Pre-compiled CSS selector matchers.
#[derive(Clone)]
pub struct Selectors {
    // Document-level selectors
    pub body: Matcher,
    pub html: Matcher,
    pub title: Matcher,
    pub meta: Matcher,

    // Common element selectors
    pub p: Matcher,
    pub a: Matcher,
    pub br: Matcher,
    pub h1: Matcher,
    pub h1_h2: Matcher,
    pub img: Matcher,
    pub li: Matcher,
    pub table: Matcher,

    // Compound selectors for hot paths
    pub ul_ol: Matcher,
    pub script_noscript: Matcher,
    pub style: Matcher,
    pub font: Matcher,
    pub noscript: Matcher,
    pub caption: Matcher,
    pub itemprop: Matcher,

    // Image/media related selectors
    pub img_picture_figure: Matcher,
    pub img_picture: Matcher,
    pub img_embed_object_iframe: Matcher,
    pub img_picture_figure_video_audio_source: Matcher,

    // Table data detection
    pub table_data_elements: Matcher,

    // Metadata selectors
    pub json_ld_script: Matcher,

    // Text density selectors (for clean_conditionally)
    pub headings: Matcher,
    pub textish_tags: Matcher,

    // clean_conditionally tag selectors
    pub form: Matcher,
    pub fieldset: Matcher,
    pub div: Matcher,

    // Readerable check
    pub p_pre_article: Matcher,
    pub div_br: Matcher,
}

impl Selectors {
    /// Create a new set of pre-compiled selectors.
    pub fn new() -> Self {
        Self {
            // Document-level selectors
            body: Matcher::new("body").unwrap(),
            html: Matcher::new("html").unwrap(),
            title: Matcher::new("title").unwrap(),
            meta: Matcher::new("meta").unwrap(),

            // Common element selectors
            p: Matcher::new("p").unwrap(),
            a: Matcher::new("a").unwrap(),
            br: Matcher::new("br").unwrap(),
            h1: Matcher::new("h1").unwrap(),
            h1_h2: Matcher::new("h1, h2").unwrap(),
            img: Matcher::new("img").unwrap(),
            li: Matcher::new("li").unwrap(),
            table: Matcher::new("table").unwrap(),

            // Compound selectors for hot paths
            ul_ol: Matcher::new("ul, ol").unwrap(),
            script_noscript: Matcher::new("script, noscript").unwrap(),
            style: Matcher::new("style").unwrap(),
            font: Matcher::new("font").unwrap(),
            noscript: Matcher::new("noscript").unwrap(),
            caption: Matcher::new("caption").unwrap(),
            itemprop: Matcher::new("[itemprop*='name']").unwrap(),

            // Image/media related selectors
            img_picture_figure: Matcher::new("img, picture, figure").unwrap(),
            img_picture: Matcher::new("img, picture").unwrap(),
            img_embed_object_iframe: Matcher::new("img, embed, object, iframe").unwrap(),
            img_picture_figure_video_audio_source: Matcher::new(
                "img, picture, figure, video, audio, source",
            )
            .unwrap(),

            // Table data detection
            table_data_elements: Matcher::new("col, colgroup, tfoot, thead, th").unwrap(),

            // Metadata selectors
            json_ld_script: Matcher::new("script[type='application/ld+json']").unwrap(),

            // Text density selectors (for clean_conditionally)
            headings: Matcher::new("h1, h2, h3, h4, h5, h6").unwrap(),
            textish_tags: Matcher::new(
                "span, li, td, blockquote, dl, div, img, ol, p, pre, table, ul",
            )
            .unwrap(),

            // clean_conditionally tag selectors
            form: Matcher::new("form").unwrap(),
            fieldset: Matcher::new("fieldset").unwrap(),
            div: Matcher::new("div").unwrap(),

            // Readerable check
            p_pre_article: Matcher::new("p, pre, article").unwrap(),
            div_br: Matcher::new("div > br").unwrap(),
        }
    }
}

impl Default for Selectors {
    fn default() -> Self {
        Self::new()
    }
}
