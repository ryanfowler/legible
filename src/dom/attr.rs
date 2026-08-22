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
    #[inline(always)]
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
    pub(crate) name: u32,
    local: html5ever::LocalName,
    pub(crate) value: tendril::StrTendril,
    kind: AttrName,
}

pub(crate) const QUALIFIED_NAME_FLAG: u32 = 1 << 31;

impl Attribute {
    pub(crate) fn known_with_local(
        kind: AttrName,
        local: html5ever::LocalName,
        value: tendril::StrTendril,
    ) -> Self {
        Self {
            name: kind as u32,
            local,
            value,
            kind,
        }
    }
    #[inline]
    pub(crate) fn new(
        name: html5ever::QualName,
        value: tendril::StrTendril,
        qualified_names: &mut Vec<Option<html5ever::QualName>>,
        free_names: &mut Vec<usize>,
    ) -> Self {
        let kind = AttrName::from_local(name.local.as_ref());
        if kind != AttrName::Other && name.ns == html5ever::ns!() && name.prefix.is_none() {
            return Self {
                name: kind as u32,
                local: name.local,
                value,
                kind,
            };
        }
        let local = name.local.clone();
        let index = free_names.pop().unwrap_or(qualified_names.len());
        if index == qualified_names.len() {
            qualified_names.push(Some(name));
        } else {
            qualified_names[index] = Some(name);
        }
        Self {
            name: QUALIFIED_NAME_FLAG | index as u32,
            local,
            value,
            kind,
        }
    }

    #[inline]
    pub(crate) fn is_named(&self, name: AttrName) -> bool {
        self.kind == name
    }

    #[inline]
    pub(crate) fn qualified_name(
        &self,
        qualified_names: &[Option<html5ever::QualName>],
    ) -> html5ever::QualName {
        if self.name & QUALIFIED_NAME_FLAG != 0 {
            qualified_names[(self.name & !QUALIFIED_NAME_FLAG) as usize]
                .as_ref()
                .expect("live qualified attribute name")
                .clone()
        } else {
            html5ever::QualName::new(None, html5ever::ns!(), self.local.clone())
        }
    }

    #[inline]
    pub(crate) fn qualified_name_index(&self) -> Option<usize> {
        (self.name & QUALIFIED_NAME_FLAG != 0)
            .then_some((self.name & !QUALIFIED_NAME_FLAG) as usize)
    }

    #[inline]
    pub(crate) fn known_kind(&self) -> AttrName {
        self.kind
    }

    #[inline]
    pub(crate) fn local_name(&self) -> &str {
        self.local.as_ref()
    }
}
