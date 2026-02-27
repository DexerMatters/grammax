use crate::parsec::tree::Tag;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Creates a node template identified by a command-local id.
    ///
    /// Nodes are assembled bottom-up, so children should be created before
    /// their parent references them.
    CreateNode {
        node_id: u64,
        tag: Tag,
        width: usize,
        token_text: Option<String>,
        children: Vec<u64>,
    },

    /// Deletes the node currently located at a path.
    DeleteNodeAtPath { path: NodePath },

    /// Inserts a previously created node at a stable path.
    ///
    /// For replacement updates, emit a `DeleteNodeAtPath` followed by this.
    InsertNodeAtPath {
        path: NodePath,
        node_id: u64,
        cascade_to_root: bool,
    },
}
