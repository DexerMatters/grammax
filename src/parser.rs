use std::sync::Arc;

use concurrent_queue::ConcurrentQueue;

use crate::tree::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Update { span: Span, new_text: String },
    Insert { position: usize, new_text: String },
    Delete { span: Span },
}

struct GlobalState {
    queue: Arc<ConcurrentQueue<Edit>>,
    arena: TreeArena,
    ast: RedNode,
}

impl GlobalState {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(ConcurrentQueue::unbounded()),
            arena: Arc::new(TreeAlloc::new()),
            ast: RedNode {
                parent: None,
                span: Span { start: 0, end: 0 },
                data: 0,
            },
        }
    }
}
