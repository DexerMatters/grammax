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
    parsec::tree::{GreenId, Tag},
    runtime::command::{Command, NodePath},
};

// ============================================================================
// Core AST Cell & Arena (stable)
// ============================================================================

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

// ============================================================================
// Mapper & Query Interface
// ============================================================================

pub struct GreenQuery<'a, T> {
    grammar: &'a Grammar,
    greens: &'a FxHashMap<GreenId, GreenNode<T>>,
    source_text: &'a str,
    green: GreenId,
}

struct GreenNode<T> {
    children: Vec<GreenId>,
    offset: usize,
    width: usize,
    token_text: Option<String>,
    tag: Tag,
    binding: Option<ASTCell<T>>,
}

impl<'a, T> GreenQuery<'a, T> {
    pub fn green(&self) -> GreenId {
        self.green
    }

    pub fn tag(&self) -> &Tag {
        &self.greens[&self.green].tag
    }

    pub fn rule_name(&self) -> Option<&'a str> {
        match self.tag() {
            Tag::Rule { rule_ix, .. } => Some(self.grammar.name(*rule_ix)),
            _ => None,
        }
    }

    pub fn span(&self) -> (usize, usize) {
        let node = &self.greens[&self.green];
        let start = node.offset.min(self.source_text.len());
        let end = start.saturating_add(node.width).min(self.source_text.len());
        (start, end)
    }

    pub fn text(&self) -> &'a str {
        let node = &self.greens[&self.green];
        if let Some(text) = &node.token_text {
            return text;
        }
        let (start, end) = self.span();
        self.source_text.get(start..end).unwrap_or("")
    }

    pub fn text_trimmed(&self) -> &'a str {
        self.text().trim()
    }

    pub fn children(&self) -> Vec<GreenQuery<'a, T>> {
        let node = &self.greens[&self.green];
        node.children
            .iter()
            .map(|&child| GreenQuery {
                grammar: self.grammar,
                greens: self.greens,
                source_text: self.source_text,
                green: child,
            })
            .collect()
    }

    pub fn child_asts(&self) -> Vec<Option<ASTCell<T>>> {
        let node = &self.greens[&self.green];
        node.children
            .iter()
            .map(|&child| self.greens[&child].binding)
            .collect()
    }

    /// Get a child at a specific index (0-based)
    pub fn child_at(&self, index: usize) -> Option<GreenQuery<'a, T>> {
        let node = &self.greens[&self.green];
        node.children.get(index).map(|&child| GreenQuery {
            grammar: self.grammar,
            greens: self.greens,
            source_text: self.source_text,
            green: child,
        })
    }

    /// Get a child with a specific field name
    pub fn child_with_field(&self, field_name: &'static str) -> Option<GreenQuery<'a, T>> {
        let node = &self.greens[&self.green];
        for &child_id in &node.children {
            let child_node = &self.greens[&child_id];
            if matches!(
                &child_node.tag,
                Tag::Field { name, .. } if *name == field_name
            ) {
                return Some(GreenQuery {
                    grammar: self.grammar,
                    greens: self.greens,
                    source_text: self.source_text,
                    green: child_id,
                });
            }
        }
        None
    }

    /// Get all children that are rules with a specific name (unwraps through field wrappers)
    pub fn children_with_rule(&self, rule_name: &str) -> Vec<GreenQuery<'a, T>> {
        let node = &self.greens[&self.green];
        let mut result = Vec::new();
        for &child_id in &node.children {
            let mut actual_id = child_id;
            // Unwrap field wrapper if present
            let child_node = &self.greens[&child_id];
            if matches!(&child_node.tag, Tag::Field { .. }) {
                if let Some(&inner_id) = child_node.children.first() {
                    actual_id = inner_id;
                }
            }

            let actual_node = &self.greens[&actual_id];
            if let Tag::Rule { rule_ix, .. } = &actual_node.tag {
                if self.grammar.name(*rule_ix) == rule_name {
                    result.push(GreenQuery {
                        grammar: self.grammar,
                        greens: self.greens,
                        source_text: self.source_text,
                        green: actual_id,
                    });
                }
            }
        }
        result
    }

    /// Get the first child that's a rule with a specific name
    pub fn first_child_with_rule(&self, rule_name: &str) -> Option<GreenQuery<'a, T>> {
        self.children_with_rule(rule_name).into_iter().next()
    }

    /// Get the AST binding for the first child (after unwrapping field wrapper)
    pub fn first_child_ast<U>(&self) -> Option<ASTCell<U>> {
        let node = &self.greens[&self.green];
        if let Some(&child_id) = node.children.first() {
            return self.greens[&child_id].binding.map(|id| id.cast::<U>());
        }
        None
    }

    /// Get all child ASTs that are mapped (filters out None values)
    pub fn mapped_children<U>(&self) -> Vec<ASTCell<U>> {
        self.child_asts()
            .into_iter()
            .filter_map(|binding| binding.map(|id| id.cast::<U>()))
            .collect()
    }
}

