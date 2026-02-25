use rustc_hash::{FxHashMap, FxHashSet};
use std::{
    any::{Any, TypeId, type_name},
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    ptr::NonNull,
};

use crate::{
    grammar::Grammar,
    parsec::tree::{GreenId, Tag, TreeAllocRef, TreeAllocRefExt},
    semantic::Command,
};

pub struct ASTCell<T> {
    raw: usize,
    arena: Option<NonNull<()>>,
    arena_ty: Option<&'static str>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Copy for ASTCell<T> {}

impl<T> Clone for ASTCell<T> {
    fn clone(&self) -> Self {
        *self
    }
}

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

    const fn cast<U>(self) -> ASTCell<U> {
        ASTCell {
            raw: self.raw,
            arena: self.arena,
            arena_ty: self.arena_ty,
            _marker: PhantomData,
        }
    }
}

/// Incremental operations produced for the user-defined IR arena.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstDeltaOp<T> {
    Create { id: ASTCell<T>, node: T },
    Update { id: ASTCell<T>, node: T },
    Delete { id: ASTCell<T> },
    SetRoot { root: Option<ASTCell<T>> },
}

/// Declarative IR delta for one parse update.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AstDelta<T> {
    pub ops: Vec<AstDeltaOp<T>>,
    pub root: Option<ASTCell<T>>,
}

/// Stable-id arena for user-defined IR nodes.
#[derive(Debug)]
pub struct AstArena<T> {
    storage: Box<AstArenaStorage>,
    _marker: PhantomData<fn() -> T>,
}

#[derive(Debug)]
struct AstArenaStorage {
    nodes: Vec<Option<ErasedAstNode>>,
    free: Vec<usize>,
}

struct ErasedAstNode {
    type_id: TypeId,
    type_name: &'static str,
    value: Box<dyn Any>,
    clone_fn: fn(&Box<dyn Any>) -> Box<dyn Any>,
    eq_fn: fn(&Box<dyn Any>, &Box<dyn Any>) -> bool,
    debug_fn: fn(&Box<dyn Any>, &mut fmt::Formatter<'_>) -> fmt::Result,
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
    fn new<U>(value: U) -> Self
    where
        U: fmt::Debug + Clone + PartialEq + 'static,
    {
        fn clone_impl<U: Clone + 'static>(value: &Box<dyn Any>) -> Box<dyn Any> {
            Box::new(
                value
                    .downcast_ref::<U>()
                    .expect("stored erased node must match clone type")
                    .clone(),
            )
        }
        fn eq_impl<U: PartialEq + 'static>(lhs: &Box<dyn Any>, rhs: &Box<dyn Any>) -> bool {
            lhs.downcast_ref::<U>() == rhs.downcast_ref::<U>()
        }
        fn debug_impl<U: fmt::Debug + 'static>(
            value: &Box<dyn Any>,
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

