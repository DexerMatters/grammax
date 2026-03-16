/// Path-based AST arena storage and transaction support.
///
/// The new design:
/// - Arena stores type-erased nodes indexed by NodePath (tree structure)
/// - Transactions operate on paths, enabling incremental updates
/// - Commands use paths, not flat slot indices
/// - Direct path-based storage via BTreeMap
use std::{
    any::{Any, TypeId, type_name},
    cell::Cell,
    collections::BTreeMap,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    ptr::NonNull,
};

use crate::scheme;
use crate::scheme::layers::NodePath;

thread_local! {
    static AST_CELL_CLONE_ARENA: Cell<Option<NonNull<()>>> = const { Cell::new(None) };
}

pub struct AstCell<T> {
    pub(crate) path: NodePath,
    pub(crate) arena: Option<NonNull<()>>,
    pub(crate) arena_ty: Option<&'static str>,
    pub(crate) _marker: PhantomData<fn() -> T>,
}
impl<T> Clone for AstCell<T> {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            arena: self
                .arena
                .or_else(|| AST_CELL_CLONE_ARENA.with(|current| current.get())),
            arena_ty: self.arena_ty,
            _marker: PhantomData,
        }
    }
}

unsafe impl<T: Send> Send for AstCell<T> {}
unsafe impl<T: Sync> Sync for AstCell<T> {}

impl<T: fmt::Debug + 'static> fmt::Debug for AstCell<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arena = self
            .arena
            .or_else(|| AST_CELL_CLONE_ARENA.with(|current| current.get()));

        if let Some(value) = self.deref_debug_value(arena) {
            return AST_CELL_CLONE_ARENA.with(|current| {
                let prev = current.replace(arena);
                let result = value.fmt(f);
                current.set(prev);
                result
            });
        }

        f.debug_struct("AstCell")
            .field("path", &self.path)
            .field("type", &self.arena_ty)
            .finish()
    }
}

impl<T> PartialEq for AstCell<T> {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.arena == other.arena
    }
}

impl<T> Eq for AstCell<T> {}

impl<T> Hash for AstCell<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.arena.hash(state);
    }
}

impl<T> PartialOrd for AstCell<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for AstCell<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.arena
            .cmp(&other.arena)
            .then_with(|| self.path.cmp(&other.path))
    }
}

impl<T> AstCell<T> {
    pub fn new(path: NodePath) -> Self {
        Self {
            path,
            arena: None,
            arena_ty: None,
            _marker: PhantomData,
        }
    }

    pub fn from_path(path: &NodePath) -> Self {
        Self::new(path.clone())
    }

    pub fn path(&self) -> &NodePath {
        &self.path
    }

    pub fn get<'a>(&self, arena: &'a AstArena<T>) -> Option<&'a T>
    where
        T: 'static,
    {
        arena.get(&self.path)
    }

    pub fn cloned(&self, arena: &AstArena<T>) -> Option<T>
    where
        T: Clone + 'static,
    {
        self.get(arena).cloned()
    }

    fn deref_debug_value(&self, arena: Option<NonNull<()>>) -> Option<&T>
    where
        T: fmt::Debug + 'static,
    {
        let storage = arena?.cast::<AstArenaStorage>();
        // SAFETY: `arena` is only produced from `AstArena::storage_ptr()` and the
        // storage outlives queried values for the duration of formatting.
        let storage = unsafe { storage.as_ref() };
        storage
            .nodes
            .get(&self.path)
            .and_then(ErasedAstNode::downcast_ref::<T>)
    }

    /// Cast the phantom type parameter to `U`.
    ///
    /// The cell itself is just a `NodePath`; the type parameter is only a
    /// compile-time hint for typed retrieval.  Use this when the mapper
    /// operates in heterogeneous mode (`AstMapAny`) but the stored value
    /// is known to be of type `U` at the call site.
    pub fn cast<U>(self) -> AstCell<U> {
        AstCell {
            path: self.path,
            arena: self.arena,
            arena_ty: self.arena_ty,
            _marker: PhantomData,
        }
    }
}

impl<T> serde::Serialize for AstCell<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.path.serialize(serializer)
    }
}

impl<'de, T> serde::Deserialize<'de> for AstCell<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let path = NodePath::deserialize(deserializer)?;
        Ok(Self::new(path))
    }
}

