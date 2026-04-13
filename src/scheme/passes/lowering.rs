//! Pass 2→3: incremental CST→AST transaction transformation through `AstMapper`.

use std::{cell::RefCell, fmt, sync::Arc};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    grammar::Grammar,
    parsec::{msg::ParserMessages, view::NodeView},
    scheme::{
        self, LayerObserver, ObserveError, URI,
        layers::{
            AstArena, AstCell, AstDelta, AstTxnBuilder, AstVec, DocumentNodePath, ParseTreeIR,
            ParseTreeQuery, ParseTreeValue, ast::AstMapAny,
        },
    },
};

type CstCommand = crate::scheme::LayerCommand<ParseTreeIR>;
type MapperHandler = Arc<dyn Fn(&AstMapCtx<'_>, &NodeView) -> Option<AstMapIntent> + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum AstMapAction {
    Skip,
    /// This CST node's AST slot is wherever `target_cst_path` resolves to.
    Forward(DocumentNodePath),
    Emit(AstMapAny),
}

#[derive(Debug, Clone)]
pub struct AstMapIntent {
    pub action: AstMapAction,
    /// Override the anchor CST path (default: the emitting node's own path).
    pub anchor: Option<DocumentNodePath>,
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
    pub fn forward(cst_path: DocumentNodePath) -> Self {
        Self::new(AstMapAction::Forward(cst_path))
    }

    pub fn with_anchor_path(mut self, path: DocumentNodePath) -> Self {
        self.anchor = Some(path);
        self
    }

    pub fn with_anchor_node(mut self, uri: &URI, node: &NodeView) -> Self {
        self.anchor = Some(DocumentNodePath(*uri, node.path().0.clone()));
        self
    }
}

pub type AstNode = NodeView;

struct QueriedParseTree<'a> {
    observer: &'a LayerObserver<ParseTreeIR>,
    views: RefCell<FxHashMap<DocumentNodePath, Option<NodeView>>>,
    messages: RefCell<FxHashMap<URI, ParserMessages>>,
    /// `Some(true)` = resolvable (retry), `Some(false)` = permanent, `None` = no failure.
    failure: RefCell<Option<bool>>,
}

impl<'a> QueriedParseTree<'a> {
    fn new(observer: &'a LayerObserver<ParseTreeIR>) -> Self {
        Self {
            observer,
            views: RefCell::new(FxHashMap::default()),
            messages: RefCell::new(FxHashMap::default()),
            failure: RefCell::new(None),
        }
    }

    fn note_failure(&self, is_resolvable: bool) {
        let mut slot = self.failure.borrow_mut();
        // Never upgrade from permanent (`false`) to resolvable (`true`).
        if *slot != Some(false) {
            *slot = Some(is_resolvable);
        }
    }

    fn take_failure(&self) -> Option<bool> {
        self.failure.borrow_mut().take()
    }

    fn view(&self, path: &DocumentNodePath) -> Option<NodeView> {
        if let Some(cached) = self.views.borrow().get(path).cloned() {
            return cached;
        }

        let view = if path.1.is_empty() {
            match self.observer.query(ParseTreeQuery::Path(path.clone())) {
                Ok(ParseTreeValue::View(view)) => Some(view),
                Ok(_) => None,
                Err(ObserveError::Absent) => None,
                Err(err) => {
                    self.note_failure(err.is_resolvable());
                    None
                }
            }
        } else {
            let parent = path.parent()?;
            let child_ix = *path.1.last()?;
            self.view(&parent)?.try_nth(child_ix).cloned()
        };

        self.views.borrow_mut().insert(path.clone(), view.clone());
        view
    }

    fn parser_messages(&self, uri: &URI) -> ParserMessages {
        if let Some(messages) = self.messages.borrow().get(uri).cloned() {
            return messages;
        }

        let messages = match self.observer.query(ParseTreeQuery::Message(*uri)) {
            Ok(ParseTreeValue::Messages(messages)) => messages,
            Err(ObserveError::Absent) => ParserMessages::default(),
            Err(err) => {
                self.note_failure(err.is_resolvable());
                ParserMessages::default()
            }
            Ok(_) => ParserMessages::default(),
        };
        self.messages.borrow_mut().insert(*uri, messages.clone());
        messages
    }
}

