//! Pass 2→3: incremental CST→AST transaction transformation through `AstMapper`.

use std::{cell::RefCell, fmt, sync::Arc};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    grammar::Grammar,
    parsec::{
        msg::ParserMessages,
        tree::{Tag, TreeAllocRefExt},
        view::NodeView,
    },
    scheme::{
        self,
        layers::{AstArena, AstCell, AstDelta, AstMapAny, AstTxnBuilder, NodePath, ParseTreeIR},
    },
};

type CstCommand = crate::scheme::Command<ParseTreeIR>;
type MapperHandler = Arc<dyn Fn(&AstMapCtx<'_>, &NodeView) -> Option<AstMapIntent> + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum AstMapAction {
    Skip,
    /// This CST node's AST slot is wherever `target_cst_path` resolves to.
    Forward(NodePath),
    Emit(AstMapAny),
}

#[derive(Debug, Clone)]
pub struct AstMapIntent {
    pub action: AstMapAction,
    /// Override the anchor CST path (default: the emitting node's own path).
    pub anchor: Option<NodePath>,
}

impl AstMapIntent {
    fn new(action: AstMapAction) -> Self {
        Self {
            action,
            anchor: None,
        }
    }

    pub fn skip() -> Self {
        Self::new(AstMapAction::Skip)
    }

    pub fn emit(value: AstMapAny) -> Self {
        Self::new(AstMapAction::Emit(value))
    }

    /// Forward: this CST node's AST slot is wherever `cst_path` resolves.
    pub fn forward(cst_path: NodePath) -> Self {
        Self::new(AstMapAction::Forward(cst_path))
    }

    pub fn with_anchor_path(mut self, path: NodePath) -> Self {
        self.anchor = Some(path);
        self
    }

    pub fn with_anchor_node(mut self, node: &NodeView) -> Self {
        self.anchor = Some(node.path().clone());
        self
    }
}

pub type AstNode = NodeView;

pub struct AstMapCtx<'a> {
    pub upstream: &'a ParseTreeIR,
    resolve_ast_path: &'a dyn Fn(&NodeView) -> Option<NodePath>,
}

impl<'a> AstMapCtx<'a> {
    /// Resolve `node` to its AST anchor and return a typed `AstCell`.
    /// The phantom type `U` is inferred from usage context (e.g. the field
    /// type in the emitted enum variant).
    pub fn read_cell<U>(&self, node: &NodeView) -> Option<AstCell<U>> {
        let ast_path = (self.resolve_ast_path)(node)?;
        Some(AstCell::from_path(&ast_path))
    }

    /// Read the trimmed source text of a CST node.
    pub fn read_text(&self, node: &NodeView) -> String {
        node.text_trimmed()
    }

    /// Type-erase and emit `value` as the AST result for this CST node.
    ///
    /// Works for *any* `Debug + Clone + PartialEq + Send + 'static` type.
    /// In a heterogeneous mapper you can call `ctx.emit(Expr::Num(1))` in
    /// one handler and `ctx.emit(Type::Int)` in another.
    pub fn emit<V>(&self, value: V) -> AstMapIntent
    where
        V: fmt::Debug + Clone + PartialEq + Send + 'static,
    {
        AstMapIntent::emit(AstMapAny::new(value))
    }

    /// Produce a `Forward` intent to resolve at the given node's path.
    pub fn forward(&self, node: &NodeView) -> AstMapIntent {
        AstMapIntent::forward(node.path().clone())
    }

    /// Produce a `Skip` intent.
    pub fn skip(&self) -> AstMapIntent {
        AstMapIntent::skip()
    }

    /// Forward to the first child, or `None` if there are no children.
    pub fn forward_first_child(&self, node: &NodeView) -> Option<AstMapIntent> {
        let child = node.each().first()?;
        Some(AstMapIntent::forward(child.path().clone()))
    }

    /// All parser-level diagnostics for the current transaction.
    pub fn parser_messages(&self) -> ParserMessages {
        self.upstream.parser_messages()
    }

    /// Returns `true` if the current transaction has any parser errors.
    pub fn has_errors(&self) -> bool {
        !self.upstream.parser_messages().is_empty()
    }
}

#[derive(Clone)]
pub struct AstMapper {
    rule_visitors: FxHashMap<&'static str, MapperHandler>,
    field_visitors: FxHashMap<&'static str, MapperHandler>,
    token_visitors: FxHashMap<&'static str, MapperHandler>,
    error_visitor: Option<MapperHandler>,
    skip_rules: FxHashSet<&'static str>,
    skip_fields: FxHashSet<&'static str>,
}