pub type AstDelta<T> = Vec<scheme::Command<AstArena<T>>>;

// ── AstMapAny ─────────────────────────────────────────────────────────────────

/// A type-erased AST value produced by a heterogeneous `AstMapper`.
///
/// `AstMapper` visitors can emit values of any concrete type
/// (`Expr`, `Type`, …) via [`AstMapCtx::emit`].  All emitted values are
/// stored as `AstMapAny` inside the downstream `AstArena<AstMapAny>`.
///
/// Retrieve a typed reference at query time with [`AstMapAny::downcast_ref`],
/// or let the observer return the raw `AstMapAny` (its `Debug` impl
/// transparently delegates to the inner value's formatter).
pub struct AstMapAny(pub(crate) ErasedAstNode);

impl AstMapAny {
    /// Wrap any owned value in `AstMapAny`.
    pub fn new<T>(value: T) -> Self
    where
        T: fmt::Debug + Clone + PartialEq + Send + 'static,
    {
        AstMapAny(ErasedAstNode::new(value))
    }

    /// Return a reference to the inner value if it is of type `T`.
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }

    /// Consume the wrapper and return the inner value as `T`.
    pub fn downcast<T: Send + 'static>(self) -> Option<T> {
        self.0.into_downcast()
    }

    /// Name of the concrete stored type (for diagnostics).
    pub fn type_name(&self) -> &'static str {
        self.0.type_name
    }
}

impl fmt::Debug for AstMapAny {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (self.0.debug_fn)(&self.0.value, f)
    }
}

impl Clone for AstMapAny {
    fn clone(&self) -> Self {
        AstMapAny(self.0.clone())
    }
}

impl PartialEq for AstMapAny {
    fn eq(&self, other: &Self) -> bool {
        if self.0.type_id != other.0.type_id {
            return false;
        }
        (self.0.eq_fn)(&self.0.value, &other.0.value)
    }
}

unsafe impl Send for AstMapAny {}
unsafe impl Sync for AstMapAny {}

pub struct AstVec<T> {
    pub(crate) base: NodePath,
    pub(crate) arena: Option<NonNull<()>>,
    pub(crate) _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for AstVec<T> {
    fn clone(&self) -> Self {
        Self {
            base: self.base.clone(),
            arena: self
                .arena
                .or_else(|| AST_CELL_CLONE_ARENA.with(|current| current.get())),
            _marker: PhantomData,
        }
    }
}

/// Two `AstVec`s are equal when they share the same `base` path.
///
/// Element content is intentionally excluded so that the parent node value
/// remains **stable** when children are added or removed, enabling fine-grained
/// incremental updates.
impl<T> PartialEq for AstVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.base == other.base
    }
}

impl<T: fmt::Debug + 'static> fmt::Debug for AstVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arena_ptr = self
            .arena
            .or_else(|| AST_CELL_CLONE_ARENA.with(|current| current.get()));

        if let Some(ptr) = arena_ptr {
            let mut list = f.debug_list();
            // SAFETY: arena pointer is valid for the duration of formatting,
            // same guarantee as in AstCell::deref_debug_value.
            let storage = unsafe { ptr.cast::<AstArenaStorage>().as_ref() };
            for (path, node) in storage.iter_mapped_children(&self.base) {
                if let Some(val) = node.downcast_ref::<T>() {
                    AST_CELL_CLONE_ARENA.with(|current| {
                        let prev = current.replace(Some(ptr));
                        list.entry(val);
                        current.set(prev);
                    });
                } else {
                    list.entry(&format_args!("<type mismatch at {:?}>", path));
                }
            }
            list.finish()
        } else {
            // No arena available: show the base path instead of empty list
            f.debug_struct("AstVec").field("base", &self.base).finish()
        }
    }
}

unsafe impl<T: Send> Send for AstVec<T> {}
unsafe impl<T: Sync> Sync for AstVec<T> {}

impl<T> AstVec<T> {
    /// Create a new `AstVec` rooted at `base`.
    ///
    /// Prefer using [`AstMapCtx::collect_vec`] inside a mapper handler, which
    /// supplies the correct base path automatically.
    pub fn new(base: NodePath) -> Self {
        Self {
            base,
            arena: None,
            _marker: PhantomData,
        }
    }

