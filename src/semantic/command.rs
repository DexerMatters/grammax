/// Semantic ID uniquely identifies semantic nodes in the tree
pub type SemanticId = usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Create a new semantic node with the given rule name
    Create(SemanticId, String),

    /// Create a new token node with the given token value
    CreateToken(SemanticId, String),

    /// Replace old semantic node with new semantic node (rebuild due to content change)
    /// Replace(old_id, new_id)
    Replace(SemanticId, SemanticId),

    /// Delete a semantic node
    Delete(SemanticId),

    /// Insert a child node into a parent
    /// Insert(parent_id, child_id)
    Insert(SemanticId, SemanticId),
}
