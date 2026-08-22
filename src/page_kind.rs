//! Internal page categories that guide extraction policy.

use crate::dom::{Dom, Tag};

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
    /// The page contains one job description and optional employer details.
    JobListing,
}

impl PageKind {
    /// Detects a job listing from independent structural and textual signals.
    pub(crate) fn detect(dom: &Dom) -> Self {
        let root = dom.body().unwrap_or_else(|| dom.root());
        let named_job_listing = std::iter::once(root)
            .chain(dom.descendants(root))
            .any(|node| {
                dom.attrs(node).iter().any(|attribute| {
                    matches!(dom.attribute_local_name(attribute), "class" | "id")
                        && attribute
                            .value
                            .split(|character: char| !character.is_ascii_alphanumeric())
                            .any(|token| token.eq_ignore_ascii_case("job"))
                })
            });

        let mut role_headings = 0_u8;
        let mut profile_fields = 0_u8;
        let mut text = String::new();
        for node in dom.descendants(root).filter(|&node| {
            matches!(
                dom.tag(node),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::Dt)
            )
        }) {
            text.clear();
            // Job profile labels are short. Bound the scan so malformed nested
            // headings do not rescan the remainder of the document.
            dom.append_normalized_text_limited(node, &mut text, 64);
            let text = text
                .trim()
                .trim_end_matches(':')
                .trim()
                .to_ascii_lowercase();
            match text.as_str() {
                "about the role"
                | "the role"
                | "the opportunity"
                | "what you'll do"
                | "what we're looking for"
                | "what you'll get"
                | "qualifications"
                | "requirements" => {
                    role_headings = role_headings.saturating_add(1);
                }
                "founded" | "batch" | "team size" | "status" => {
                    profile_fields = profile_fields.saturating_add(1);
                }
                _ => {}
            }
        }

        if role_headings >= 2 && (named_job_listing || profile_fields >= 3) {
            Self::JobListing
        } else {
            Self::Unknown
        }
    }

    /// Returns whether article-oriented relevance cleanup is appropriate.
    pub(crate) fn uses_article_cleanup(self) -> bool {
        matches!(self, Self::Unknown | Self::JobListing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wrapped_job_profile_labels() {
        let dom = Dom::parse_document(
            "<main class='job'><h2><span>About the role</span></h2><h3>Qualifications</h3></main>",
        )
        .unwrap();

        assert_eq!(PageKind::detect(&dom), PageKind::JobListing);
    }

    #[test]
    fn detects_common_job_section_headings() {
        let dom = Dom::parse_document(
            "<main class='show_job'><h2>The Opportunity</h2><h2>What You'll Do</h2><h2>What We're Looking For</h2></main>",
        )
        .unwrap();

        assert_eq!(PageKind::detect(&dom), PageKind::JobListing);
    }
}