pub struct AstMapCtx<'a> {
    upstream: &'a QueriedParseTree<'a>,
    pub uri: &'a URI,
    resolve_ast_path: &'a dyn Fn(&NodeView) -> Option<DocumentNodePath>,
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
    /// Works for *any* `Debug + Clone + PartialEq + Send + Sync + 'static` type.
    /// In a heterogeneous mapper you can call `ctx.emit(Expr::Num(1))` in
    /// one handler and `ctx.emit(Type::Int)` in another.
    pub fn emit<V>(&self, value: V) -> AstMapIntent
    where
        V: fmt::Debug + Clone + PartialEq + Send + Sync + 'static,
    {
        AstMapIntent::emit(AstMapAny::new(value))
    }

    /// Produce a `Forward` intent to resolve at the given node's path.
    pub fn forward(&self, node: &NodeView) -> AstMapIntent {
        AstMapIntent::forward(DocumentNodePath(*self.uri, node.path().0.clone()))
    }

    /// Produce a `Skip` intent.
    pub fn skip(&self) -> AstMapIntent {
        AstMapIntent::skip()
    }

    /// Forward to the first child, or `None` if there are no children.
    pub fn try_forward_first(&self, node: &NodeView) -> Option<AstMapIntent> {
        let child = node.each().first()?;
        Some(AstMapIntent::forward(DocumentNodePath(
            *self.uri,
            child.path().0.clone(),
        )))
    }

    /// Forward to the first child, or skip if there are no children.
    pub fn forward_first(&self, node: &NodeView) -> AstMapIntent {
        node.each()
            .first()
            .map(|child| AstMapIntent::forward(DocumentNodePath(*self.uri, child.path().0.clone())))
            .unwrap_or_else(AstMapIntent::skip)
    }

    /// Build an [`AstVec`] rooted at `parent`'s CST path.
    ///
    /// The returned handle is **path-stable**: its identity is the parent's
    /// CST path, so the parent node's stored value does **not** change when
    /// children are inserted or removed.  Individual elements are managed by
    /// their own handler registrations and stored at direct-child paths of
    /// `parent`.
    ///
    /// ```ignore
    /// .on_rule("list", |ctx, node| {
    ///     Some(ctx.emit(Expr::List(ctx.collect_vec(node))))
    /// })
    /// // Elements handled separately:
    /// .on_rule("item", |ctx, node| Some(ctx.emit(Expr::Item(...))))
    /// ```
    pub fn collect_vec<U>(&self, parent: &NodeView) -> AstVec<U> {
        AstVec::new(DocumentNodePath(*self.uri, parent.path().0.clone()))
    }

    /// All parser-level diagnostics for the current transaction.
    pub fn parser_messages(&self) -> ParserMessages {
        self.upstream.parser_messages(self.uri)
    }

    /// Returns `true` if the current transaction has any parser errors.
    pub fn has_errors(&self) -> bool {
        !self.upstream.parser_messages(self.uri).is_empty()
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

    /// Register a rule handler that returns `AstMapIntent` directly (no `Option`).
    pub fn rule<F>(mut self, rule_name: &'static str, visitor: F) -> Self
    where
        F: Fn(&AstMapCtx<'_>, &NodeView) -> AstMapIntent + Send + Sync + 'static,
    {
        self.rule_visitors.insert(
            rule_name,
            Arc::new(move |ctx, node| Some(visitor(ctx, node))),
        );
        self
    }

    /// Register a field handler that returns `AstMapIntent` directly (no `Option`).
    pub fn field<F>(mut self, field_name: &'static str, visitor: F) -> Self
    where
        F: Fn(&AstMapCtx<'_>, &NodeView) -> AstMapIntent + Send + Sync + 'static,
    {
        self.field_visitors.insert(
            field_name,
            Arc::new(move |ctx, node| Some(visitor(ctx, node))),
        );
        self
    }

    /// Register a token handler that returns `AstMapIntent` directly (no `Option`).
    pub fn token<F>(mut self, token_name: &'static str, visitor: F) -> Self
    where
        F: Fn(&AstMapCtx<'_>, &NodeView) -> AstMapIntent + Send + Sync + 'static,
    {
        self.token_visitors.insert(
            token_name,
            Arc::new(move |ctx, node| Some(visitor(ctx, node))),
        );
        self
    }

    /// Register an error handler that returns `AstMapIntent` directly (no `Option`).
    pub fn error<F>(mut self, visitor: F) -> Self
    where
        F: Fn(&AstMapCtx<'_>, &NodeView) -> AstMapIntent + Send + Sync + 'static,
    {
        self.error_visitor = Some(Arc::new(move |ctx, node| Some(visitor(ctx, node))));
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
    mapper: AstMapper,
}

impl IncrementalLowerer {
    pub fn new(_grammar: &'static Grammar, mapper: impl Into<AstMapper>) -> Self {
        Self {
            mapper: mapper.into(),
        }
    }

    fn apply_from_queries(
        &mut self,
        upstream: &QueriedParseTree<'_>,
        uri: &URI,
        downstream: &AstArena<AstMapAny>,
        commands: &[CstCommand],
    ) -> AstDelta<AstMapAny> {
        let memo: RefCell<FxHashMap<DocumentNodePath, AstMapIntent>> =
            RefCell::new(FxHashMap::default());
        let resolving: RefCell<FxHashSet<DocumentNodePath>> = RefCell::new(FxHashSet::default());

        // Discover dirty anchors from the transaction:
        // - Root Insert (full parse): DFS-scan the entire upstream CST to find all anchors.
        // - Non-root Insert / Replace: walk up ancestor chain to the nearest mapped rule.
        //   The incremental reparser only emits a Replace for the changed leaf token;
        //   the containing rule node (e.g. `primary`) won't appear in the transaction
        //   but its subtree changed, so we must find it by walking up.
        let mut dirty_anchors: FxHashSet<DocumentNodePath> = FxHashSet::default();
        for cmd in commands {
            match cmd {
                CstCommand::Insert { index, .. } => {
                    let path = index;
                    if path.1.is_empty() {
                        // Root insert: this is a full parse — DFS the whole tree.
                        if let Some(root) = upstream.view(&DocumentNodePath::root(*uri)) {
                            self.collect_upstream_anchors(
                                upstream,
                                uri,
                                &root,
                                &memo,
                                &resolving,
                                &mut dirty_anchors,
                            );
                        }
                    } else {
                        // Non-root insert (incremental mid-tree insert).
                        if let Some(anchor) =
                            self.resolve_anchor_or_ancestor(upstream, uri, path, &memo, &resolving)
                        {
                            dirty_anchors.insert(anchor);
                        }
                    }
                }
                CstCommand::Replace { index, .. } => {
                    let path = index;
                    if let Some(anchor) =
                        self.resolve_anchor_or_ancestor(upstream, uri, path, &memo, &resolving)
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
        let stale: Vec<DocumentNodePath> = downstream
            .storage
            .nodes
            .keys()
            .filter(|p| p.0 == *uri)
            .filter(|p| upstream.view(p).is_none())
            .cloned()
            .collect();
        for p in stale {
            builder.delete(p.clone());
        }

        // Delete stale AST nodes that still exist at a CST path but no longer
        // map to themselves after recovery reshapes a dirty subtree.
        //
        // Example: an `on_error()` node may have emitted `Json::Error` at path
        // `P.4`, then after more input that same CST slot becomes a skipped
        // token or part of another structure. The path still exists in the CST,
        // so the simple `green_at_path == None` cleanup above does not remove
        // it. We must sweep existing AST nodes under dirty anchors and delete
        // any path that no longer resolves to itself as a live AST anchor.
        let stale_in_dirty: Vec<DocumentNodePath> = downstream
            .storage
            .nodes
            .keys()
            .filter(|p| p.0 == *uri)
            .filter(|existing_path| {
                dirty_anchors
                    .iter()
                    .any(|dirty| dirty.is_prefix_of(existing_path))
            })
            .filter(|existing_path| upstream.view(existing_path).is_some())
            .filter(|existing_path| {
                self.resolve_anchor_path(upstream, uri, existing_path, &memo, &resolving)
                    .as_ref()
                    != Some(*existing_path)
            })
            .cloned()
            .collect();
        for p in stale_in_dirty {
            builder.delete(p);
        }

        // Re-lower dirty anchors deepest-first so child anchors are settled
        // before parent anchors that reference them via read_cell.
        let mut dirty: Vec<DocumentNodePath> = dirty_anchors.into_iter().collect();
        dirty.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.1.cmp(&b.1)));

        for anchor in dirty {
            self.relower_anchor(
                upstream,
                uri,
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
        upstream: &QueriedParseTree<'_>,
        uri: &URI,
        downstream: &AstArena<AstMapAny>,
        anchor_path: &DocumentNodePath,
        builder: &mut AstTxnBuilder<AstMapAny>,
        memo: &RefCell<FxHashMap<DocumentNodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<DocumentNodePath>>,
    ) {
        let Some(intent) = self.resolve_intent(upstream, uri, anchor_path, memo, resolving) else {
            return;
        };

        let AstMapIntent {
            action,
            anchor: declared_anchor,
        } = intent;
        let anchor = declared_anchor.unwrap_or_else(|| anchor_path.clone());

        match action {
            AstMapAction::Emit(value) => {
                // Compare against erased stored value to suppress no-ops.
                match downstream.get_erased_as_any(&anchor) {
                    Some(existing) if existing == value => return, // already correct
                    None => builder.insert_value(anchor, value),   // new anchor
                    Some(_) => builder.replace_value(anchor, value), // updated anchor
                }
            }
            AstMapAction::Skip | AstMapAction::Forward(_) => {
                // Important: a CST path may previously have emitted a value
                // (for example via `on_error`) and later become an unmapped
                // token or forwarding node after recovery stabilizes. In that
                // case the AST node at the old anchor must be deleted even
                // though the CST path itself still exists.
                if downstream.get_erased_as_any(anchor_path).is_some() {
                    builder.delete(anchor_path.clone());
                }
            }
        }
    }

    fn resolve_intent(
        &self,
        upstream: &QueriedParseTree<'_>,
        uri: &URI,
        cst_path: &DocumentNodePath,
        memo: &RefCell<FxHashMap<DocumentNodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<DocumentNodePath>>,
    ) -> Option<AstMapIntent> {
        if let Some(cached) = memo.borrow().get(cst_path).cloned() {
            return Some(cached);
        }

        let node = upstream.view(cst_path)?;
        if !resolving.borrow_mut().insert(cst_path.clone()) {
            return Some(AstMapIntent::skip());
        }

        let resolve_ast_path = |child: &NodeView| {
            let child_path = DocumentNodePath(*uri, child.path().0.clone());
            self.resolve_ast_path(upstream, uri, &child_path, memo, resolving)
        };
        let ctx = AstMapCtx {
            upstream,
            uri,
            resolve_ast_path: &resolve_ast_path,
        };

        let intent = self.mapper.map(&ctx, &node);
        resolving.borrow_mut().remove(cst_path);
        memo.borrow_mut().insert(cst_path.clone(), intent.clone());
        Some(intent)
    }

    fn resolve_ast_path(
        &self,
        upstream: &QueriedParseTree<'_>,
        uri: &URI,
        cst_path: &DocumentNodePath,
        memo: &RefCell<FxHashMap<DocumentNodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<DocumentNodePath>>,
    ) -> Option<DocumentNodePath> {
        let intent = self.resolve_intent(upstream, uri, cst_path, memo, resolving)?;
        match intent.action {
            AstMapAction::Skip => None,
            AstMapAction::Forward(target) => {
                self.resolve_ast_path(upstream, uri, &target, memo, resolving)
            }
            AstMapAction::Emit(_) => Some(intent.anchor.unwrap_or_else(|| cst_path.clone())),
        }
    }

    fn collect_upstream_anchors(
        &self,
        upstream: &QueriedParseTree<'_>,
        uri: &URI,
        root: &NodeView,
        memo: &RefCell<FxHashMap<DocumentNodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<DocumentNodePath>>,
        dirty_anchors: &mut FxHashSet<DocumentNodePath>,
    ) {
        // Iterative DFS to avoid stack overflow on deep trees.
        let mut stack: Vec<NodeView> = vec![root.clone()];
        while let Some(node) = stack.pop() {
            let path = DocumentNodePath(*uri, node.path().0.clone());
            if let Some(anchor) = self.resolve_anchor_path(upstream, uri, &path, memo, resolving) {
                dirty_anchors.insert(anchor);
                // Still recurse: children may be independent mapped anchors
                // (e.g. `primary` and `mul` are both inside `add`'s subtree).
            }
            for child in node.each().iter().rev() {
                stack.push(child.clone());
            }
        }
    }

    fn resolve_anchor_or_ancestor(
        &self,
        upstream: &QueriedParseTree<'_>,
        uri: &URI,
        cst_path: &DocumentNodePath,
        memo: &RefCell<FxHashMap<DocumentNodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<DocumentNodePath>>,
    ) -> Option<DocumentNodePath> {
        // Try the path itself first (covers full-parse case where every node appears).
        if let Some(anchor) = self.resolve_anchor_path(upstream, uri, cst_path, memo, resolving) {
            return Some(anchor);
        }
        // Walk up the ancestor chain (covers incremental case where only the leaf appears).
        let mut cur = cst_path.parent()?;
        loop {
            if let Some(anchor) = self.resolve_anchor_path(upstream, uri, &cur, memo, resolving) {
                return Some(anchor);
            }
            cur = cur.parent()?;
        }
    }

    fn resolve_anchor_path(
        &self,
        upstream: &QueriedParseTree<'_>,
        uri: &URI,
        cst_path: &DocumentNodePath,
        memo: &RefCell<FxHashMap<DocumentNodePath, AstMapIntent>>,
        resolving: &RefCell<FxHashSet<DocumentNodePath>>,
    ) -> Option<DocumentNodePath> {
        let intent = self.resolve_intent(upstream, uri, cst_path, memo, resolving)?;
        match intent.action {
            AstMapAction::Emit(_) => Some(intent.anchor.unwrap_or_else(|| cst_path.clone())),
            AstMapAction::Forward(target) => {
                self.resolve_anchor_path(upstream, uri, &target, memo, resolving)
            }
            AstMapAction::Skip => None,
        }
    }
}

impl scheme::Pass<ParseTreeIR, AstArena<AstMapAny>> for IncrementalLowerer {
    fn push(
        &mut self,
        upstream: &LayerObserver<ParseTreeIR>,
        downstream: &AstArena<AstMapAny>,
        txn: &[scheme::LayerCommand<ParseTreeIR>],
    ) -> Vec<scheme::LayerCommand<AstArena<AstMapAny>>> {
        let upstream = QueriedParseTree::new(upstream);
        let Some(uri) = extract_uri_from_commands(txn) else {
            return Vec::new();
        };
        let commands = self.apply_from_queries(&upstream, &uri, downstream, txn);
        if upstream.take_failure().is_some() {
            return Vec::new();
        }
        commands
    }

    fn resolve(
        &mut self,
        upstream: &LayerObserver<ParseTreeIR>,
        downstream: &AstArena<AstMapAny>,
        index: DocumentNodePath,
    ) -> scheme::ResolveOutcome<AstArena<AstMapAny>> {
        let probe = vec![scheme::Command::Replace {
            index: index.clone(),
            id: usize::MAX,
        }];
        let queried = QueriedParseTree::new(upstream);
        let commands = self.apply_from_queries(&queried, &index.0, downstream, &probe);
        match queried.take_failure() {
            Some(true) => scheme::ResolveOutcome::Blocked,
            Some(false) => scheme::ResolveOutcome::Impossible,
            None if commands.is_empty() => scheme::ResolveOutcome::Impossible,
            None => scheme::ResolveOutcome::Done(Arc::new(commands)),
        }
    }
}

/// Extract the document URI from a CST command batch.
/// Returns `None` only if the batch is empty or contains no path/message commands.
fn extract_uri_from_commands(commands: &[CstCommand]) -> Option<URI> {
    commands.iter().find_map(|cmd| {
        let index = match cmd {
            CstCommand::Insert { index, .. } | CstCommand::Replace { index, .. } => index,
            CstCommand::Delete { index } => index,
            CstCommand::Create { .. } => return None,
        };
        Some(index.0)
    })
}