    fn downcast_ref<U: 'static>(&self) -> Option<&U> {
        self.value.downcast_ref::<U>()
    }

    fn into_downcast<U: 'static>(self) -> Option<U> {
        self.value.downcast::<U>().ok().map(|value| *value)
    }

    fn same_value(&self, other: &Self) -> bool {
        self.type_id == other.type_id && (self.eq_fn)(&self.value, &other.value)
    }

    fn fmt_value(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        U: fmt::Debug + Clone + PartialEq + 'static,
    {
        self.insert_erased(ErasedAstNode::new(node)).cast()
    }

    pub fn set<U>(&mut self, id: ASTCell<U>, node: U)
    where
        U: fmt::Debug + Clone + PartialEq + 'static,
    {
        self.set_erased(id.cast(), ErasedAstNode::new(node));
    }

    pub fn remove<U>(&mut self, id: ASTCell<U>) -> Option<U>
    where
        U: 'static,
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

    fn insert_erased(&mut self, node: ErasedAstNode) -> ASTCell<()> {
        let node_ty = node.type_name;
        if let Some(id) = self.storage.free.pop() {
            self.storage.nodes[id] = Some(node);
            return self.cell(id, node_ty);
        }
        let id = self.storage.nodes.len();
        self.storage.nodes.push(Some(node));
        self.cell(id, node_ty)
    }

    fn set_erased(&mut self, id: ASTCell<()>, node: ErasedAstNode) {
        let raw = id.into_raw();
        if raw >= self.storage.nodes.len() {
            self.storage.nodes.resize_with(raw + 1, || None);
        }
        self.storage.nodes[raw] = Some(node);
    }

    fn remove_erased(&mut self, id: ASTCell<()>) -> Option<ErasedAstNode> {
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

    fn get_erased(&self, id: ASTCell<()>) -> Option<&ErasedAstNode> {
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
}

/// Mapper output for one parse node.
pub struct MapOutput<T> {
    kind: MapOutputKind,
    _marker: PhantomData<fn() -> T>,
}

enum MapOutputKind {
    Node(ErasedAstNode),
    Alias(ASTCell<()>),
    ForwardChild(usize),
    Skip,
}

impl<T> MapOutput<T> {
    pub fn node<U>(node: U) -> Self
    where
        U: fmt::Debug + Clone + PartialEq + 'static,
    {
        Self {
            kind: MapOutputKind::Node(ErasedAstNode::new(node)),
            _marker: PhantomData,
        }
    }

    pub fn forward_child(index: usize) -> Self {
        Self {
            kind: MapOutputKind::ForwardChild(index),
            _marker: PhantomData,
        }
    }

    pub fn alias<U>(id: ASTCell<U>) -> Self {
        Self {
            kind: MapOutputKind::Alias(id.cast()),
            _marker: PhantomData,
        }
    }

    pub fn skip() -> Self {
        Self {
            kind: MapOutputKind::Skip,
            _marker: PhantomData,
        }
    }
}

/// Read-only view over one parse node during lowering.
pub struct NodeView<'a, T> {
    grammar: &'a Grammar,
    alloc: &'a TreeAllocRef,
    parse_nodes: &'a FxHashMap<GreenId, ParseMemo<T>>,
    source_text: &'a str,
    green: GreenId,
    trail: Vec<CursorFrame>,
}

impl<'a, T> Clone for NodeView<'a, T> {
    fn clone(&self) -> Self {
        Self {
            grammar: self.grammar,
            alloc: self.alloc,
            parse_nodes: self.parse_nodes,
            source_text: self.source_text,
            green: self.green,
            trail: self.trail.clone(),
        }
    }
}

#[derive(Clone, Copy)]
struct CursorFrame {
    parent: GreenId,
    child_index: usize,
}

pub struct NextEachRule<'a, T> {
    cursor: NodeView<'a, T>,
    rule_name: String,
    active: bool,
}

impl<'a, T> Iterator for NextEachRule<'a, T> {
    type Item = NodeView<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.active {
            return None;
        }
        if !self.cursor.next_rule(&self.rule_name) {
            self.active = false;
            return None;
        }

        let out = self.cursor.clone();
        if !self.cursor.next() {
            self.active = false;
        }
        Some(out)
    }
}

impl<'a, T> NodeView<'a, T> {
    fn child_at(&self, parent: GreenId, child_index: usize) -> Option<GreenId> {
        self.parse_nodes
            .get(&parent)
            .and_then(|memo| memo.children.get(child_index).copied())
    }

    fn first_child_of(&self, parent: GreenId) -> Option<GreenId> {
        self.child_at(parent, 0)
    }

    pub fn green(&self) -> GreenId {
        self.green
    }

    pub fn step_in(&mut self) -> bool {
        let Some(child) = self.first_child_of(self.green) else {
            return false;
        };
        self.trail.push(CursorFrame {
            parent: self.green,
            child_index: 0,
        });
        self.green = child;
        true
    }

    pub fn step_out(&mut self) -> bool {
        let Some(frame) = self.trail.pop() else {
            return false;
        };
        self.green = frame.parent;
        true
    }

    pub fn next(&mut self) -> bool {
        let Some(frame) = self.trail.last().copied() else {
            return false;
        };
        let next_index = frame.child_index + 1;
        let Some(next_green) = self.child_at(frame.parent, next_index) else {
            return false;
        };
        if let Some(frame) = self.trail.last_mut() {
            frame.child_index = next_index;
        }
        self.green = next_green;
        true
    }

    pub fn next_into(&mut self, mut scope: impl FnMut(&mut Self) -> bool) -> bool {
        if !self.step_in() {
            return false;
        }
        let result = scope(self);
        self.step_out();
        self.next();
        result
    }

    pub fn next_field(&mut self, field_name: &str) -> bool {
        if self.field_name_is(field_name) {
            return true;
        }
        while self.next() {
            if self.field_name_is(field_name) {
                return true;
            }
        }
        false
    }

    pub fn is_error(&self) -> bool {
        matches!(&self.alloc.get_node(self.green).tag, Tag::Error(_))
    }

    pub fn next_rule(&mut self, rule_name: &str) -> bool {
        if self.rule_name_is(rule_name) {
            return true;
        }
        while self.next() {
            if self.rule_name_is(rule_name) {
                return true;
            }
        }
        false
    }