pub trait AstMapper<T> {
    fn map(&self, cx: &GreenQuery<'_, T>) -> MapOutput<T>;
}

impl AstMapper<()> for () {
    fn map(&self, _: &GreenQuery<'_, ()>) -> MapOutput<()> {
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

type RuleMapperFn<T> = dyn for<'a> Fn(&GreenQuery<'a, T>) -> MapOutput<T> + Send + Sync + 'static;

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
        F: for<'a> Fn(&GreenQuery<'a, T>) -> MapOutput<T> + Send + Sync + 'static,
    {
        self.rules_name.insert(rule_name.into(), Box::new(mapper));
        self
    }

    pub fn on_rule_ix<F>(mut self, rule_ix: usize, mapper: F) -> Self
    where
        F: for<'a> Fn(&GreenQuery<'a, T>) -> MapOutput<T> + Send + Sync + 'static,
    {
        self.rules_ix.insert(rule_ix, Box::new(mapper));
        self
    }

    pub fn on_token<F>(mut self, rule_ix: usize, mapper: F) -> Self
    where
        F: for<'a> Fn(&GreenQuery<'a, T>) -> MapOutput<T> + Send + Sync + 'static,
    {
        self.tokens.insert(rule_ix, Box::new(mapper));
        self
    }

    pub fn on_field<F>(mut self, rule_ix: usize, mapper: F) -> Self
    where
        F: for<'a> Fn(&GreenQuery<'a, T>) -> MapOutput<T> + Send + Sync + 'static,
    {
        self.fields.insert(rule_ix, Box::new(mapper));
        self
    }

    pub fn on_error<F>(mut self, mapper: F) -> Self
    where
        F: for<'a> Fn(&GreenQuery<'a, T>) -> MapOutput<T> + Send + Sync + 'static,
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
    fn map(&self, cx: &GreenQuery<'_, T>) -> MapOutput<T> {
        match cx.tag() {
            Tag::Rule { rule_ix, .. } => {
                if let Some(mapper) = self.rules_ix.get(rule_ix) {
                    return (mapper)(cx);
                }
                if let Some(rule_name) = cx.rule_name() {
                    if let Some(mapper) = self.rules_name.get(rule_name) {
                        return (mapper)(cx);
                    }
                }
                self.fallback()
            }
            Tag::Token { rule_ix } => self
                .tokens
                .get(rule_ix)
                .map(|f| (f)(cx))
                .unwrap_or_else(|| self.fallback()),
            Tag::Field { rule_ix, .. } => self
                .fields
                .get(rule_ix)
                .map(|f| (f)(cx))
                .unwrap_or_else(|| self.fallback()),
            Tag::Error(_) => self
                .error
                .as_ref()
                .map(|f| (f)(cx))
                .unwrap_or_else(|| self.fallback()),
        }
    }
}

// ============================================================================
// Incremental Lowerer - Clean Command→AST Delta
// ============================================================================

pub struct IncrementalLowerer<T, M> {
    grammar: &'static Grammar,
    mapper: M,
    greens: FxHashMap<GreenId, GreenNode<T>>,
    field_symbols: FxHashMap<String, &'static str>,
    command_nodes: FxHashMap<u64, GreenId>,
    arena: AstArena<T>,
    root_green: Option<GreenId>,
    root_ast: Option<ASTCell<T>>,
    next_green_id: GreenId,
}