    /// The base path — direct children of this path in the arena are the
    /// vector's elements.
    pub fn base(&self) -> &NodePath {
        &self.base
    }

    /// Return typed cells for every element currently in the arena.
    ///
    /// Each element must have been stored by an `AstMapper` handler that emits
    /// a value of type `T` at a direct-child path of the base.
    pub fn cells(&self) -> Vec<AstCell<T>> {
        let arena_ptr = self
            .arena
            .or_else(|| AST_CELL_CLONE_ARENA.with(|current| current.get()));

        let Some(ptr) = arena_ptr else {
            return Vec::new();
        };
        // SAFETY: same guarantee as AstCell::deref_debug_value.
        let storage = unsafe { ptr.cast::<AstArenaStorage>().as_ref() };
        storage
            .iter_mapped_children(&self.base)
            .into_iter()
            .map(|(path, node)| AstCell {
                path,
                arena: Some(ptr),
                arena_ty: Some(node.type_name),
                _marker: PhantomData,
            })
            .collect()
    }

    /// Iterate resolved values using an explicit arena reference.
    ///
    /// Prefer this over [`cells`] when you already have the arena at hand and
    /// want to avoid collecting a temporary `Vec`.
    pub fn iter_in<'a>(&self, arena: &'a AstArena<T>) -> impl Iterator<Item = &'a T>
    where
        T: 'static,
    {
        let paths: Vec<NodePath> = arena
            .storage
            .iter_mapped_children(&self.base)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        paths.into_iter().filter_map(move |p| arena.get(&p))
    }

    /// Number of elements currently in the arena under this base.
    ///
    /// Requires an arena pointer to be populated (e.g. inside a Debug context
    /// or after retrieval from the arena via `query`).
    pub fn len_in(&self, arena: &AstArena<T>) -> usize
    where
        T: 'static,
    {
        arena.storage.iter_mapped_children(&self.base).len()
    }
}

impl<T> serde::Serialize for AstVec<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.base.serialize(serializer)
    }
}

pub(crate) struct AstTxnBuilder<T>
where
    T: fmt::Debug + Clone + PartialEq + Send + 'static,
{
    ops: AstDelta<T>,
    next_staging_id: usize,
}

impl<T> Default for AstTxnBuilder<T>
where
    T: fmt::Debug + Clone + PartialEq + Send + 'static,
{
    fn default() -> Self {
        Self {
            ops: Vec::new(),
            next_staging_id: 0,
        }
    }
}