impl Default for AstMapper {
    fn default() -> Self {
        Self {
            rule_visitors: FxHashMap::default(),
            field_visitors: FxHashMap::default(),
            token_visitors: FxHashMap::default(),
            error_visitor: None,
            skip_rules: FxHashSet::default(),
            skip_fields: FxHashSet::default(),
        }
    }
}

impl From<()> for AstMapper {
    fn from(_: ()) -> Self {
        Self::new()
    }
}

impl AstMapper {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_rule<F>(mut self, rule_name: &'static str, visitor: F) -> Self
    where
        F: Fn(&AstMapCtx<'_>, &NodeView) -> Option<AstMapIntent> + Send + Sync + 'static,
    {
        self.rule_visitors.insert(rule_name, Arc::new(visitor));
        self
    }

    pub fn on_field<F>(mut self, field_name: &'static str, visitor: F) -> Self
    where
        F: Fn(&AstMapCtx<'_>, &NodeView) -> Option<AstMapIntent> + Send + Sync + 'static,
    {
        self.field_visitors.insert(field_name, Arc::new(visitor));
        self
    }

    pub fn on_token<F>(mut self, token_name: &'static str, visitor: F) -> Self
    where
        F: Fn(&AstMapCtx<'_>, &NodeView) -> Option<AstMapIntent> + Send + Sync + 'static,
    {
        self.token_visitors.insert(token_name, Arc::new(visitor));
        self
    }

    pub fn on_error<F>(mut self, visitor: F) -> Self
    where
        F: Fn(&AstMapCtx<'_>, &NodeView) -> Option<AstMapIntent> + Send + Sync + 'static,
    {
        self.error_visitor = Some(Arc::new(visitor));
        self
    }

    pub fn skip_rule(mut self, rule_name: &'static str) -> Self {
        self.skip_rules.insert(rule_name);
        self
    }

    pub fn skip_field(mut self, field_name: &'static str) -> Self {
        self.skip_fields.insert(field_name);
        self
    }

    pub fn map(&self, ctx: &AstMapCtx<'_>, node: &NodeView) -> AstMapIntent {
        self.dispatch(ctx, node)
    }

    fn dispatch(&self, ctx: &AstMapCtx<'_>, node: &NodeView) -> AstMapIntent {
        if let Some(field_name) = node.field_name() {
            if let Some(visitor) = self.field_visitors.get(field_name) {
                return visitor(ctx, node).unwrap_or_else(AstMapIntent::skip);
            }
            debug_assert!(
                self.skip_fields.contains(field_name),
                "AstMapper: no handler registered for field '{field_name}'. \
                 Add .on_field(\"{field_name}\", ...) or .skip_field(\"{field_name}\")."
            );
            return AstMapIntent::skip();
        }

        if let Some(rule_name) = node.rule_name() {
            if let Some(visitor) = self.rule_visitors.get(rule_name) {
                return visitor(ctx, node).unwrap_or_else(AstMapIntent::skip);
            }
            debug_assert!(
                self.skip_rules.contains(rule_name),
                "AstMapper: no handler registered for rule '{rule_name}'. \
                 Add .on_rule(\"{rule_name}\", ...) or .skip_rule(\"{rule_name}\")."
            );
            return AstMapIntent::skip();
        }

        if let Some(token_name) = node.token_name() {
            if let Some(visitor) = self.token_visitors.get(token_name) {
                return visitor(ctx, node).unwrap_or_else(AstMapIntent::skip);
            }
            return AstMapIntent::skip();
        }

        if node.error().is_some() {
            if let Some(visitor) = &self.error_visitor {
                return visitor(ctx, node).unwrap_or_else(AstMapIntent::skip);
            }
        }

        AstMapIntent::skip()
    }
}

pub struct IncrementalLowerer {
    grammar: &'static Grammar,
    mapper: AstMapper,
}

impl IncrementalLowerer {
    pub fn new(grammar: &'static Grammar, mapper: impl Into<AstMapper>) -> Self {
        Self {
            grammar,
            mapper: mapper.into(),
        }
    }

    fn apply_from_ir(
        &mut self,
        upstream: &ParseTreeIR,
        downstream: &AstArena<AstMapAny>,
        commands: &[CstCommand],
    ) -> AstDelta<AstMapAny> {
        let memo: RefCell<FxHashMap<NodePath, AstMapIntent>> = RefCell::new(FxHashMap::default());
        let resolving: RefCell<FxHashSet<NodePath>> = RefCell::new(FxHashSet::default());

        // Discover dirty anchors from the transaction:
        // - Root Insert (full parse): DFS-scan the entire upstream CST to find all anchors.
        // - Non-root Insert / Replace: walk up ancestor chain to the nearest mapped rule.
        //   The incremental reparser only emits a Replace for the changed leaf token;
        //   the containing rule node (e.g. `primary`) won't appear in the transaction
        //   but its subtree changed, so we must find it by walking up.
        let mut dirty_anchors: FxHashSet<NodePath> = FxHashSet::default();
        for cmd in commands {
            match cmd {
                CstCommand::Insert { index, .. } => {
                    let crate::scheme::layers::ParseTreeQuery::Path(path) = index else {
                        continue;
                    };
                    if path.0.is_empty() {
                        // Root insert: this is a full parse — DFS the whole tree.
                        self.collect_upstream_anchors(
                            upstream,
                            &NodePath::root(),
                            &memo,
                            &resolving,
                            &mut dirty_anchors,
                        );
                    } else {
                        // Non-root insert (incremental mid-tree insert).
                        if let Some(anchor) =
                            self.resolve_anchor_or_ancestor(upstream, path, &memo, &resolving)
                        {
                            dirty_anchors.insert(anchor);
                        }
                    }
                }
                CstCommand::Replace { index, .. } => {
                    let crate::scheme::layers::ParseTreeQuery::Path(path) = index else {
                        continue;
                    };
                    if let Some(anchor) =
                        self.resolve_anchor_or_ancestor(upstream, path, &memo, &resolving)
                    {
                        dirty_anchors.insert(anchor);
                    }
                }
                CstCommand::SetRoot { .. } => {
                    if let Some(anchor) =
                        self.resolve_anchor_path(upstream, &NodePath::root(), &memo, &resolving)
                    {
                        dirty_anchors.insert(anchor);
                    }
                }
                CstCommand::Delete { .. } | CstCommand::Create { .. } => {}
            }
        }

        let mut builder: AstTxnBuilder<AstMapAny> = AstTxnBuilder::new();

        // Delete AST anchors whose CST source path no longer exists in the tree.
        // `downstream.storage.nodes` is the ground truth for currently-live anchors.
        let stale: Vec<NodePath> = downstream
            .storage
            .nodes
            .keys()
            .filter(|p| upstream.green_at_path(p).is_none())
            .cloned()
            .collect();
        for p in stale {
            builder.delete(p.clone());
        }

        // Re-lower dirty anchors deepest-first so child anchors are settled
        // before parent anchors that reference them via read_cell.
        let mut dirty: Vec<NodePath> = dirty_anchors.into_iter().collect();
        dirty.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));

        for anchor in dirty {
            self.relower_anchor(
                upstream,
                downstream,
                &anchor,
                &mut builder,
                &memo,
                &resolving,
            );
        }

        builder.finish()
    }

    fn relower_anchor(
        &mut self,
        upstream: &ParseTreeIR,
        downstream: &AstArena<AstMapAny>,
        anchor_path: &NodePath,
        builder: &mut AstTxnBuilder<AstMapAny>,
        memo: &RefCell<FxHashMap<NodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<NodePath>>,
    ) {
        let Some(intent) = self.resolve_intent(upstream, anchor_path, memo, resolving) else {
            return;
        };

        let AstMapIntent {
            action,
            anchor: declared_anchor,
        } = intent;
        let anchor = declared_anchor.unwrap_or_else(|| anchor_path.clone());

        if let AstMapAction::Emit(value) = action {
            // Compare against erased stored value to suppress no-ops.
            match downstream.get_erased_as_any(&anchor) {
                Some(existing) if existing == value => return, // already correct
                None => builder.insert_value(anchor, value),   // new anchor
                Some(_) => builder.replace_value(anchor, value), // updated anchor
            }
        }
    }

    fn resolve_intent(
        &self,
        upstream: &ParseTreeIR,
        cst_path: &NodePath,
        memo: &RefCell<FxHashMap<NodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<NodePath>>,
    ) -> Option<AstMapIntent> {
        if let Some(cached) = memo.borrow().get(cst_path).cloned() {
            return Some(cached);
        }

        let green = upstream.green_at_path(cst_path)?;
        let offset = upstream.offset_at_path(cst_path)?;
        let mut node = upstream
            .viewer(self.grammar)
            .node(green, offset)
            .with_path(cst_path.clone());

        // Attach grammar-derived field name from parent context.
        // `ParseTreeIR` uses `Tag::Rule` for everything (field wrappers are
        // transparent in the protocol), so the only way to know a child is at a
        // named field position is to look at the parent production's field_positions.
        if let Some(child_ix) = cst_path.0.last().copied() {
            if let Some(parent_path) = cst_path.parent() {
                if let Some(parent_green) = upstream.green_at_path(&parent_path) {
                    let parent_node = upstream.alloc.node(parent_green);
                    if let Tag::Rule {
                        rule_ix: parent_rule_ix,
                        ..
                    } = &parent_node.tag
                    {
                        let n_siblings = parent_node.children.len();
                        'field_lookup: for prod in &self.grammar.table.productions {
                            if prod.lhs == *parent_rule_ix && prod.rhs.len() == n_siblings {
                                for &(pos, name) in &prod.field_positions {
                                    if pos == child_ix {
                                        node = node.with_grammar_field_name(name);
                                        break 'field_lookup;
                                    }
                                }
                                break 'field_lookup;
                            }
                        }
                    }
                }
            }
        }
        if !resolving.borrow_mut().insert(cst_path.clone()) {
            return Some(AstMapIntent::skip());
        }

        let resolve_ast_path =
            |child: &NodeView| self.resolve_ast_path(upstream, child.path(), memo, resolving);
        let ctx = AstMapCtx {
            upstream,
            resolve_ast_path: &resolve_ast_path,
        };

        let intent = self.mapper.map(&ctx, &node);
        resolving.borrow_mut().remove(cst_path);
        memo.borrow_mut().insert(cst_path.clone(), intent.clone());
        Some(intent)
    }

    fn resolve_ast_path(
        &self,
        upstream: &ParseTreeIR,
        cst_path: &NodePath,
        memo: &RefCell<FxHashMap<NodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<NodePath>>,
    ) -> Option<NodePath> {
        let intent = self.resolve_intent(upstream, cst_path, memo, resolving)?;
        match intent.action {
            AstMapAction::Skip => None,
            AstMapAction::Forward(target) => {
                self.resolve_ast_path(upstream, &target, memo, resolving)
            }
            AstMapAction::Emit(_) => Some(intent.anchor.unwrap_or_else(|| cst_path.clone())),
        }
    }

    fn collect_upstream_anchors(
        &self,
        upstream: &ParseTreeIR,
        root_path: &NodePath,
        memo: &RefCell<FxHashMap<NodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<NodePath>>,
        dirty_anchors: &mut FxHashSet<NodePath>,
    ) {
        // Iterative DFS to avoid stack overflow on deep trees.
        let mut stack: Vec<NodePath> = vec![root_path.clone()];
        while let Some(path) = stack.pop() {
            if let Some(anchor) = self.resolve_anchor_path(upstream, &path, memo, resolving) {
                dirty_anchors.insert(anchor);
                // Still recurse: children may be independent mapped anchors
                // (e.g. `primary` and `mul` are both inside `add`'s subtree).
            }
            if let Some(green) = upstream.green_at_path(&path) {
                let node = upstream.alloc.node(green);
                let n_children = node.children.len();
                drop(node);
                for ix in 0..n_children {
                    let mut child_path = path.0.clone();
                    child_path.push(ix);
                    stack.push(NodePath(child_path));
                }
            }
        }
    }

    fn resolve_anchor_or_ancestor(
        &self,
        upstream: &ParseTreeIR,
        cst_path: &NodePath,
        memo: &RefCell<FxHashMap<NodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<NodePath>>,
    ) -> Option<NodePath> {
        // Try the path itself first (covers full-parse case where every node appears).
        if let Some(anchor) = self.resolve_anchor_path(upstream, cst_path, memo, resolving) {
            return Some(anchor);
        }
        // Walk up the ancestor chain (covers incremental case where only the leaf appears).
        let mut cur = cst_path.parent()?;
        loop {
            if let Some(anchor) = self.resolve_anchor_path(upstream, &cur, memo, resolving) {
                return Some(anchor);
            }
            cur = cur.parent()?;
        }
    }

    fn resolve_anchor_path(
        &self,
        upstream: &ParseTreeIR,
        cst_path: &NodePath,
        memo: &RefCell<FxHashMap<NodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<NodePath>>,
    ) -> Option<NodePath> {
        let intent = self.resolve_intent(upstream, cst_path, memo, resolving)?;
        match intent.action {
            AstMapAction::Emit(_) => Some(intent.anchor.unwrap_or_else(|| cst_path.clone())),
            AstMapAction::Forward(target) => {
                self.resolve_anchor_path(upstream, &target, memo, resolving)
            }
            AstMapAction::Skip => None,
        }
    }
}

impl scheme::Pass<ParseTreeIR, AstArena<AstMapAny>> for IncrementalLowerer {
    type Error = std::convert::Infallible;

    fn transform(
        &mut self,
        upstream: &ParseTreeIR,
        downstream: &AstArena<AstMapAny>,
        txn: scheme::Transaction<ParseTreeIR>,
    ) -> Result<scheme::Transaction<AstArena<AstMapAny>>, Self::Error> {
        Ok(Arc::new(self.apply_from_ir(upstream, downstream, &txn)))
    }
}