    pub fn next_each_rule(&self, rule_name: &str) -> NextEachRule<'a, T> {
        let mut cursor = self.clone();
        let active = cursor.step_in();
        NextEachRule {
            cursor,
            rule_name: rule_name.to_string(),
            active,
        }
    }

    pub fn rule_name(&self) -> Option<&'a str> {
        let rule_ix = match &self.alloc.get_node(self.green).tag {
            Tag::Rule { rule_ix } => Some(*rule_ix),
            _ => None,
        }?;
        Some(self.grammar.name(rule_ix))
    }

    pub fn rule_name_is(&self, expected: &str) -> bool {
        self.rule_name().map_or(false, |name| name == expected)
    }

    pub fn field_name_is(&self, expected: &str) -> bool {
        self.field_name().map_or(false, |name| name == expected)
    }

    pub fn field_name(&self) -> Option<&'static str> {
        match &self.alloc.get_node(self.green).tag {
            Tag::Field { name, .. } => Some(*name),
            _ => None,
        }
    }

    pub fn mapped_opt<U>(&self) -> Option<ASTCell<U>> {
        self.parse_nodes
            .get(&self.green)
            .and_then(|memo| memo.binding.cell())
            .map(ASTCell::cast)
    }

    pub fn mapped<U>(&self) -> ASTCell<U> {
        self.mapped_opt::<U>().unwrap_or_else(|| {
            panic!(
                "NodeView: no mapped AST cell for node (rule: {:?}, green: {:?})",
                self.rule_name(),
                self.green
            )
        })
    }

    pub fn span(&self) -> (usize, usize) {
        let start = self
            .parse_nodes
            .get(&self.green)
            .map_or(0, |memo| memo.offset)
            .min(self.source_text.len());
        let width = self.alloc.get_node(self.green).width;
        let end = start.saturating_add(width).min(self.source_text.len());
        (start, end)
    }

    pub fn text(&self) -> &'a str {
        let (start, end) = self.span();
        self.source_text.get(start..end).unwrap_or("")
    }

    pub fn text_trimmed(&self) -> &'a str {
        self.text().trim()
    }

    pub fn mapped_children<U>(&self) -> impl Iterator<Item = ASTCell<U>> + 'a {
        self.children().filter_map(|child| child.mapped_opt::<U>())
    }

    pub fn children(&self) -> impl Iterator<Item = NodeView<'a, T>> + 'a {
        let mut out = Vec::new();
        let mut cursor = self.clone();
        if !cursor.step_in() {
            return out.into_iter();
        }
        loop {
            out.push(cursor.clone());
            if !cursor.next() {
                break;
            }
        }
        out.into_iter()
    }

    pub fn for_nodes_by_name(
        &self,
        rule_name: &'static str,
    ) -> impl Iterator<Item = NodeView<'a, T>> + 'a {
        self.next_each_rule(rule_name)
    }

    pub fn for_node_by_field_name(&self, field_name: &'static str) -> NodeView<'a, T> {
        let mut cursor = self.clone();
        if !cursor.step_in() || !cursor.next_field(field_name) {
            panic!(
                "NodeView: no child with field name '{}' (node rule: {:?}, green: {:?})",
                field_name,
                self.rule_name(),
                self.green
            );
        }
        cursor
    }
}

/// Context passed to user mapping closures.
pub struct LowerCtx<'a, T> {
    pub green: GreenId,
    pub tag: &'a Tag,
    pub rule_name: Option<&'a str>,
    grammar: &'a Grammar,
    alloc: &'a TreeAllocRef,
    parse_nodes: &'a FxHashMap<GreenId, ParseMemo<T>>,
    source_text: &'a str,
    offset: usize,
    width: usize,
    child_asts: &'a [Option<ASTCell<()>>],
    child_greens: &'a [GreenId],
}

impl<'a, T> LowerCtx<'a, T> {
    pub fn node(&self) -> NodeView<'a, T> {
        NodeView {
            grammar: self.grammar,
            alloc: self.alloc,
            parse_nodes: self.parse_nodes,
            source_text: self.source_text,
            green: self.green,
            trail: Vec::new(),
        }
    }

    pub fn child<U>(&self, index: usize) -> Option<ASTCell<U>> {
        self.child_asts
            .get(index)
            .copied()
            .flatten()
            .map(ASTCell::cast)
    }

    pub fn child_green(&self, index: usize) -> Option<GreenId> {
        self.child_greens.get(index).copied()
    }

    pub fn child_field_name(&self, index: usize) -> Option<&'static str> {
        let child = *self.child_greens.get(index)?;
        match &self.alloc.get_node(child).tag {
            Tag::Field { name, .. } => Some(*name),
            _ => None,
        }
    }

    pub fn children<'b, U: 'b>(&'b self) -> impl Iterator<Item = ASTCell<U>> + 'b {
        self.child_asts
            .iter()
            .filter_map(|id| *id)
            .map(|id| id.cast::<U>())
    }

    pub fn fields<'b, U: 'b>(
        &'b self,
        field_name: &'b str,
    ) -> impl Iterator<Item = ASTCell<U>> + 'b {
        self.child_asts
            .iter()
            .enumerate()
            .filter_map(
                move |(index, id)| match (id, self.child_field_name(index)) {
                    (Some(id), Some(name)) if name == field_name => Some(id.cast::<U>()),
                    _ => None,
                },
            )
    }

    pub fn next_field<U>(&self, field_name: &str) -> Option<ASTCell<U>> {
        self.child_asts.iter().enumerate().find_map(|(index, id)| {
            match (id, self.child_field_name(index)) {
                (Some(id), Some(name)) if name == field_name => Some(id.cast::<U>()),
                _ => None,
            }
        })
    }

    pub fn span(&self) -> (usize, usize) {
        let start = self.offset.min(self.source_text.len());
        let end = start.saturating_add(self.width).min(self.source_text.len());
        (start, end)
    }

    pub fn text(&self) -> &'a str {
        let (start, end) = self.span();
        self.source_text.get(start..end).unwrap_or("")
    }

    pub fn text_trimmed(&self) -> &'a str {
        self.text().trim()
    }
}