impl<T> AstTxnBuilder<T>
where
    T: fmt::Debug + Clone + PartialEq + Send + 'static,
{
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn stage(&mut self, value: T) -> usize {
        let id = self.next_staging_id;
        self.next_staging_id += 1;
        self.ops.push(scheme::Command::Create { id, value });
        id
    }

    /// Stage a value and insert it at the given path
    pub(crate) fn insert_value(&mut self, path: NodePath, value: T) {
        let id = self.stage(value);
        self.ops.push(scheme::Command::Insert { index: path, id });
    }

    /// Stage a value and replace the existing node at the given path
    pub(crate) fn replace_value(&mut self, path: NodePath, value: T) {
        let id = self.stage(value);
        self.ops.push(scheme::Command::Replace { index: path, id });
    }

    /// Delete the node at the given path
    pub(crate) fn delete(&mut self, path: NodePath) {
        self.ops.push(scheme::Command::Delete { index: path });
    }

    pub(crate) fn finish(self) -> AstDelta<T> {
        self.ops
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstArenaError {
    MissingPath {
        path: NodePath,
    },
    TypeMismatch {
        path: NodePath,
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
    /// Path-based storage for all AST nodes
    pub(crate) nodes: BTreeMap<NodePath, ErasedAstNode>,
    pub(crate) root: Option<NodePath>,
}

impl AstArenaStorage {
    /// Return the "mapped children" of `base` — all arena paths that are
    /// descendants of `base` but **not** descendants of any other mapped
    /// ancestor between `base` and themselves.
    ///
    /// Concretely, this finds the shallowest set of paths P where:
    /// - `base` is a strict prefix of P (P is a descendant of base)
    /// - no other path Q in the arena satisfies `base ⊂ Q ⊂ P` as prefixes
    ///
    /// This handles `sep`-desugared grammars where list elements are at
    /// non-uniform depths (e.g. `P.1.0`, `P.2.1.0`, `P.2.2.1.0`, …) but
    /// are still logically "one level" below the containing list node.
    pub(crate) fn iter_mapped_children(&self, base: &NodePath) -> Vec<(NodePath, &ErasedAstNode)> {
        let start = base.child(0);
        let mut result = Vec::new();
        let mut last_included: Option<NodePath> = None;
        for (path, node) in self
            .nodes
            .range(start..)
            .take_while(|(p, _)| base.is_prefix_of(p))
        {
            // Skip paths that are descendants of an already-included mapped node.
            if let Some(ref last) = last_included {
                if last.is_prefix_of(path) {
                    continue;
                }
            }
            last_included = Some(path.clone());
            result.push((path.clone(), node));
        }
        result
    }
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
}

impl<T> Default for AstArena<T> {
    fn default() -> Self {
        Self {
            storage: Box::new(AstArenaStorage {
                nodes: BTreeMap::new(),
                root: None,
            }),
            _marker: PhantomData,
        }
    }
}

impl<T> AstArena<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn root_path(&self) -> Option<&NodePath> {
        self.storage.root.as_ref()
    }

    /// Insert a new value at the given path (path must not already exist)
    pub fn insert<U>(&mut self, path: NodePath, node: U) -> AstCell<U>
    where
        U: fmt::Debug + Clone + PartialEq + Send + 'static,
    {
        let cell = self
            .insert_erased(path.clone(), ErasedAstNode::new(node))
            .cast();
        self.refresh_root();
        cell
    }

    /// Update the value at the given path (path must exist)
    pub fn set<U>(&mut self, path: NodePath, node: U)
    where
        U: fmt::Debug + Clone + PartialEq + Send + 'static,
    {
        self.set_erased(path, ErasedAstNode::new(node));
        self.refresh_root();
    }

    /// Remove the node at the given path
    pub fn remove<U>(&mut self, path: NodePath) -> Option<U>
    where
        U: Send + 'static,
    {
        let removed = self
            .remove_erased(path)
            .and_then(ErasedAstNode::into_downcast);
        self.refresh_root();
        removed
    }

    /// Query the node at the given path
    pub fn get<U>(&self, path: &NodePath) -> Option<&U>
    where
        U: 'static,
    {
        self.resolve_path(path)
            .and_then(|path| self.get_erased(path))
            .and_then(ErasedAstNode::downcast_ref)
    }

    pub fn get_by_cell<U>(&self, cell: &AstCell<U>) -> Option<&U>
    where
        U: 'static,
    {
        self.get(cell.path())
    }

    pub fn cloned_by_cell<U>(&self, cell: &AstCell<U>) -> Option<U>
    where
        U: Clone + 'static,
    {
        self.get_by_cell(cell).cloned()
    }

    pub(crate) fn insert_erased(&mut self, path: NodePath, node: ErasedAstNode) -> AstCell<()> {
        self.storage.nodes.insert(path.clone(), node);
        self.cell(path)
    }

    pub(crate) fn set_erased(&mut self, path: NodePath, node: ErasedAstNode) {
        self.storage.nodes.insert(path, node);
    }

    pub(crate) fn remove_erased(&mut self, path: NodePath) -> Option<ErasedAstNode> {
        self.storage.nodes.remove(&path)
    }

    pub(crate) fn get_erased(&self, path: &NodePath) -> Option<&ErasedAstNode> {
        self.storage.nodes.get(path)
    }

    /// Retrieve the stored node wrapped in [`AstMapAny`].
    ///
    /// Unlike `get::<T>()` this always succeeds as long as the path exists —
    /// it simply wraps the underlying [`ErasedAstNode`] without downcasting.
    /// Use it in heterogeneous pipelines where the arena's stored type is
    /// `AstMapAny` and you need to compare or forward the erased value.
    pub fn get_erased_as_any(&self, path: &NodePath) -> Option<AstMapAny> {
        let path = self.resolve_path(path)?;
        self.get_erased(path).map(|n| AstMapAny(n.clone()))
    }

    pub fn get_cell(&self, path: &NodePath) -> Option<AstCell<T>>
    where
        T: 'static,
    {
        let resolved = self.resolve_path(path)?.clone();
        self.get_erased(&resolved).map(|node| AstCell {
            path: resolved,
            arena: Some(self.storage_ptr()),
            arena_ty: Some(node.type_name),
            _marker: PhantomData,
        })
    }

    fn resolve_path<'a>(&'a self, path: &'a NodePath) -> Option<&'a NodePath> {
        if path.0.is_empty() {
            self.storage.root.as_ref()
        } else {
            Some(path)
        }
    }

    fn refresh_root(&mut self) {
        self.storage.root = self
            .storage
            .nodes
            .keys()
            .min_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)))
            .cloned();
    }

    fn cell(&self, path: NodePath) -> AstCell<()> {
        let node_ty = self
            .get_erased(&path)
            .map(|n| n.type_name)
            .unwrap_or("unknown");
        AstCell {
            path,
            arena: Some(self.storage_ptr()),
            arena_ty: Some(node_ty),
            _marker: PhantomData,
        }
    }

    fn storage_ptr(&self) -> NonNull<()> {
        NonNull::from(self.storage.as_ref()).cast()
    }
}

