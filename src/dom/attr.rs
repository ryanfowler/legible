#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum AttrName {
    Align,
    AriaHidden,
    AriaLabel,
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
    DataFootnoteRef,
    DataFootnotes,
    DataLang,
    DataLanguage,
    DataLatex,
    DataMath,
    DataSrc,
    DataSrcset,
    DataTable,
    Dir,
    Disabled,
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
    Poster,
    Property,
    Rel,
    Role,
    RowSpan,
    Rules,
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
            "align" => Self::Align,
            "aria-hidden" => Self::AriaHidden,
            "aria-label" => Self::AriaLabel,
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
            "data-legible-footnote-ref" => Self::DataFootnoteRef,
            "data-legible-footnotes" => Self::DataFootnotes,
            "data-lang" => Self::DataLang,
            "data-language" => Self::DataLanguage,
            "data-latex" => Self::DataLatex,
            "data-legible-math" => Self::DataMath,
            "data-src" => Self::DataSrc,
            "data-srcset" => Self::DataSrcset,
            "datatable" => Self::DataTable,
            "dir" => Self::Dir,
            "disabled" => Self::Disabled,
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
            "poster" => Self::Poster,
            "property" => Self::Property,
            "rel" => Self::Rel,
            "role" => Self::Role,
            "rowspan" => Self::RowSpan,
            "rules" => Self::Rules,
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
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Align => "align",
            Self::AriaHidden => "aria-hidden",
            Self::AriaLabel => "aria-label",
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
            Self::DataFootnoteRef => "data-legible-footnote-ref",
            Self::DataFootnotes => "data-legible-footnotes",
            Self::DataLang => "data-lang",
            Self::DataLanguage => "data-language",
            Self::DataLatex => "data-latex",
            Self::DataMath => "data-legible-math",
            Self::DataSrc => "data-src",
            Self::DataSrcset => "data-srcset",
            Self::DataTable => "datatable",
            Self::Dir => "dir",
            Self::Disabled => "disabled",
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
            Self::Poster => "poster",
            Self::Property => "property",
            Self::Rel => "rel",
            Self::Role => "role",
            Self::RowSpan => "rowspan",
            Self::Rules => "rules",
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
}