pub trait AstMapper<T> {
    fn map(&self, cx: &LowerCtx<'_, T>) -> MapOutput<T>;
}

impl AstMapper<()> for () {
    fn map(&self, _cx: &LowerCtx<'_, ()>) -> MapOutput<()> {
        MapOutput::skip()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackMode {
    Skip,
    ForwardFirstChild,
}

impl Default for FallbackMode {
    fn default() -> Self {
        Self::ForwardFirstChild
    }
}

type RuleMapperFn<T> = dyn for<'a> Fn(NodeView<'a, T>) -> MapOutput<T> + Send + Sync + 'static;

/// Builder-style mapper so users only register the rules they care about.
pub struct RuleMap<T> {
    rules_ix: FxHashMap<usize, Box<RuleMapperFn<T>>>,
    rules_name: FxHashMap<String, Box<RuleMapperFn<T>>>,
    tokens: FxHashMap<usize, Box<RuleMapperFn<T>>>,
    fields: FxHashMap<usize, Box<RuleMapperFn<T>>>,
    error: Option<Box<RuleMapperFn<T>>>,
    fallback: FallbackMode,
}

impl<T> Default for RuleMap<T> {
    fn default() -> Self {
        Self {
            rules_ix: FxHashMap::default(),
            rules_name: FxHashMap::default(),
            tokens: FxHashMap::default(),
            fields: FxHashMap::default(),
            error: None,
            fallback: FallbackMode::default(),
        }
    }
}

impl<T> RuleMap<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fallback(mut self, fallback: FallbackMode) -> Self {
        self.fallback = fallback;
        self
    }

    pub fn on_rule<F>(mut self, rule_name: impl Into<String>, mapper: F) -> Self
    where
        F: for<'a> Fn(NodeView<'a, T>) -> MapOutput<T> + Send + Sync + 'static,
    {
        self.rules_name.insert(rule_name.into(), Box::new(mapper));
        self
    }

    pub fn on_rule_ix<F>(mut self, rule_ix: usize, mapper: F) -> Self
    where
        F: for<'a> Fn(NodeView<'a, T>) -> MapOutput<T> + Send + Sync + 'static,
    {
        self.rules_ix.insert(rule_ix, Box::new(mapper));
        self
    }

    pub fn on_token<F>(mut self, rule_ix: usize, mapper: F) -> Self
    where
        F: for<'a> Fn(NodeView<'a, T>) -> MapOutput<T> + Send + Sync + 'static,
    {
        self.tokens.insert(rule_ix, Box::new(mapper));
        self
    }

    pub fn on_field<F>(mut self, rule_ix: usize, mapper: F) -> Self
    where
        F: for<'a> Fn(NodeView<'a, T>) -> MapOutput<T> + Send + Sync + 'static,
    {
        self.fields.insert(rule_ix, Box::new(mapper));
        self
    }

    pub fn on_error<F>(mut self, mapper: F) -> Self
    where
        F: for<'a> Fn(NodeView<'a, T>) -> MapOutput<T> + Send + Sync + 'static,
    {
        self.error = Some(Box::new(mapper));
        self
    }

    fn fallback(&self) -> MapOutput<T> {
        match self.fallback {
            FallbackMode::Skip => MapOutput::skip(),
            FallbackMode::ForwardFirstChild => MapOutput::forward_child(0),
        }
    }
}

