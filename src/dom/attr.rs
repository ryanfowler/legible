use html5ever::QualName;
use tendril::StrTendril;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum AttrName {
    Align,
    AriaHidden,
    AriaModal,
    Background,
    BgColor,
    Border,
    CellPadding,
    CellSpacing,
    Class,
    ColSpan,
    Content,
    DataSrc,
    DataSrcset,
    DataTable,
    Dir,
    Frame,
    Height,
    Hidden,
    Href,
    HSpace,
    Id,
    ItemProp,
    Lang,
    Name,
    Poster,
    Property,
    Rel,
    Role,
    RowSpan,
    Rules,
    Src,
    Srcset,
    Style,
    Summary,
    Type,
    VAlign,
    VSpace,
    Width,
    Other,
}
impl AttrName {
    pub(crate) fn from_local(s: &str) -> Self {
        // html5ever normalizes HTML attribute names to lowercase. Avoid an
        // allocation for that hot path, but keep case-insensitive behavior for
        // qualified names supplied by callers.
        let lowercase;
        let s = if s.bytes().any(|byte| byte.is_ascii_uppercase()) {
            lowercase = s.to_ascii_lowercase();
            lowercase.as_str()
        } else {
            s
        };
        match s {
            "align" => Self::Align,
            "aria-hidden" => Self::AriaHidden,
            "aria-modal" => Self::AriaModal,
            "background" => Self::Background,
            "bgcolor" => Self::BgColor,
            "border" => Self::Border,
            "cellpadding" => Self::CellPadding,
            "cellspacing" => Self::CellSpacing,
            "class" => Self::Class,
            "colspan" => Self::ColSpan,
            "content" => Self::Content,
            "data-src" => Self::DataSrc,
            "data-srcset" => Self::DataSrcset,
            "datatable" => Self::DataTable,
            "dir" => Self::Dir,
            "frame" => Self::Frame,
            "height" => Self::Height,
            "hidden" => Self::Hidden,
            "href" => Self::Href,
            "hspace" => Self::HSpace,
            "id" => Self::Id,
            "itemprop" => Self::ItemProp,
            "lang" => Self::Lang,
            "name" => Self::Name,
            "poster" => Self::Poster,
            "property" => Self::Property,
            "rel" => Self::Rel,
            "role" => Self::Role,
            "rowspan" => Self::RowSpan,
            "rules" => Self::Rules,
            "src" => Self::Src,
            "srcset" => Self::Srcset,
            "style" => Self::Style,
            "summary" => Self::Summary,
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
            Self::AriaModal => "aria-modal",
            Self::Background => "background",
            Self::BgColor => "bgcolor",
            Self::Border => "border",
            Self::CellPadding => "cellpadding",
            Self::CellSpacing => "cellspacing",
            Self::Class => "class",
            Self::ColSpan => "colspan",
            Self::Content => "content",
            Self::DataSrc => "data-src",
            Self::DataSrcset => "data-srcset",
            Self::DataTable => "datatable",
            Self::Dir => "dir",
            Self::Frame => "frame",
            Self::Height => "height",
            Self::Hidden => "hidden",
            Self::Href => "href",
            Self::HSpace => "hspace",
            Self::Id => "id",
            Self::ItemProp => "itemprop",
            Self::Lang => "lang",
            Self::Name => "name",
            Self::Poster => "poster",
            Self::Property => "property",
            Self::Rel => "rel",
            Self::Role => "role",
            Self::RowSpan => "rowspan",
            Self::Rules => "rules",
            Self::Src => "src",
            Self::Srcset => "srcset",
            Self::Style => "style",
            Self::Summary => "summary",
            Self::Type => "type",
            Self::VAlign => "valign",
            Self::VSpace => "vspace",
            Self::Width => "width",
            Self::Other => "",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Attribute {
    pub(crate) name: QualName,
    pub(crate) known: AttrName,
    pub(crate) value: StrTendril,
}
