use super::{Dom, NodeId, Tag};
impl Dom {
    pub(crate) fn first_descendant_by_tag(&self, root: NodeId, tag: Tag) -> Option<NodeId> {
        self.descendants(root).find(|&id| self.tag(id) == Some(tag))
    }
    pub(crate) fn any_descendant_by_tags(&self, root: NodeId, tags: &[Tag]) -> bool {
        self.descendants(root)
            .any(|id| self.tag(id).is_some_and(|t| tags.contains(&t)))
    }
}