impl<T> AstMapper<T> for RuleMap<T> {
    fn map(&self, cx: &LowerCtx<'_, T>) -> MapOutput<T> {
        let node = cx.node();
        match cx.tag {
            Tag::Rule { rule_ix } => {
                if let Some(mapper) = self.rules_ix.get(rule_ix) {
                    return (mapper)(node);
                }
                if let Some(rule_name) = cx.rule_name {
                    if let Some(mapper) = self.rules_name.get(rule_name) {
                        return (mapper)(node);
                    }
                }
                self.fallback()
            }
            Tag::Token { rule_ix } => self
                .tokens
                .get(rule_ix)
                .map(|f| (f)(node))
                .unwrap_or_else(|| self.fallback()),
            Tag::Field { rule_ix, .. } => self
                .fields
                .get(rule_ix)
                .map(|f| (f)(node))
                .unwrap_or_else(|| self.fallback()),
            Tag::Error(_) => self
                .error
                .as_ref()
                .map(|f| (f)(node))
                .unwrap_or_else(|| self.fallback()),
            Tag::Root => self.fallback(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AstBinding<T> {
    None,
    Alias(ASTCell<T>),
    Owned(ASTCell<T>),
}

impl<T> AstBinding<T> {
    fn cell(self) -> Option<ASTCell<T>> {
        match self {
            AstBinding::None => None,
            AstBinding::Alias(id) | AstBinding::Owned(id) => Some(id),
        }
    }
}

impl<T> Copy for AstBinding<T> {}

impl<T> Clone for AstBinding<T> {
    fn clone(&self) -> Self {
        *self
    }
}

#[derive(Debug, Clone)]
struct ParseMemo<T> {
    children: Vec<GreenId>,
    offset: usize,
    binding: AstBinding<T>,
}

/// Incremental red-green -> user IR lowering state.
///
/// Usage:
/// 1. Build a `RuleMap` (or implement `AstMapper`).
/// 2. Call `initialize_root` once with the parser root green.
/// 3. Feed parser/reparser semantic commands into `apply_parse_delta`.
pub struct IncrementalLowerer<T, M> {
    alloc: TreeAllocRef,
    grammar: Grammar,
    mapper: M,
    parse_nodes: FxHashMap<GreenId, ParseMemo<T>>,
    parents: FxHashMap<GreenId, FxHashSet<GreenId>>,
    arena: AstArena<T>,
    root_green: Option<GreenId>,
    root_ast: Option<ASTCell<T>>,
}

impl<T, M> IncrementalLowerer<T, M>
where
    T: Clone + PartialEq + 'static,
    M: AstMapper<T>,
{
    pub fn new(alloc: TreeAllocRef, grammar: Grammar, mapper: M) -> Self {
        Self {
            alloc,
            grammar,
            mapper,
            parse_nodes: FxHashMap::default(),
            parents: FxHashMap::default(),
            arena: AstArena::new(),
            root_green: None,
            root_ast: None,
        }
    }

    pub fn arena(&self) -> &AstArena<T> {
        &self.arena
    }

    pub fn root_ast(&self) -> Option<ASTCell<T>> {
        self.root_ast
    }

    pub fn has_parse_node(&self, green: GreenId) -> bool {
        self.parse_nodes.contains_key(&green)
    }

    pub fn initialize_root(&mut self, root_green: GreenId) -> AstDelta<T> {
        self.initialize_root_with_source(root_green, "")
    }

    pub fn initialize_root_with_source(
        &mut self,
        root_green: GreenId,
        source_text: &str,
    ) -> AstDelta<T> {
        let bootstrap = [Command::ReplaceGreen {
            parent_green: None,
            child_index: 0,
            new_green: root_green,
        }];
        self.apply_parse_delta_with_source(&bootstrap, source_text)
    }

    pub fn apply_parse_delta(&mut self, commands: &[Command]) -> AstDelta<T> {
        self.apply_parse_delta_with_source(commands, "")
    }

    pub fn apply_parse_delta_with_source(
        &mut self,
        commands: &[Command],
        source_text: &str,
    ) -> AstDelta<T> {
        if let [
            Command::TreeChanged {
                changed_green,
                changed_offset,
                lineage,
                new_root,
            },
        ] = commands
        {
            return self.apply_tree_changed(
                *changed_green,
                *changed_offset,
                lineage,
                *new_root,
                source_text,
            );
        }

        let mut ops = Vec::new();
        let mut recompute_starts = Vec::new();
        let mut seen_recompute = FxHashSet::default();
        for command in commands {
            if let Some(start) = self.apply_command(command, &mut ops) {
                if seen_recompute.insert(start) {
                    recompute_starts.push(start);
                }
            }
        }
        for start in recompute_starts {
            if self.parse_nodes.contains_key(&start) {
                self.recompute_lineage(start, &mut ops, source_text);
            }
        }

        let next_root = self.root_green.and_then(|green| {
            self.parse_nodes
                .get(&green)
                .and_then(|memo| memo.binding.cell())
                .filter(|id| id.arena_ty == Some(type_name::<T>()))
        });

        if next_root != self.root_ast {
            self.root_ast = next_root;
            ops.push(AstDeltaOp::SetRoot { root: next_root });
        }

        AstDelta {
            root: self.root_ast,
            ops,
        }
    }

    fn apply_tree_changed(
        &mut self,
        changed_green: GreenId,
        changed_offset: usize,
        lineage: &[(GreenId, usize)],
        new_root: GreenId,
        source_text: &str,
    ) -> AstDelta<T> {
        let mut ops = Vec::new();
        let old_root = self.root_green;

        self.ensure_parse_subtree(changed_green, changed_offset);

        let mut child = changed_green;
        let mut child_offset = changed_offset;
        for &(parent_green, child_index) in lineage {
            let children = self.alloc.get_node(parent_green).children.clone();
            let prefix_width: usize = children
                .iter()
                .take(child_index)
                .map(|&green| self.alloc.get_node(green).width)
                .sum();
            let parent_offset = child_offset.saturating_sub(prefix_width);

            match self.parse_nodes.get_mut(&parent_green) {
                Some(memo) => {
                    memo.children = children.clone();
                    memo.offset = parent_offset;
                }
                None => {
                    self.parse_nodes.insert(
                        parent_green,
                        ParseMemo {
                            children: children.clone(),
                            offset: parent_offset,
                            binding: AstBinding::None,
                        },
                    );
                }
            }

            self.add_parent(child, parent_green);
            for &sibling in &children {
                if sibling != child {
                    self.add_parent(sibling, parent_green);
                }
            }

            child = parent_green;
            child_offset = parent_offset;
        }

        self.root_green = Some(new_root);
        if self.parse_nodes.contains_key(&new_root) {
            if let Some(root_memo) = self.parse_nodes.get_mut(&new_root) {
                root_memo.offset = 0;
            }
        } else {
            self.ensure_parse_subtree(new_root, 0);
        }

        if let Some(previous_root) = old_root {
            if previous_root != new_root {
                self.prune_unreachable(previous_root, &mut ops);
            }
        }

        self.recompute_one(changed_green, &mut ops, source_text);
        for &(parent_green, _) in lineage {
            self.recompute_one(parent_green, &mut ops, source_text);
        }

        let next_root = self.root_green.and_then(|green| {
            self.parse_nodes
                .get(&green)
                .and_then(|memo| memo.binding.cell())
                .filter(|id| id.arena_ty == Some(type_name::<T>()))
        });

        if next_root != self.root_ast {
            self.root_ast = next_root;
            ops.push(AstDeltaOp::SetRoot { root: next_root });
        }

        AstDelta {
            root: self.root_ast,
            ops,
        }
    }

    fn apply_command(
        &mut self,
        command: &Command,
        ops: &mut Vec<AstDeltaOp<T>>,
    ) -> Option<GreenId> {
        match command {
            Command::CreateGreen { green } => {
                self.ensure_parse_subtree(*green, 0);
                None
            }
            Command::ReplaceGreen {
                parent_green: None,
                child_index: _,
                new_green,
            } => {
                self.ensure_parse_subtree(*new_green, 0);
                if let Some(old_root) = self.root_green.replace(*new_green) {
                    if old_root == *new_green {
                        return Some(*new_green);
                    }
                    self.prune_unreachable(old_root, ops);
                }
                Some(*new_green)
            }
            Command::ReplaceGreen {
                parent_green: Some(parent),
                child_index,
                new_green,
            } => {
                let child_offset = self.child_offset(*parent, *child_index);
                self.ensure_parse_subtree(*new_green, child_offset);
                let old_child = std::mem::replace(
                    &mut self.memo_mut(*parent).children[*child_index],
                    *new_green,
                );
                self.remove_parent(old_child, *parent);
                self.add_parent(*new_green, *parent);
                if self.parent_count(old_child) == 0 && self.root_green != Some(old_child) {
                    self.prune_unreachable(old_child, ops);
                }
                self.refresh_offsets(*parent);
                Some(*parent)
            }
            Command::InsertGreen {
                parent_green,
                child_index,
                green,
            } => {
                let child_offset = self.child_offset(*parent_green, *child_index);
                self.ensure_parse_subtree(*green, child_offset);
                self.memo_mut(*parent_green)
                    .children
                    .insert(*child_index, *green);
                self.add_parent(*green, *parent_green);
                self.refresh_offsets(*parent_green);
                Some(*parent_green)
            }
            Command::DeleteGreen {
                parent_green,
                child_index,
                green,
            } => {
                let removed = self.memo_mut(*parent_green).children.remove(*child_index);
                debug_assert_eq!(removed, *green);

                self.remove_parent(removed, *parent_green);
                if self.parent_count(removed) == 0 && self.root_green != Some(removed) {
                    self.prune_unreachable(removed, ops);
                }
                self.refresh_offsets(*parent_green);
                Some(*parent_green)
            }
            Command::PathUpdate { path } => {
                // Path contains (parent_green: Option, child_idx, new_green)
                // Apply updates sequentially, starting from root and working down
                let mut last_updated = None;
                for (parent_opt, child_idx, new_green) in path.iter() {
                    match parent_opt {
                        Some(parent) => {
                            // Ensure the new green node's subtree exists in parse_nodes
                            let child_offset = self.child_offset(*parent, *child_idx);
                            self.ensure_parse_subtree(*new_green, child_offset);
                            let old_child = std::mem::replace(
                                &mut self.memo_mut(*parent).children[*child_idx],
                                *new_green,
                            );
                            self.remove_parent(old_child, *parent);
                            self.add_parent(*new_green, *parent);
                            if self.parent_count(old_child) == 0
                                && self.root_green != Some(old_child)
                            {
                                self.prune_unreachable(old_child, ops);
                            }
                            self.refresh_offsets(*parent);
                            last_updated = Some(*parent);
                        }
                        None => {
                            // Root update
                            self.ensure_parse_subtree(*new_green, 0);
                            if let Some(old_root) = self.root_green.replace(*new_green) {
                                if old_root != *new_green {
                                    self.prune_unreachable(old_root, ops);
                                }
                            }
                            last_updated = Some(*new_green);
                        }
                    }
                }
                last_updated
            }
            Command::TreeChanged {
                changed_green: _,
                changed_offset: _,
                lineage: _,
                new_root: _,
            } => None,
        }
    }

    fn recompute_lineage(
        &mut self,
        mut green: GreenId,
        ops: &mut Vec<AstDeltaOp<T>>,
        source_text: &str,
    ) {
        loop {
            self.recompute_one(green, ops, source_text);
            let Some(parent) = self.primary_parent(green) else {
                break;
            };
            green = parent;
        }
    }

    fn ensure_binding(
        &mut self,
        green: GreenId,
        ops: &mut Vec<AstDeltaOp<T>>,
        source_text: &str,
    ) -> Option<ASTCell<T>> {
        let cached = self.memo(green).binding.cell();
        if cached.is_some() {
            return cached;
        }
        self.recompute_one(green, ops, source_text)
    }

    fn recompute_one(
        &mut self,
        green: GreenId,
        ops: &mut Vec<AstDeltaOp<T>>,
        source_text: &str,
    ) -> Option<ASTCell<T>> {
        let memo_snapshot = self.memo(green).clone();
        let mut child_asts = Vec::with_capacity(memo_snapshot.children.len());
        for &child in &memo_snapshot.children {
            child_asts.push(self.ensure_binding(child, ops, source_text));
        }
        let child_cells: Vec<Option<ASTCell<()>>> = child_asts
            .iter()
            .copied()
            .map(|cell| cell.map(|id| id.cast::<()>()))
            .collect();
        let (tag, width) = {
            let node = self.alloc.get_node(green);
            (node.tag.clone(), node.width)
        };
        let rule_name = match &tag {
            Tag::Rule { rule_ix } => Some(self.grammar.name(*rule_ix)),
            _ => None,
        };
        let mapped = {
            let cx = LowerCtx {
                green,
                tag: &tag,
                rule_name,
                grammar: &self.grammar,
                alloc: &self.alloc,
                parse_nodes: &self.parse_nodes,
                source_text,
                offset: memo_snapshot.offset,
                width,
                child_asts: &child_cells,
                child_greens: &memo_snapshot.children,
            };
            self.mapper.map(&cx)
        };
        let new_binding = self.apply_map_output(memo_snapshot.binding, mapped, &child_asts, ops);
        self.memo_mut(green).binding = new_binding;
        new_binding.cell()
    }

    fn apply_map_output(
        &mut self,
        old_binding: AstBinding<T>,
        mapped: MapOutput<T>,
        child_asts: &[Option<ASTCell<T>>],
        ops: &mut Vec<AstDeltaOp<T>>,
    ) -> AstBinding<T> {
        let new_binding = match mapped.kind {
            MapOutputKind::Node(node) => match old_binding {
                AstBinding::Owned(id) => {
                    if self
                        .arena
                        .get_erased(id.cast())
                        .is_some_and(|current| current.same_value(&node))
                    {
                        AstBinding::Owned(id)
                    } else {
                        let typed_node = node.downcast_ref::<T>().cloned();
                        self.arena.set_erased(id.cast(), node);
                        if let Some(node) = typed_node {
                            ops.push(AstDeltaOp::Update { id, node });
                        }
                        AstBinding::Owned(id)
                    }
                }
                AstBinding::Alias(_) | AstBinding::None => {
                    let typed_node = node.downcast_ref::<T>().cloned();
                    let id = self.arena.insert_erased(node).cast::<T>();
                    if let Some(node) = typed_node {
                        ops.push(AstDeltaOp::Create { id, node });
                    }
                    AstBinding::Owned(id)
                }
            },
            MapOutputKind::ForwardChild(index) => child_asts
                .get(index)
                .copied()
                .flatten()
                .map(AstBinding::Alias)
                .unwrap_or(AstBinding::None),
            MapOutputKind::Alias(id) => AstBinding::Alias(id.cast()),
            MapOutputKind::Skip => AstBinding::None,
        };

        if let AstBinding::Owned(old_id) = old_binding {
            if !matches!(new_binding, AstBinding::Owned(id) if id == old_id) {
                self.arena.remove_erased(old_id.cast());
                if old_id.arena_ty == Some(type_name::<T>()) {
                    ops.push(AstDeltaOp::Delete { id: old_id });
                }
            }
        }

        new_binding
    }

    fn ensure_parse_subtree(&mut self, green: GreenId, offset: usize) {
        if let Some(memo) = self.parse_nodes.get_mut(&green) {
            memo.offset = offset;
            return;
        }
        let children = self.alloc.get_node(green).children.clone();
        self.parse_nodes.insert(
            green,
            ParseMemo {
                children: children.clone(),
                offset,
                binding: AstBinding::None,
            },
        );
        let mut child_offset = offset;
        for child in children {
            self.ensure_parse_subtree(child, child_offset);
            self.add_parent(child, green);
            child_offset += self.alloc.get_node(child).width;
        }
    }

    fn child_offset(&self, parent: GreenId, child_index: usize) -> usize {
        let memo = self.memo(parent);
        let mut offset = memo.offset;
        for &child in memo.children.iter().take(child_index) {
            offset += self.alloc.get_node(child).width;
        }
        offset
    }

    fn refresh_offsets(&mut self, root: GreenId) {
        let start = self.memo(root).offset;
        self.refresh_offsets_from(root, start);
    }

    fn refresh_offsets_from(&mut self, green: GreenId, offset: usize) {
        let Some(memo) = self.parse_nodes.get_mut(&green) else {
            return;
        };
        memo.offset = offset;
        let children = memo.children.clone();
        let mut child_offset = offset;
        for child in children {
            self.refresh_offsets_from(child, child_offset);
            child_offset += self.alloc.get_node(child).width;
        }
    }

    #[inline]
    fn memo(&self, green: GreenId) -> &ParseMemo<T> {
        self.parse_nodes.get(&green).unwrap_or_else(|| {
            panic!(
                "well-formed command stream should reference existing parse nodes (missing green={green})"
            )
        })
    }

    #[inline]
    fn memo_mut(&mut self, green: GreenId) -> &mut ParseMemo<T> {
        self.parse_nodes.get_mut(&green).unwrap_or_else(|| {
            panic!(
                "well-formed command stream should reference existing parse nodes (missing green={green})"
            )
        })
    }

    fn add_parent(&mut self, child: GreenId, parent: GreenId) {
        self.parents.entry(child).or_default().insert(parent);
    }

    fn remove_parent(&mut self, child: GreenId, parent: GreenId) {
        if let Some(parents) = self.parents.get_mut(&child) {
            parents.remove(&parent);
            if parents.is_empty() {
                self.parents.remove(&child);
            }
        }
    }

    fn parent_count(&self, child: GreenId) -> usize {
        self.parents.get(&child).map_or(0, FxHashSet::len)
    }

    fn primary_parent(&self, child: GreenId) -> Option<GreenId> {
        self.parents
            .get(&child)
            .and_then(|parents| parents.iter().next().copied())
    }

    fn prune_unreachable(&mut self, start: GreenId, ops: &mut Vec<AstDeltaOp<T>>) {
        let mut stack = vec![start];
        while let Some(green) = stack.pop() {
            if self.root_green == Some(green) {
                continue;
            }
            if self.parent_count(green) > 0 {
                continue;
            }

            let Some(removed) = self.parse_nodes.remove(&green) else {
                continue;
            };
            self.parents.remove(&green);

            if let AstBinding::Owned(ast_id) = removed.binding {
                self.arena.remove(ast_id);
                ops.push(AstDeltaOp::Delete { id: ast_id });
            }

            for child in removed.children {
                self.remove_parent(child, green);
                if self.root_green != Some(child) && self.parent_count(child) == 0 {
                    stack.push(child);
                }
            }
        }
    }
}
