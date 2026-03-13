use std::{
    any::{Any, TypeId, type_name},
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    ptr::NonNull,
};

use crate::scheme;

pub struct ASTCell<T> {
    pub(crate) raw: usize,
    pub(crate) arena: Option<NonNull<()>>,
    pub(crate) arena_ty: Option<&'static str>,
    pub(crate) _marker: PhantomData<fn() -> T>,
}

impl<T> Copy for ASTCell<T> {}

impl<T> Clone for ASTCell<T> {
    fn clone(&self) -> Self {
        *self
    }
}

// SAFETY: `ASTCell<T>` is just a stable index + a raw arena pointer used only
// for `Debug` formatting. The arena is always owned by the same owner that
// hands out the cell, so sending the cell between threads is safe as long as
// `T` itself is `Send`.
unsafe impl<T: Send> Send for ASTCell<T> {}

impl<T> fmt::Debug for ASTCell<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(arena) = self.arena {
            // SAFETY: `arena` comes from `AstArena`; node slots are stable.
            let arena = unsafe { &*arena.cast::<AstArenaStorage>().as_ptr() };
            if let Some(node) = arena.nodes.get(self.raw).and_then(|slot| slot.as_ref()) {
                return node.fmt_value(f);
            }
        }
        f.debug_tuple("ASTCell").field(&self.raw).finish()
    }
}

impl<T> PartialEq for ASTCell<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw && self.arena == other.arena
    }
}

impl<T> Eq for ASTCell<T> {}

impl<T> Hash for ASTCell<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
        self.arena.hash(state);
    }
}

impl<T> PartialOrd for ASTCell<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for ASTCell<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.arena
            .cmp(&other.arena)
            .then_with(|| self.raw.cmp(&other.raw))
    }
}

impl<T> ASTCell<T> {
    pub const fn new(raw: usize) -> Self {
        Self {
            raw,
            arena: None,
            arena_ty: None,
            _marker: PhantomData,
        }
    }

    pub const fn into_raw(self) -> usize {
        self.raw
    }

    pub(crate) const fn cast<U>(self) -> ASTCell<U> {
        ASTCell {
            raw: self.raw,
            arena: self.arena,
            arena_ty: self.arena_ty,
            _marker: PhantomData,
        }
    }
}

pub type AstDelta<T> = Vec<scheme::Command<AstArena<T>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstArenaError {
    MissingIndex {
        index: usize,
    },
    TypeMismatch {
        index: usize,
        expected: &'static str,
    },
}

#[derive(Debug)]
pub struct AstArena<T> {
    pub(crate) storage: Box<AstArenaStorage>,
    _marker: PhantomData<fn() -> T>,
}

#[derive(Debug, Clone)]
pub(crate) struct AstArenaStorage {
    pub(crate) nodes: Vec<Option<ErasedAstNode>>,
    pub(crate) free: Vec<usize>,
}

impl<T> Clone for AstArena<T> {
    fn clone(&self) -> Self {
        Self {
            storage: Box::new((*self.storage).clone()),
            _marker: PhantomData,
        }
    }
}

pub(crate) struct ErasedAstNode {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
    pub(crate) value: Box<dyn Any + Send>,
    pub(crate) clone_fn: fn(&Box<dyn Any + Send>) -> Box<dyn Any + Send>,
    pub(crate) eq_fn: fn(&Box<dyn Any + Send>, &Box<dyn Any + Send>) -> bool,
    pub(crate) debug_fn: fn(&Box<dyn Any + Send>, &mut fmt::Formatter<'_>) -> fmt::Result,
}

impl fmt::Debug for ErasedAstNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErasedAstNode")
            .field("type_name", &self.type_name)
            .finish()
    }
}

impl Clone for ErasedAstNode {
    fn clone(&self) -> Self {
        Self {
            type_id: self.type_id,
            type_name: self.type_name,
            value: (self.clone_fn)(&self.value),
            clone_fn: self.clone_fn,
            eq_fn: self.eq_fn,
            debug_fn: self.debug_fn,
        }
    }
}

impl ErasedAstNode {
    pub(crate) fn new<U>(value: U) -> Self
    where
        U: fmt::Debug + Clone + PartialEq + Send + 'static,
    {
        fn clone_impl<U: Clone + Send + 'static>(
            value: &Box<dyn Any + Send>,
        ) -> Box<dyn Any + Send> {
            Box::new(
                value
                    .downcast_ref::<U>()
                    .expect("stored erased node must match clone type")
                    .clone(),
            )
        }
        fn eq_impl<U: PartialEq + Send + 'static>(
            lhs: &Box<dyn Any + Send>,
            rhs: &Box<dyn Any + Send>,
        ) -> bool {
            lhs.downcast_ref::<U>() == rhs.downcast_ref::<U>()
        }
        fn debug_impl<U: fmt::Debug + Send + 'static>(
            value: &Box<dyn Any + Send>,
            f: &mut fmt::Formatter<'_>,
        ) -> fmt::Result {
            value
                .downcast_ref::<U>()
                .expect("stored erased node must match debug type")
                .fmt(f)
        }

        Self {
            type_id: TypeId::of::<U>(),
            type_name: type_name::<U>(),
            value: Box::new(value),
            clone_fn: clone_impl::<U>,
            eq_fn: eq_impl::<U>,
            debug_fn: debug_impl::<U>,
        }
    }

    pub(crate) fn downcast_ref<U: 'static>(&self) -> Option<&U> {
        self.value.downcast_ref::<U>()
    }

    pub(crate) fn into_downcast<U: Send + 'static>(self) -> Option<U> {
        self.value.downcast::<U>().ok().map(|value| *value)
    }

    pub(crate) fn fmt_value(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (self.debug_fn)(&self.value, f)
    }
}