impl<T, M> IncrementalLowerer<T, M>
where
    T: Clone + PartialEq + 'static,
    M: AstMapper<T>,
{
    pub fn new(grammar: &'static Grammar, mapper: M) -> Self {
        Self {
            grammar,
            mapper,
            greens: FxHashMap::default(),
            field_symbols: FxHashMap::default(),
            command_nodes: FxHashMap::default(),
            arena: AstArena::new(),
            root_green: None,
            root_ast: None,
            next_green_id: 1,
        }
    }

    pub fn arena(&self) -> &AstArena<T> {
        &self.arena
    }

    pub fn root_ast(&self) -> Option<ASTCell<T>> {
        self.root_ast
    }

    pub fn has_parse_node(&self, green: GreenId) -> bool {
        self.greens.contains_key(&green)
    }

    pub fn apply_parse_delta(&mut self, commands: &[Command]) -> AstDelta<T> {
        self.apply_parse_delta_with_source(commands, "")
    }

    pub fn apply_parse_delta_with_source(
        &mut self,
        commands: &[Command],
        source_text: &str,
    ) -> AstDelta<T> {
        // Phase 1: Update green tree from commands
        self.reset_command_nodes_if_needed(commands);
        let mut dirty = FxHashSet::default();

        for command in commands {
            self.apply_command(command, &mut dirty);
        }

        // Phase 2: Recompute dirty greens & emit AST deltas
        let mut ops = Vec::new();
        for &green in &dirty {
            if self.greens.contains_key(&green) {
                self.recompute_binding(green, source_text, &mut ops);
            }
        }

        // Phase 3: Update root & emit root-change if needed
        let next_root = self.compute_root();
        if next_root != self.root_ast {
            self.root_ast = next_root;
            ops.push(AstDeltaOp::SetRoot { root: next_root });
        }

        AstDelta {
            root: self.root_ast,
            ops,
        }
    }

    fn apply_command(&mut self, command: &Command, dirty: &mut FxHashSet<GreenId>) {
        match command {
            Command::CreateToken {
                node_id,
                rule_ix,
                text,
                field,
            } => {
                let green = self.create_token_leaf(*rule_ix, text);
                let wrapped = self.wrap_in_field(green, *rule_ix, field);
                self.command_nodes.insert(*node_id, wrapped);
                dirty.insert(wrapped);
            }
            Command::CreateError {
                node_id,
                kind,
                text,
                field,
            } => {
                let green = self.create_error_leaf(kind.clone(), text);
                let fallback_rule_ix = self.grammar.table.start_rule;
                let wrapped = self.wrap_in_field(green, fallback_rule_ix, field);
                self.command_nodes.insert(*node_id, wrapped);
                dirty.insert(wrapped);
            }
            Command::CreateNode {
                node_id,
                rule_ix,
                children,
                field,
            } => {
                let child_greens: Vec<GreenId> = children
                    .iter()
                    .filter_map(|id| self.command_nodes.get(id).copied())
                    .collect();
                let parent = self.create_rule_node(*rule_ix, child_greens);
                let wrapped = self.wrap_in_field(parent, *rule_ix, field);
                self.command_nodes.insert(*node_id, wrapped);
                dirty.insert(wrapped);
            }
            Command::DeleteNodeAtPath { path } => {
                if path.0.is_empty() {
                    if let Some(old) = self.root_green.take() {
                        self.prune_green(old);
                    }
                } else if let Some(parent) = self.green_at_path(&path.parent().unwrap()) {
                    if let Some(&idx) = path.0.last() {
                        if let Some(removed) = self.greens.get_mut(&parent).and_then(|p| {
                            if idx < p.children.len() {
                                Some(p.children.remove(idx))
                            } else {
                                None
                            }
                        }) {
                            self.prune_green(removed);
                            dirty.insert(parent);
                        }
                    }
                }
            }
            Command::ReplaceNodeAtPath {
                path,
                node_id,
                target_kind: _,
            } => {
                let new_green = self.command_nodes.get(node_id).copied();
                if let Some(new_green) = new_green {
                    if path.0.is_empty() {
                        if let Some(old) = self.root_green.replace(new_green) {
                            self.prune_green(old);
                        }
                        dirty.insert(new_green);
                    } else if let Some(parent) = self.green_at_path(&path.parent().unwrap()) {
                        if let Some(&idx) = path.0.last() {
                            if let Some(node) = self.greens.get_mut(&parent) {
                                if idx < node.children.len() {
                                    let old = node.children[idx];
                                    node.children[idx] = new_green;
                                    self.prune_green(old);
                                    dirty.insert(parent);
                                }
                            }
                        }
                    }
                }
            }
            Command::InsertNodeAtPath {
                path,
                node_id,
                cascade_to_root: _,
            } => {
                let new_green = self.command_nodes.get(node_id).copied();
                if let Some(new_green) = new_green {
                    if path.0.is_empty() {
                        if let Some(old) = self.root_green.replace(new_green) {
                            self.prune_green(old);
                        }
                        dirty.insert(new_green);
                    } else if let Some(parent) = self.green_at_path(&path.parent().unwrap()) {
                        if let Some(&idx) = path.0.last() {
                            if let Some(node) = self.greens.get_mut(&parent) {
                                node.children.insert(idx, new_green);
                                dirty.insert(parent);
                            }
                        }
                    }
                }
            }
        }
    }

    fn create_token_leaf(&mut self, rule_ix: usize, text: &str) -> GreenId {
        let id = self.next_green_id;
        self.next_green_id = self.next_green_id.saturating_add(1);
        self.greens.insert(
            id,
            GreenNode {
                children: Vec::new(),
                offset: 0,
                width: text.len(),
                token_text: Some(text.to_string()),
                tag: Tag::Token { rule_ix },
                binding: None,
            },
        );
        id
    }

    fn create_error_leaf(&mut self, kind: crate::parsec::tree::ParsecError, text: &str) -> GreenId {
        let id = self.next_green_id;
        self.next_green_id = self.next_green_id.saturating_add(1);
        self.greens.insert(
            id,
            GreenNode {
                children: Vec::new(),
                offset: 0,
                width: text.len(),
                token_text: Some(text.to_string()),
                tag: Tag::new_error(kind),
                binding: None,
            },
        );
        id
    }

    fn create_rule_node(&mut self, rule_ix: usize, children: Vec<GreenId>) -> GreenId {
        let width = children.iter().map(|&c| self.greens[&c].width).sum();
        let id = self.next_green_id;
        self.next_green_id = self.next_green_id.saturating_add(1);
        self.greens.insert(
            id,
            GreenNode {
                children,
                offset: 0,
                width,
                token_text: None,
                tag: Tag::Rule {
                    rule_ix,
                    reparse_rule_ix: rule_ix,
                },
                binding: None,
            },
        );
        id
    }

    fn wrap_in_field(&mut self, child: GreenId, rule_ix: usize, field_name: &str) -> GreenId {
        if field_name.is_empty() {
            return child;
        }
        let width = self.greens[&child].width;
        let name = self.intern_field_name(field_name);
        let id = self.next_green_id;
        self.next_green_id = self.next_green_id.saturating_add(1);
        self.greens.insert(
            id,
            GreenNode {
                children: vec![child],
                offset: 0,
                width,
                token_text: None,
                tag: Tag::Field { rule_ix, name },
                binding: None,
            },
        );
        id
    }

    fn intern_field_name(&mut self, name: &str) -> &'static str {
        if let Some(&interned) = self.field_symbols.get(name) {
            return interned;
        }
        let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
        self.field_symbols.insert(name.to_string(), leaked);
        leaked
    }

    fn green_at_path(&self, path: &NodePath) -> Option<GreenId> {
        let mut current = self.root_green?;
        for &idx in &path.0 {
            let children = &self.greens.get(&current)?.children;
            current = *children.get(idx)?;
        }
        Some(current)
    }

    fn recompute_binding(
        &mut self,
        green: GreenId,
        source_text: &str,
        ops: &mut Vec<AstDeltaOp<T>>,
    ) {
        // Ensure all children are recomputed first
        let children = self.greens[&green].children.clone();
        for child in children {
            if self
                .greens
                .get(&child)
                .map_or(false, |n| n.binding.is_none())
            {
                self.recompute_binding(child, source_text, ops);
            }
        }

        // Get child AST IDs before invoking mapper (to avoid borrow conflicts)
        let child_asts: Vec<_> = self.greens[&green]
            .children
            .iter()
            .map(|&child| self.greens[&child].binding)
            .collect();

        // Build query view and invoke mapper
        let query = GreenQuery {
            grammar: self.grammar,
            greens: &self.greens,
            source_text,
            green,
        };
        let mapped = self.mapper.map(&query);

        // Process mapped output to get new binding (before mutating)
        let old_binding = self.greens[&green].binding;
        let forward_child_ast = match &mapped.kind {
            MapOutputKind::ForwardChild(idx) => child_asts.get(*idx).copied(),
            _ => None,
        };

        // Drop query to release borrow on self.greens
        drop(query);

        // Now update binding
        let new_binding = match mapped.kind {
            MapOutputKind::Node(erased) => {
                let typed = erased.downcast_ref::<T>().cloned();
                let id = self.arena.insert_erased(erased).cast();
                if let Some(node) = typed {
                    ops.push(AstDeltaOp::Create { id, node });
                }
                Some(id)
            }
            MapOutputKind::Alias(id) => Some(id.cast()),
            MapOutputKind::ForwardChild(_) => forward_child_ast.flatten(),
            MapOutputKind::Skip => None,
        };

        // Clean up old binding if replaced
        if let Some(old_id) = old_binding {
            if new_binding != Some(old_id) {
                if let Some(_) = self.arena.remove_erased(old_id.cast()) {
                    ops.push(AstDeltaOp::Delete { id: old_id });
                }
            }
        }

        self.greens.get_mut(&green).unwrap().binding = new_binding;
    }

    fn compute_root(&self) -> Option<ASTCell<T>> {
        self.root_green.and_then(|g| {
            self.greens
                .get(&g)
                .and_then(|n| n.binding)
                .filter(|id| id.arena_ty == Some(type_name::<T>()))
        })
    }

    fn prune_green(&mut self, green: GreenId) {
        if let Some(node) = self.greens.remove(&green) {
            if let Some(binding_id) = node.binding {
                self.arena.remove_erased(binding_id.cast());
            }
            for child in node.children {
                self.prune_green(child);
            }
        }
    }

    fn reset_command_nodes_if_needed(&mut self, commands: &[Command]) {
        if commands.iter().any(|cmd| {
            matches!(
                cmd,
                Command::CreateNode { .. }
                    | Command::CreateToken { .. }
                    | Command::CreateError { .. }
            )
        }) {
            self.command_nodes.clear();
        }
    }
}
