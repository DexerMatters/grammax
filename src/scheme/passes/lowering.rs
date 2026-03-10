//! Pass 2→3: the incremental semantic lowerer.
//!
//! [`IncrementalLowerer<T, M>`] implements [`scheme::Pass<RedGreenTreeIR, AstArena<T>>`]:
//! given a [`RedGreenTreeIR`] transaction (the parser commands), it produces
//! an [`AstDelta<T>`] transaction that drives the [`AstArena<T>`] downstream.
//!
//! User code supplies an [`AstMapper<T>`] (typically [`RuleMap<T>`]) that maps
//! each green parse-tree node to an AST value, an alias, or skips it. The
//! lowerer caches the green tree in its own shadow map so it does not need to
//! re-parse from scratch on every edit.

use std::{any::type_name, fmt, marker::PhantomData};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::scheme::layers::ast::ErasedAstNode;
use crate::{
    grammar::Grammar,
    parsec::tree::{GreenId, Tag, TreeAllocRefExt},
    scheme::{
        self,
        layers::{
            ASTCell, AstArena, AstDelta, Command, NodePath, ParseNodeValue, ParseTreeQuery,
            RedGreenTreeIR,
        },
    },
};

// ============================================================================
// MapOutput — what the user's mapper returns for each green node
// ============================================================================

/// The result of mapping a single green parse-tree node to an AST value.
pub struct MapOutput<T> {
    pub(crate) kind: MapOutputKind,
    _marker: PhantomData<fn() -> T>,
}

pub(crate) enum MapOutputKind {
    Node(ErasedAstNode),
    Alias(ASTCell<()>),
    ForwardChild(usize),
    Skip,
}

impl<T> MapOutput<T> {
    /// The node maps to a concrete AST value of some type `U`.
    pub fn node<U>(node: U) -> Self
    where
        U: fmt::Debug + Clone + PartialEq + Send + 'static,
    {
        Self {
            kind: MapOutputKind::Node(ErasedAstNode::new(node)),
            _marker: PhantomData,
        }
    }

    /// The node forwards the AST of child at `index` (transparent wrapper).
    pub fn forward_child(index: usize) -> Self {
        Self {
            kind: MapOutputKind::ForwardChild(index),
            _marker: PhantomData,
        }
    }

    /// The node shares the same AST cell as an existing node.
    pub fn alias<U>(id: ASTCell<U>) -> Self {
        Self {
            kind: MapOutputKind::Alias(id.cast()),
            _marker: PhantomData,
        }
    }

    /// The node produces no AST value.
    pub fn skip() -> Self {
        Self {
            kind: MapOutputKind::Skip,
            _marker: PhantomData,
        }
    }
}

// ============================================================================
// Query interface — passed to AstMapper callbacks
// ============================================================================

/// A read-only view of a green node, passed to [`AstMapper::map`].
pub struct GreenQuery<'a, T> {
    grammar: &'a Grammar,
    greens: &'a FxHashMap<GreenId, GreenNode<T>>,
    source_text: &'a str,
    green: GreenId,
}

#[derive(Clone)]
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

    pub fn child_at(&self, index: usize) -> Option<GreenQuery<'a, T>> {
        let node = &self.greens[&self.green];
        node.children.get(index).map(|&child| GreenQuery {
            grammar: self.grammar,
            greens: self.greens,
            source_text: self.source_text,
            green: child,
        })
    }

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

    pub fn children_with_rule(&self, rule_name: &str) -> Vec<GreenQuery<'a, T>> {
        let node = &self.greens[&self.green];
        let mut result = Vec::new();
        for &child_id in &node.children {
            let mut actual_id = child_id;
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

    pub fn first_child_with_rule(&self, rule_name: &str) -> Option<GreenQuery<'a, T>> {
        self.children_with_rule(rule_name).into_iter().next()
    }

    pub fn first_child_ast<U>(&self) -> Option<ASTCell<U>> {
        let node = &self.greens[&self.green];
        if let Some(&child_id) = node.children.first() {
            return self.greens[&child_id].binding.map(|id| id.cast::<U>());
        }
        None
    }

    pub fn mapped_children<U>(&self) -> Vec<ASTCell<U>> {
        self.child_asts()
            .into_iter()
            .filter_map(|binding| binding.map(|id| id.cast::<U>()))
            .collect()
    }
}