impl<T> Default for AstArena<T> {
    fn default() -> Self {
        Self {
            storage: Box::new(AstArenaStorage {
                nodes: Vec::new(),
                free: Vec::new(),
            }),
            _marker: PhantomData,
        }
    }
}

impl<T> AstArena<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<U>(&mut self, node: U) -> ASTCell<U>
    where
        U: fmt::Debug + Clone + PartialEq + Send + 'static,
    {
        self.insert_erased(ErasedAstNode::new(node)).cast()
    }

    pub fn set<U>(&mut self, id: ASTCell<U>, node: U)
    where
        U: fmt::Debug + Clone + PartialEq + Send + 'static,
    {
        self.set_erased(id.cast(), ErasedAstNode::new(node));
    }

    pub fn remove<U>(&mut self, id: ASTCell<U>) -> Option<U>
    where
        U: Send + 'static,
    {
        self.remove_erased(id.cast())
            .and_then(ErasedAstNode::into_downcast)
    }

    pub fn get<U>(&self, id: ASTCell<U>) -> Option<&U>
    where
        U: 'static,
    {
        self.get_erased(id.cast())
            .and_then(ErasedAstNode::downcast_ref)
    }

    // ── crate-internal helpers used by IncrementalLowerer ──────────────────

    pub(crate) fn insert_erased(&mut self, node: ErasedAstNode) -> ASTCell<()> {
        let node_ty = node.type_name;
        if let Some(id) = self.storage.free.pop() {
            self.storage.nodes[id] = Some(node);
            return self.cell(id, node_ty);
        }
        let id = self.storage.nodes.len();
        self.storage.nodes.push(Some(node));
        self.cell(id, node_ty)
    }

    pub(crate) fn set_erased(&mut self, id: ASTCell<()>, node: ErasedAstNode) {
        let raw = id.into_raw();
        if raw >= self.storage.nodes.len() {
            self.storage.nodes.resize_with(raw + 1, || None);
        }
        self.storage.nodes[raw] = Some(node);
    }

    pub(crate) fn remove_erased(&mut self, id: ASTCell<()>) -> Option<ErasedAstNode> {
        let raw = id.into_raw();
        if raw >= self.storage.nodes.len() {
            return None;
        }
        let prev = self.storage.nodes[raw].take();
        if prev.is_some() {
            self.storage.free.push(raw);
        }
        prev
    }

    pub(crate) fn get_erased(&self, id: ASTCell<()>) -> Option<&ErasedAstNode> {
        self.storage
            .nodes
            .get(id.into_raw())
            .and_then(|slot| slot.as_ref())
    }

    fn cell<U>(&self, raw: usize, node_ty: &'static str) -> ASTCell<U> {
        ASTCell {
            raw,
            arena: Some(self.storage_ptr()),
            arena_ty: Some(node_ty),
            _marker: PhantomData,
        }
    }

    fn storage_ptr(&self) -> NonNull<()> {
        NonNull::from(self.storage.as_ref()).cast()
    }

    /// Force-allocate an erased node at a specific raw index (used by IR impl).
    pub(crate) fn force_alloc_at(&mut self, index: usize, node: ErasedAstNode) {
        if index >= self.storage.nodes.len() {
            self.storage.nodes.resize_with(index + 1, || None);
        }
        self.storage.free.retain(|&i| i != index);
        self.storage.nodes[index] = Some(node);
    }
}

impl<T: fmt::Debug + Clone + PartialEq + Send + 'static> scheme::IR for AstArena<T> {
    type Ix = usize;
    type Value = T;
    type Error = AstArenaError;

    fn query(&self, index: usize) -> Result<T, Self::Error> {
        let Some(node) = self.get_erased(ASTCell::<()>::new(index)) else {
            return Err(AstArenaError::MissingIndex { index });
        };

        node.downcast_ref::<T>()
            .cloned()
            .ok_or(AstArenaError::TypeMismatch {
                index,
                expected: type_name::<T>(),
            })
    }
    fn apply_transaction(&mut self, txn: scheme::Transaction<Self>) -> Result<(), Self::Error> {
        let mut staging: Vec<Option<ErasedAstNode>> = Vec::new();
        for cmd in txn.iter() {
            match cmd {
                scheme::Command::Create { id, value } => {
                    if *id >= staging.len() {
                        staging.resize_with(*id + 1, || None);
                    }
                    staging[*id] = Some(ErasedAstNode::new(value.clone()));
                }
                scheme::Command::Insert { index, id } => {
                    if let Some(slot) = staging.get_mut(*id) {
                        if let Some(node) = slot.take() {
                            self.force_alloc_at(*index, node);
                        }
                    }
                }
                scheme::Command::Replace { index, id } => {
                    if let Some(slot) = staging.get_mut(*id) {
                        if let Some(node) = slot.take() {
                            self.set_erased(ASTCell::<()>::new(*index), node);
                        }
                    }
                }
                scheme::Command::Delete { index } => {
                    self.remove_erased(ASTCell::<()>::new(*index));
                }
                scheme::Command::SetRoot { .. } => {}
            }
        }
        Ok(())
    }
}