impl<T: fmt::Debug + Clone + PartialEq + Send + 'static> scheme::IR for AstArena<T> {
    type Ix = NodePath;
    type Value = T;
    type Error = AstArenaError;

    fn query(&self, index: NodePath) -> Result<T, Self::Error> {
        let query_path = index;
        let Some(path) = self.resolve_path(&query_path) else {
            return Err(AstArenaError::MissingPath { path: query_path });
        };

        let Some(node) = self.get_erased(path) else {
            return Err(AstArenaError::MissingPath { path: query_path });
        };

        // Special case: when T == AstMapAny, wrap the stored ErasedAstNode
        // instead of trying to downcast the concrete stored type to AstMapAny.
        if TypeId::of::<T>() == TypeId::of::<AstMapAny>() {
            return AST_CELL_CLONE_ARENA.with(|slot| {
                let prev = slot.replace(Some(self.storage_ptr()));
                let map_any = AstMapAny(node.clone());
                let boxed: Box<dyn Any> = Box::new(map_any);
                let result =
                    boxed
                        .downcast::<T>()
                        .map(|b| *b)
                        .map_err(|_| AstArenaError::TypeMismatch {
                            path: query_path,
                            expected: type_name::<T>(),
                        });
                slot.set(prev);
                result
            });
        }

        AST_CELL_CLONE_ARENA.with(|slot| {
            let prev = slot.replace(Some(self.storage_ptr()));
            let result = node
                .downcast_ref::<T>()
                .cloned()
                .ok_or(AstArenaError::TypeMismatch {
                    path: query_path,
                    expected: type_name::<T>(),
                });
            slot.set(prev);
            result
        })
    }

    fn apply_transaction(&mut self, txn: scheme::Transaction<Self>) -> Result<(), Self::Error> {
        use std::collections::HashMap;

        // Is T == AstMapAny?  If so we must *unwrap* the inner ErasedAstNode
        // rather than double-boxing it (AstMapAny already IS an ErasedAstNode).
        let is_any = TypeId::of::<T>() == TypeId::of::<AstMapAny>();

        // Staging area: usize ID -> ErasedAstNode
        let mut staging: HashMap<usize, ErasedAstNode> = HashMap::new();

        for cmd in txn.iter() {
            match cmd {
                scheme::Command::Create { id, value } => {
                    let erased = if is_any {
                        // SAFETY: is_any guarantees T == AstMapAny at runtime.
                        let boxed: Box<dyn Any + Send> = Box::new(value.clone());
                        boxed.downcast::<AstMapAny>().expect("T==AstMapAny").0
                    } else {
                        ErasedAstNode::new(value.clone())
                    };
                    staging.insert(*id, erased);
                }
                scheme::Command::Insert { index, id } => {
                    if let Some(node) = staging.remove(id) {
                        self.set_erased(index.clone(), node);
                    }
                }
                scheme::Command::Replace { index, id } => {
                    if let Some(node) = staging.remove(id) {
                        self.set_erased(index.clone(), node);
                    }
                }
                scheme::Command::Delete { index } => {
                    self.remove_erased(index.clone());
                }
                scheme::Command::SetRoot { id: _ } => {
                    // SetRoot with staging: reserved for future root tracking
                }
            }
        }
        self.refresh_root();
        Ok(())
    }
}
