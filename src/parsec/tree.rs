use rustc_hash::FxHashMap;
use std::cell::{self, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

/// Error types that can occur during parsing, used for error reporting and recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParsecError {
    Incomplete,
    UnexpectedToken,
    MissingToken,
    Placeholder,
    LRError, // Added for LR parser
}

/// A type alias for the identifier of a green node in the syntax tree. This is used to reference nodes in the tree allocator.
pub type GreenId = usize;

/// Tags indicating the type of a syntax tree node, such as whether it's a rule, token, field, or an error.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tag {
    Rule { rule_ix: usize },
    Token { rule_ix: usize }, // Usually terminal_idx
    Field { rule_ix: usize, name: &'static str },
    Error(Vec<ParsecError>),
    Root,
}

impl Tag {
    pub fn new_rule(rule_ix: usize) -> Self {
        Tag::Rule { rule_ix }
    }
    pub fn new_token(rule_ix: usize) -> Self {
        Tag::Token { rule_ix }
    }
    pub fn new_field(rule_ix: usize, name: &'static str) -> Self {
        Tag::Field { rule_ix, name }
    }
    pub fn new_error(err: ParsecError) -> Self {
        Tag::Error(vec![err])
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Tag::Error(_))
    }
}

/// Red nodes are the nodes in the "red" syntax tree,
/// which includes parent references and offsets for easier traversal and error reporting.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RedNode {
    pub parent: Option<Rc<RedNode>>,
    pub offset: usize,
    pub green: GreenId,
}

impl RedNode {
    /// Creates a root red node with the given green node ID.
    /// Its parent is None and offset is 0.
    pub fn root(green: GreenId) -> Self {
        Self {
            parent: None,
            offset: 0,
            green,
        }
    }
}

/// Green nodes are the nodes in the "green" syntax tree, which are immutable and can be shared.
/// They contain the tag, width, and children references, and are interned for memory efficiency.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GreenNode {
    pub tag: Tag,
    pub width: usize,
    pub children: Vec<GreenId>,
}

/// The tree allocator is responsible for managing the allocation and deduplication of green nodes.
pub struct TreeAlloc {
    nodes: Vec<GreenNode>,
    dedup: FxHashMap<u64, Vec<usize>>,
}

/// A shared reference to the tree allocator.
pub type TreeAllocRef = Rc<RefCell<TreeAlloc>>;

/// A trait that extends the functionality of the tree allocator reference, providing methods for creating and managing green nodes.
pub trait TreeAllocRefExt {
    /// Creates a new tree allocator reference.
    fn create() -> Self;
    /// Retrieves a reference to a green node by its ID.
    fn get_node(&self, id: GreenId) -> cell::Ref<'_, GreenNode>;
    /// Allocates a new token node with the given tag and width, returning its ID.
    fn alloc_token(&self, tag: Tag, width: usize) -> GreenId;
    /// Allocates a new node with the given tag, children, and width, returning its ID.
    fn alloc(&self, tag: Tag, children: Vec<GreenId>, width: usize) -> GreenId;
    /// Creates a new placeholder node with the given width, used for error recovery.
    fn new_placeholder(&self, width: usize) -> GreenId;
}

impl TreeAllocRefExt for TreeAllocRef {
    fn create() -> Self {
        Rc::new(RefCell::new(TreeAlloc {
            nodes: Vec::new(),
            dedup: FxHashMap::default(),
        }))
    }

    fn get_node(&self, id: GreenId) -> cell::Ref<'_, GreenNode> {
        cell::Ref::map(self.borrow(), |alloc| &alloc.nodes[id])
    }

    fn alloc_token(&self, tag: Tag, width: usize) -> GreenId {
        self.alloc(tag, vec![], width)
    }

    fn alloc(&self, tag: Tag, children: Vec<GreenId>, width: usize) -> GreenId {
        let node = GreenNode {
            tag,
            children,
            width,
        };

        // Deduplicate only errors. Tokens carry user text via source spans, so they
        // should remain distinct per allocation.
        let should_dedup = matches!(node.tag, Tag::Error(_));
        if should_dedup {
            let mut hasher = DefaultHasher::new();
            node.hash(&mut hasher);
            let hash = hasher.finish();

            {
                let borrowed = self.borrow();
                if let Some(indices) = borrowed.dedup.get(&hash) {
                    for &idx in indices {
                        if borrowed.nodes[idx] == node {
                            return idx;
                        }
                    }
                }
            }

            let mut borrowed = self.borrow_mut();
            let idx = borrowed.nodes.len();
            borrowed.nodes.push(node);
            borrowed.dedup.entry(hash).or_default().push(idx);
            idx
        } else {
            let mut borrowed = self.borrow_mut();
            let idx = borrowed.nodes.len();
            borrowed.nodes.push(node);
            idx
        }
    }

    fn new_placeholder(&self, width: usize) -> GreenId {
        self.alloc(Tag::Error(vec![ParsecError::Placeholder]), vec![], width)
    }
}
