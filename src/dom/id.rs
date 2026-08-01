use std::fmt;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct NodeId(pub(crate) u32);

impl NodeId {
    #[inline]
    pub(crate) const fn index(self) -> usize {
        self.0 as usize
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NodeLink(pub(crate) u32);

impl NodeLink {
    pub(crate) const NONE: Self = Self(u32::MAX);
    #[inline]
    pub(crate) const fn from_option(id: Option<NodeId>) -> Self {
        match id {
            Some(id) => Self(id.0),
            None => Self::NONE,
        }
    }
    #[inline]
    pub(crate) const fn get(self) -> Option<NodeId> {
        if self.0 == u32::MAX {
            None
        } else {
            Some(NodeId(self.0))
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DomError(pub(crate) String);
impl fmt::Display for DomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DomError {}