// ============================================================================
// AstMapper trait — user-supplied logic
// ============================================================================

/// Maps a green parse-tree node to an AST value (or skips/aliases it).
pub trait AstMapper<T> {
    fn map(&self, cx: &GreenQuery<'_, T>) -> MapOutput<T>;
}

impl AstMapper<()> for () {
    fn map(&self, _: &GreenQuery<'_, ()>) -> MapOutput<()> {
        MapOutput::skip()
    }
}

/// What to do when no mapper rule matches a node.
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

/// A rule-based [`AstMapper<T>`] that dispatches on rule name / rule index /
/// token kind / field kind.
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
// IncrementalLowerer — the Pass<RedGreenTreeIR, AstArena<T>>
// ============================================================================

/// The semantic loweringPass (Layer 2 → Layer 3).
///
/// Maintains a shadow copy of the green parse tree indexed by stable
/// [`GreenId`]s. On each call to [`apply_parse_delta_with_source`], it:
///
/// 1. Applies the parse-tree delta to its internal shadow tree.
/// 2. Marks affected nodes as dirty.
/// 3. Re-runs the [`AstMapper`] over dirty nodes.
/// 4. Emits an [`AstDelta<T>`] describing what changed in the [`AstArena<T>`].
///
/// [`apply_parse_delta_with_source`]: IncrementalLowerer::apply_parse_delta_with_source
pub struct IncrementalLowerer<T, M> {
    grammar: &'static Grammar,
    mapper: M,
    greens: FxHashMap<GreenId, GreenNode<T>>,
    pub(crate) arena: AstArena<T>,
    root_green: Option<GreenId>,
    root_ast: Option<ASTCell<T>>,
}

impl<T, M> IncrementalLowerer<T, M>
where
    T: fmt::Debug + Clone + PartialEq + Send + 'static,
    M: AstMapper<T>,
{
    pub fn new(grammar: &'static Grammar, mapper: M) -> Self {
        Self {
            grammar,
            mapper,
            greens: FxHashMap::default(),
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
        self.greens.contains_key(&green)
    }

    fn apply_from_ir2(
        &mut self,
        upstream: &RedGreenTreeIR,
        commands: &[Command],
        source_text: &str,
    ) -> Vec<scheme::Command<AstArena<T>>> {
        let mut ops = Vec::new();

        self.sync_from_upstream(upstream, &mut ops);
        let dirty = self.collect_dirty_nodes(upstream, commands);

        // Phase 2: Recompute dirty greens & emit AST deltas
        let mut visited = FxHashSet::default();
        let mut next_create_id = 0usize;
        for &green in &dirty {
            if self.greens.contains_key(&green) {
                self.recompute_binding(
                    green,
                    source_text,
                    &mut ops,
                    &mut visited,
                    &mut next_create_id,
                );
            }
        }

        // Phase 3: Update root & emit root-change if needed
        let next_root = self.compute_root();
        if next_root != self.root_ast {
            self.root_ast = next_root;
            ops.push(scheme::Command::SetRoot {
                id: next_root.map(|c| c.into_raw()),
            });
        }

        ops
    }

    pub fn transform_with_source(
        &mut self,
        upstream: &RedGreenTreeIR,
        txn: scheme::Transaction<RedGreenTreeIR>,
        source_text: &str,
    ) -> scheme::Transaction<AstArena<T>> {
        std::sync::Arc::new(self.apply_from_ir2(upstream, &txn, source_text))
    }

    fn collect_dirty_nodes(
        &self,
        upstream: &RedGreenTreeIR,
        commands: &[Command],
    ) -> FxHashSet<GreenId> {
        let mut dirty = FxHashSet::default();

        for command in commands {
            match command {
                scheme::Command::Insert { index, .. }
                | scheme::Command::Replace { index, .. }
                | scheme::Command::Delete { index } => {
                    let ParseTreeQuery::Path(index) = index else {
                        continue;
                    };

                    if index.0.is_empty() {
                        if let Some(root) = upstream.root {
                            dirty.insert(root);
                        }
                        continue;
                    }

                    for depth in 0..index.0.len() {
                        let path = NodePath(index.0[..depth].to_vec());
                        if let Some(green) = upstream.green_at_path(&path) {
                            dirty.insert(green);
                        }
                    }

                    if !matches!(command, scheme::Command::Delete { .. }) {
                        if let Some(green) = upstream.green_at_path(index) {
                            dirty.insert(green);
                        }
                    }
                }
                scheme::Command::SetRoot { .. } => {
                    if let Some(root) = upstream.root {
                        dirty.insert(root);
                    }
                }
                scheme::Command::Create { .. } => {}
            }
        }

        if dirty.is_empty() {
            if let Some(root) = upstream.root {
                dirty.insert(root);
            }
        }

        dirty
    }

    fn sync_from_upstream(
        &mut self,
        upstream: &RedGreenTreeIR,
        ops: &mut Vec<scheme::Command<AstArena<T>>>,
    ) {
        let old = std::mem::take(&mut self.greens);
        let mut next = FxHashMap::default();

        if let Some(root) = upstream.root {
            self.build_shadow_from_ir2(root, 0, upstream, &old, &mut next);
        }

        for (green, node) in old {
            if !next.contains_key(&green) {
                if let Some(binding) = node.binding {
                    if self.arena.remove_erased(binding.cast()).is_some() {
                        ops.push(scheme::Command::Delete {
                            index: binding.into_raw(),
                        });
                    }
                }
            }
        }

        self.greens = next;
        self.root_green = upstream.root;
    }

    fn build_shadow_from_ir2(
        &self,
        green: GreenId,
        offset: usize,
        upstream: &RedGreenTreeIR,
        old: &FxHashMap<GreenId, GreenNode<T>>,
        out: &mut FxHashMap<GreenId, GreenNode<T>>,
    ) {
        if let Some(existing) = old.get(&green) {
            if existing.offset == offset {
                self.copy_shadow_subtree(green, upstream, old, out);
                return;
            }
        }

        if out.contains_key(&green) {
            return;
        }

        let alloc_node = upstream.alloc.get_node(green);
        let children = alloc_node.children.clone();
        let width = alloc_node.width;
        let tag = alloc_node.tag.clone();
        drop(alloc_node);

        let token_text = match upstream.value_of_green(green) {
            ParseNodeValue::Token { text, .. } | ParseNodeValue::Error { text, .. } => Some(text),
            ParseNodeValue::Node { .. } | ParseNodeValue::Messages { .. } => None,
        };

        let binding = old.get(&green).and_then(|n| n.binding);

        out.insert(
            green,
            GreenNode {
                children: children.clone(),
                offset,
                width,
                token_text,
                tag,
                binding,
            },
        );

        let mut child_offset = offset;
        for child in children {
            let child_width = upstream.alloc.get_node(child).width;
            self.build_shadow_from_ir2(child, child_offset, upstream, old, out);
            child_offset = child_offset.saturating_add(child_width);
        }
    }

    fn copy_shadow_subtree(
        &self,
        green: GreenId,
        upstream: &RedGreenTreeIR,
        old: &FxHashMap<GreenId, GreenNode<T>>,
        out: &mut FxHashMap<GreenId, GreenNode<T>>,
    ) {
        let mut stack = vec![green];
        while let Some(green) = stack.pop() {
            if out.contains_key(&green) {
                continue;
            }
            let Some(existing) = old.get(&green).cloned() else {
                let offset = upstream
                    .green_at_path(&NodePath(vec![]))
                    .and_then(|root| (root == green).then_some(0))
                    .unwrap_or(0);
                self.build_shadow_from_ir2(green, offset, upstream, old, out);
                continue;
            };
            let children = existing.children.clone();
            out.insert(green, existing);
            for child in children.into_iter().rev() {
                if old.contains_key(&child) {
                    stack.push(child);
                }
            }
        }
    }

    fn recompute_binding(
        &mut self,
        green: GreenId,
        source_text: &str,
        ops: &mut AstDelta<T>,
        visited: &mut FxHashSet<GreenId>,
        next_create_id: &mut usize,
    ) {
        let mut stack = vec![(green, false)];

        while let Some((green, expanded)) = stack.pop() {
            if expanded {
                let child_asts: Vec<_> = self.greens[&green]
                    .children
                    .iter()
                    .map(|&child| self.greens[&child].binding)
                    .collect();

                let query = GreenQuery {
                    grammar: self.grammar,
                    greens: &self.greens,
                    source_text,
                    green,
                };
                let mapped = self.mapper.map(&query);

                let old_binding = self.greens[&green].binding;
                let forward_child_ast = match &mapped.kind {
                    MapOutputKind::ForwardChild(idx) => child_asts.get(*idx).copied(),
                    _ => None,
                };
                drop(query);

                let new_binding = match mapped.kind {
                    MapOutputKind::Node(erased) => {
                        let typed = erased.downcast_ref::<T>().cloned();
                        let id = self.arena.insert_erased(erased).cast::<T>();
                        if let Some(value) = typed {
                            let raw = id.into_raw();
                            let staging_id = *next_create_id;
                            *next_create_id += 1;
                            ops.push(scheme::Command::Create {
                                id: staging_id,
                                value,
                            });
                            ops.push(scheme::Command::Insert {
                                index: raw,
                                id: staging_id,
                            });
                        }
                        Some(id)
                    }
                    MapOutputKind::Alias(id) => Some(id.cast()),
                    MapOutputKind::ForwardChild(_) => forward_child_ast.flatten(),
                    MapOutputKind::Skip => None,
                };

                if let Some(old_id) = old_binding {
                    if new_binding != Some(old_id) {
                        if self.arena.remove_erased(old_id.cast()).is_some() {
                            ops.push(scheme::Command::Delete {
                                index: old_id.into_raw(),
                            });
                        }
                    }
                }

                self.greens.get_mut(&green).unwrap().binding = new_binding;
                continue;
            }

            if !visited.insert(green) {
                continue;
            }

            stack.push((green, true));
            let children = self.greens[&green].children.clone();
            for child in children.into_iter().rev() {
                if !visited.contains(&child) {
                    stack.push((child, false));
                }
            }
        }
    }

    fn compute_root(&self) -> Option<ASTCell<T>> {
        self.root_green.and_then(|g| {
            self.greens
                .get(&g)
                .and_then(|n| n.binding)
                .filter(|id| id.arena_ty == Some(type_name::<T>()))
        })
    }
}

impl<T, M> scheme::Pass<RedGreenTreeIR, AstArena<T>> for IncrementalLowerer<T, M>
where
    T: fmt::Debug + Clone + PartialEq + Send + 'static,
    M: AstMapper<T> + Send + 'static,
{
    type Error = std::convert::Infallible;

    fn transform(
        &mut self,
        upstream: &RedGreenTreeIR,
        txn: scheme::Transaction<RedGreenTreeIR>,
    ) -> Result<scheme::Transaction<AstArena<T>>, Self::Error> {
        Ok(self.transform_with_source(upstream, txn, ""))
    }
}
