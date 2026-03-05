use serde::Serialize;

use crate::parsec::tree::ParsecError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize)]
pub struct NodePath(pub Vec<usize>);

impl NodePath {
    pub fn root() -> Self {
        Self(Vec::new())
    }

    pub fn parent(&self) -> Option<Self> {
        if self.0.is_empty() {
            None
        } else {
            let mut path = self.0.clone();
            path.pop();
            Some(Self(path))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PathTargetKind {
    Node,
    Leaf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum Command {
    /// Creates a token leaf node with text content.
    ///
    /// Tokens are terminal nodes that carry the actual source text.
    /// Field name can be empty string for direct tokens, or contain the field identifier.
    CreateToken {
        node_id: u64,
        rule_ix: usize,
        text: String,
        field: String,
    },

    /// Creates an error node representing a parser error.
    ///
    /// Used when parsing fails but recovery allows continuation.
    CreateError {
        node_id: u64,
        kind: ParsecError,
        text: String,
        field: String,
    },

    /// Creates an internal node with children references.
    ///
    /// Nodes are assembled bottom-up, so children should be created before
    /// their parent references them.
    CreateNode {
        node_id: u64,
        rule_ix: usize,
        children: Vec<u64>,
        field: String,
    },

    /// Deletes the node currently located at a path.
    DeleteNodeAtPath { path: NodePath },

    /// Replaces the node currently located at a path with a previously created node.
    ///
    /// This is used for direct substitutions (including leaf token/error replacement)
    /// where delete+insert pairs would be ambiguous for downstream consumers.
    ReplaceNodeAtPath {
        path: NodePath,
        node_id: u64,
        target_kind: PathTargetKind,
    },

    /// Inserts a previously created node at a stable path.
    ///
    /// For replacement updates, emit a `DeleteNodeAtPath` followed by this.
    InsertNodeAtPath {
        path: NodePath,
        node_id: u64,
        cascade_to_root: bool,
    },
}
