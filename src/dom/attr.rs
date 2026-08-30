#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum AttrName {
    Action,
    Align,
    Alt,
    AriaHidden,
    AriaLabel,
    AriaLive,
    AriaLevel,
    AriaModal,
    Background,
    BgColor,
    Border,
    CellPadding,
    CellSpacing,
    Checked,
    Class,
    ColSpan,
    Content,
    DataCallout,
    DataCalloutLegacy,
    DataCodeLanguage,
    DataLegibleAuthor,
    DataLegibleBody,
    DataLegibleByline,
    DataLegibleKind,
    DataLegiblePrimary,
    DataLegibleReplies,
    DataLegibleReply,
    DataLegibleReplyBody,
    DataLegibleReplyMeta,
    DataFootnote,
    DataFootnoteLegacy,
    DataFootnoteRef,
    DataFootnoteRefLegacy,
    DataFootnotes,
    DataFootnotesLegacy,
    DataFn,
    DataType,
    DataLang,
    DataLanguage,
    DataLatex,
    DataMath,
    DataMathLegacy,
    DataSrc,
    DataSrcset,
    DataTable,
    DataArticleToc,
    DataDiscourseBaseUrl,
    DataFullname,
    DataMessageAuthorRole,
    DataMessageContent,
    DataPostId,
    DataRole,
    DataShortid,
    DataTestid,
    DataTopicTitle,
    DataTurboBody,
    DataUserCard,
    DataLanguageLabel,
    DataFootnoteBackref,
    DataTex,
    DataFormula,
    HttpEquiv,
    Datetime,
    Display,
    Fill,
    Dir,
    Disabled,
    Encoding,
    For,
    Frame,
    Height,
    Hidden,
    Href,
    HSpace,
    Id,
    ItemProp,
    Lang,
    Language,
    Name,
    Open,
    Poster,
    Property,
    Rel,
    Role,
    RowSpan,
    Rules,
    Separators,
    Src,
    Srcset,
    Start,
    Style,
    Summary,
    Title,
    Type,
    VAlign,
    VSpace,
    Width,
    Other,
}
impl AttrName {
    pub(crate) fn from_local(s: &str) -> Self {
        let kind = Self::from_lowercase(s);
        if kind != Self::Other || !s.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return kind;
        }
        let lowercase = s.to_ascii_lowercase();
        Self::from_lowercase(&lowercase)
    }

    #[inline]
    fn from_lowercase(s: &str) -> Self {
        match s {
            "action" => Self::Action,
            "align" => Self::Align,
            "alt" => Self::Alt,
            "aria-hidden" => Self::AriaHidden,
            "aria-label" => Self::AriaLabel,
            "aria-live" => Self::AriaLive,
            "aria-level" => Self::AriaLevel,
            "aria-modal" => Self::AriaModal,
            "background" => Self::Background,
            "bgcolor" => Self::BgColor,
            "border" => Self::Border,
            "cellpadding" => Self::CellPadding,
            "cellspacing" => Self::CellSpacing,
            "checked" => Self::Checked,
            "class" => Self::Class,
            "colspan" => Self::ColSpan,
            "content" => Self::Content,
            "data-legible-callout" => Self::DataCallout,
            "data-callout" => Self::DataCalloutLegacy,
            "data-code-language" => Self::DataCodeLanguage,
            "data-legible-author" => Self::DataLegibleAuthor,
            "data-legible-body" => Self::DataLegibleBody,
            "data-legible-byline" => Self::DataLegibleByline,
            "data-legible-kind" => Self::DataLegibleKind,
            "data-legible-primary" => Self::DataLegiblePrimary,
            "data-legible-replies" => Self::DataLegibleReplies,
            "data-legible-reply" => Self::DataLegibleReply,
            "data-legible-reply-body" => Self::DataLegibleReplyBody,
            "data-legible-reply-meta" => Self::DataLegibleReplyMeta,
            "data-legible-footnote" => Self::DataFootnote,
            "data-footnote" => Self::DataFootnoteLegacy,
            "data-legible-footnote-ref" => Self::DataFootnoteRef,
            "data-footnote-ref" => Self::DataFootnoteRefLegacy,
            "data-legible-footnotes" => Self::DataFootnotes,
            "data-footnotes" => Self::DataFootnotesLegacy,
            "data-fn" => Self::DataFn,
            "data-type" => Self::DataType,
            "data-lang" => Self::DataLang,
            "data-language" => Self::DataLanguage,
            "data-latex" => Self::DataLatex,
            "data-legible-math" => Self::DataMath,
            "data-math" => Self::DataMathLegacy,
            "data-src" => Self::DataSrc,
            "data-srcset" => Self::DataSrcset,
            "datatable" => Self::DataTable,
            "data-article-toc" => Self::DataArticleToc,
            "data-discourse-base-url" => Self::DataDiscourseBaseUrl,
            "data-fullname" => Self::DataFullname,
            "data-message-author-role" => Self::DataMessageAuthorRole,
            "data-message-content" => Self::DataMessageContent,
            "data-post-id" => Self::DataPostId,
            "data-role" => Self::DataRole,
            "data-shortid" => Self::DataShortid,
            "data-testid" => Self::DataTestid,
            "data-topic-title" => Self::DataTopicTitle,
            "data-turbo-body" => Self::DataTurboBody,
            "data-user-card" => Self::DataUserCard,
            "data-language-label" => Self::DataLanguageLabel,
            "data-footnote-backref" => Self::DataFootnoteBackref,
            "data-tex" => Self::DataTex,
            "data-formula" => Self::DataFormula,
            "http-equiv" => Self::HttpEquiv,
            "datetime" => Self::Datetime,
            "display" => Self::Display,
            "fill" => Self::Fill,
            "dir" => Self::Dir,
            "disabled" => Self::Disabled,
            "encoding" => Self::Encoding,
            "for" => Self::For,
            "frame" => Self::Frame,
            "height" => Self::Height,
            "hidden" => Self::Hidden,
            "href" => Self::Href,
            "hspace" => Self::HSpace,
            "id" => Self::Id,
            "itemprop" => Self::ItemProp,
            "lang" => Self::Lang,
            "language" => Self::Language,
            "name" => Self::Name,
            "open" => Self::Open,
            "poster" => Self::Poster,
            "property" => Self::Property,
            "rel" => Self::Rel,
            "role" => Self::Role,
            "rowspan" => Self::RowSpan,
            "rules" => Self::Rules,
            "separators" => Self::Separators,
            "src" => Self::Src,
            "srcset" => Self::Srcset,
            "start" => Self::Start,
            "style" => Self::Style,
            "summary" => Self::Summary,
            "title" => Self::Title,
            "type" => Self::Type,
            "valign" => Self::VAlign,
            "vspace" => Self::VSpace,
            "width" => Self::Width,
            _ => Self::Other,
        }
    }
    /// Returns whether this kind exists only to accelerate dynamic local-name
    /// lookups. These names remain non-semantic for cleanup purposes.
    pub(crate) const fn is_lookup_only(self) -> bool {
        matches!(
            self,
            Self::Action
                | Self::Alt
                | Self::AriaLive
                | Self::AriaLevel
                | Self::DataCalloutLegacy
                | Self::DataFootnoteLegacy
                | Self::DataFootnoteRefLegacy
                | Self::DataFootnotesLegacy
                | Self::DataFn
                | Self::DataType
                | Self::Encoding
                | Self::For
                | Self::Open
                | Self::Separators
                | Self::DataArticleToc
                | Self::DataDiscourseBaseUrl
                | Self::DataFullname
                | Self::DataMessageAuthorRole
                | Self::DataMessageContent
                | Self::DataPostId
                | Self::DataRole
                | Self::DataShortid
                | Self::DataTestid
                | Self::DataTopicTitle
                | Self::DataTurboBody
                | Self::DataUserCard
                | Self::DataLanguageLabel
                | Self::DataFootnoteBackref
                | Self::DataTex
                | Self::DataFormula
                | Self::DataMathLegacy
                | Self::HttpEquiv
                | Self::Datetime
                | Self::Display
                | Self::Fill
        )
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Align => "align",
            Self::Alt => "alt",
            Self::AriaHidden => "aria-hidden",
            Self::AriaLabel => "aria-label",
            Self::AriaLive => "aria-live",
            Self::AriaLevel => "aria-level",
            Self::AriaModal => "aria-modal",
            Self::Background => "background",
            Self::BgColor => "bgcolor",
            Self::Border => "border",
            Self::CellPadding => "cellpadding",
            Self::CellSpacing => "cellspacing",
            Self::Checked => "checked",
            Self::Class => "class",
            Self::ColSpan => "colspan",
            Self::Content => "content",
            Self::DataCallout => "data-legible-callout",
            Self::DataCalloutLegacy => "data-callout",
            Self::DataCodeLanguage => "data-code-language",
            Self::DataLegibleAuthor => "data-legible-author",
            Self::DataLegibleBody => "data-legible-body",
            Self::DataLegibleByline => "data-legible-byline",
            Self::DataLegibleKind => "data-legible-kind",
            Self::DataLegiblePrimary => "data-legible-primary",
            Self::DataLegibleReplies => "data-legible-replies",
            Self::DataLegibleReply => "data-legible-reply",
            Self::DataLegibleReplyBody => "data-legible-reply-body",
            Self::DataLegibleReplyMeta => "data-legible-reply-meta",
            Self::DataFootnote => "data-legible-footnote",
            Self::DataFootnoteLegacy => "data-footnote",
            Self::DataFootnoteRef => "data-legible-footnote-ref",
            Self::DataFootnoteRefLegacy => "data-footnote-ref",
            Self::DataFootnotes => "data-legible-footnotes",
            Self::DataFootnotesLegacy => "data-footnotes",
            Self::DataFn => "data-fn",
            Self::DataType => "data-type",
            Self::DataLang => "data-lang",
            Self::DataLanguage => "data-language",
            Self::DataLatex => "data-latex",
            Self::DataMath => "data-legible-math",
            Self::DataMathLegacy => "data-math",
            Self::DataSrc => "data-src",
            Self::DataSrcset => "data-srcset",
            Self::DataTable => "datatable",
            Self::DataArticleToc => "data-article-toc",
            Self::DataDiscourseBaseUrl => "data-discourse-base-url",
            Self::DataFullname => "data-fullname",
            Self::DataMessageAuthorRole => "data-message-author-role",
            Self::DataMessageContent => "data-message-content",
            Self::DataPostId => "data-post-id",
            Self::DataRole => "data-role",
            Self::DataShortid => "data-shortid",
            Self::DataTestid => "data-testid",
            Self::DataTopicTitle => "data-topic-title",
            Self::DataTurboBody => "data-turbo-body",
            Self::DataUserCard => "data-user-card",
            Self::DataLanguageLabel => "data-language-label",
            Self::DataFootnoteBackref => "data-footnote-backref",
            Self::DataTex => "data-tex",
            Self::DataFormula => "data-formula",
            Self::HttpEquiv => "http-equiv",
            Self::Datetime => "datetime",
            Self::Display => "display",
            Self::Fill => "fill",
            Self::Dir => "dir",
            Self::Disabled => "disabled",
            Self::Encoding => "encoding",
            Self::For => "for",
            Self::Frame => "frame",
            Self::Height => "height",
            Self::Hidden => "hidden",
            Self::Href => "href",
            Self::HSpace => "hspace",
            Self::Id => "id",
            Self::ItemProp => "itemprop",
            Self::Lang => "lang",
            Self::Language => "language",
            Self::Name => "name",
            Self::Open => "open",
            Self::Poster => "poster",
            Self::Property => "property",
            Self::Rel => "rel",
            Self::Role => "role",
            Self::RowSpan => "rowspan",
            Self::Rules => "rules",
            Self::Separators => "separators",
            Self::Src => "src",
            Self::Srcset => "srcset",
            Self::Start => "start",
            Self::Style => "style",
            Self::Summary => "summary",
            Self::Title => "title",
            Self::Type => "type",
            Self::VAlign => "valign",
            Self::VSpace => "vspace",
            Self::Width => "width",
            Self::Other => "",
        }
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub(crate) struct Attribute {
    pub(crate) name: html5ever::QualName,
    pub(crate) value: tendril::StrTendril,
    kind: AttrName,
}

impl From<html5ever::Attribute> for Attribute {
    #[inline]
    fn from(attribute: html5ever::Attribute) -> Self {
        let kind = AttrName::from_local(attribute.name.local.as_ref());
        Self {
            name: attribute.name,
            value: attribute.value,
            kind,
        }
    }
}

impl Attribute {
    #[inline]
    pub(crate) fn new(name: html5ever::QualName, value: tendril::StrTendril) -> Self {
        let kind = AttrName::from_local(name.local.as_ref());
        Self { name, value, kind }
    }

    #[inline]
    pub(crate) fn is_named(&self, name: AttrName) -> bool {
        self.kind == name
    }

    #[inline]
    pub(crate) fn is_lookup_only(&self) -> bool {
        self.kind.is_lookup_only()
    }
}
