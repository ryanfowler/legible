//! Internal page categories that guide extraction policy.

/// Describes the semantic shape of the selected page content.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PageKind {
    /// Generic extraction has not assigned a more precise category.
    #[default]
    Unknown,
    /// The page contains a sequence of independent entries.
    Listing,
    /// The page contains a primary entry and a threaded discussion.
    Discussion,
}

impl PageKind {
    /// Returns whether article-oriented relevance cleanup is appropriate.
    pub(crate) fn uses_article_cleanup(self) -> bool {
        matches!(self, Self::Unknown)
    }
}
