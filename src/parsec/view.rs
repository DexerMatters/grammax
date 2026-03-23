use std::{
    any::{Any, TypeId},
    fmt::Display,
    ops::Index,
    sync::{Arc, OnceLock},
};

use rustc_hash::FxHashMap;

use crate::{
    grammar::Grammar,
    parsec::{
        display::format_ast,
        parser,
        tree::{ParsecError, RedNode, Tag, TreeAllocRef, TreeAllocRefExt},
    },
    scheme::{Span, layers::NodePath},
};

/// Action returned by typed visitor closures.
pub enum ViewAction<T> {
    /// Delegate to the default traversal for the requested type.
    Relay,
    /// Return a specific value and stop descending.
    Exact(T),
}

enum ErasedViewAction {
    Relay,
    Exact(Box<dyn Any>),
}

type ErasedHandler = Arc<dyn Fn(&Viewer, &NodeView) -> ErasedViewAction>;

pub struct NodeView {
    grammar: &'static Grammar,
    alloc: TreeAllocRef,
    source: Arc<str>,
    token_texts: Arc<FxHashMap<usize, String>>,
    green: usize,
    offset: usize,
    path: NodePath,
    children: OnceLock<Vec<NodeView>>,
    /// Grammar-derived field name for this node (set by parent's `each()` or by
    /// `resolve_intent` from parent context). Takes priority over the raw tag.
    grammar_field_name: Option<&'static str>,
}

impl Clone for NodeView {
    fn clone(&self) -> Self {
        Self {
            grammar: self.grammar,
            alloc: self.alloc.clone(),
            source: self.source.clone(),
            token_texts: self.token_texts.clone(),
            green: self.green,
            offset: self.offset,
            path: self.path.clone(),
            children: OnceLock::new(),
            grammar_field_name: self.grammar_field_name,
        }
    }
}

impl NodeView {
    pub(crate) fn init(
        grammar: &'static Grammar,
        alloc: TreeAllocRef,
        source: impl Into<Arc<str>>,
        token_texts: Arc<FxHashMap<usize, String>>,
        green: usize,
        offset: usize,
    ) -> Self {
        Self {
            grammar,
            alloc,
            source: source.into(),
            token_texts,
            green,
            offset,
            path: NodePath::root(),
            children: OnceLock::new(),
            grammar_field_name: None,
        }
    }

    pub fn new(result: &parser::Result) -> Self {
        Self::init(
            result.grammar,
            result.alloc.clone(),
            result.source,
            Arc::new(FxHashMap::default()),
            result.root.green,
            0,
        )
    }

    pub fn from_specs(
        grammar: &'static Grammar,
        alloc: TreeAllocRef,
        source: impl Into<Arc<str>>,
        green: usize,
        offset: usize,
    ) -> Self {
        Self::init(
            grammar,
            alloc,
            source,
            Arc::new(FxHashMap::default()),
            green,
            offset,
        )
    }

    pub fn with_path(mut self, path: NodePath) -> Self {
        self.path = path;
        self
    }

    pub fn path(&self) -> &NodePath {
        &self.path
    }

    /// Override the field name with a grammar-derived value.
    /// Used when the parent rule's production designates this child's position
    /// as a named field, even though `ParseTreeIR` stores no `Tag::Field` wrapper.
    pub fn with_grammar_field_name(mut self, name: &'static str) -> Self {
        self.grammar_field_name = Some(name);
        self
    }

    pub fn text(&self) -> String {
        let node = self.alloc.node(self.green);
        if matches!(&node.tag, Tag::Token { .. } | Tag::Error(_)) {
            if let Some(text) = self.token_texts.get(&self.green) {
                return text.clone();
            }
            // Fall back to source slice when token_texts is not populated.
            if !self.source.is_empty() {
                let width = node.width;
                let start = self.offset.min(self.source.len());
                let end = self.offset.saturating_add(width).min(self.source.len());
                return self.source[start..end].to_string();
            }
            return String::new();
        }

        if !self.source.is_empty() {
            let width = node.width;
            let start = self.offset.min(self.source.len());
            let end = self.offset.saturating_add(width).min(self.source.len());
            return self.source[start..end].to_string();
        }

        self.each()
            .iter()
            .map(NodeView::text)
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn green(&self) -> usize {
        self.green
    }

    pub fn text_trimmed(&self) -> String {
        self.text().trim().to_string()
    }

    pub fn text_normalized(&self) -> String {
        let input = self.text();
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars();

        while let Some(ch) = chars.next() {
            if ch != '\\' {
                out.push(ch);
                continue;
            }

            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        }

        out
    }

    pub fn span_bytes(&self) -> (usize, usize) {
        let node = self.alloc.node(self.green);
        (self.offset, self.offset + node.width)
    }

    pub fn span(&self) -> Span {
        let (start, end) = self.span_bytes();
        Span::new(start, end)
    }

    pub fn rule_name(&self) -> Option<&'static str> {
        let node = self.alloc.node(self.green);
        match &node.tag {
            Tag::Rule { rule_ix, .. } => Some(self.grammar.name(*rule_ix)),
            _ => None,
        }
    }

    pub fn field_name(&self) -> Option<&'static str> {
        // Grammar-derived hint (set by parent's each() or resolve_intent) takes priority.
        if let Some(name) = self.grammar_field_name {
            return Some(name);
        }
        let node = self.alloc.node(self.green);
        match &node.tag {
            Tag::Field { name, .. } => Some(name),
            _ => None,
        }
    }

    pub fn token_name(&self) -> Option<&str> {
        let node = self.alloc.node(self.green);
        let Tag::Token { rule_ix } = &node.tag else {
            return None;
        };
        self.grammar.table.terminals[*rule_ix]
            .preview()
            .or_else(|| Some(self.grammar.table.terminals[*rule_ix].display().leak()))
    }

    pub fn error(&self) -> Option<ParsecError> {
        let node = self.alloc.node(self.green);
        match &node.tag {
            Tag::Error(err) => Some(err.clone()),
            _ => None,
        }
    }

    pub fn name(&self) -> Option<&'static str> {
        self.field_name().or_else(|| self.rule_name())
    }

    pub fn is_leaf(&self) -> bool {
        let node = self.alloc.node(self.green);
        matches!(&node.tag, Tag::Token { .. } | Tag::Error(_))
    }

    pub fn each(&self) -> &[NodeView] {
        self.children.get_or_init(|| {
            let node = self.alloc.node(self.green);
            let n_children = node.children.len();

            // Build a child-index → field-name map from the grammar's production table.
            // Works for `ParseTreeIR` which stores everything as `Tag::Rule` (no
            // `Tag::Field` wrappers), making the grammar-derived name the only source.
            let field_map: Vec<Option<&'static str>> = if let Tag::Rule { rule_ix, .. } = &node.tag
            {
                let rule_ix = *rule_ix;
                let mut map = vec![None; n_children];
                'outer: for prod in &self.grammar.table.productions {
                    if prod.lhs == rule_ix && prod.rhs.len() == n_children {
                        for &(pos, name) in &prod.field_positions {
                            if pos < n_children {
                                map[pos] = Some(name);
                            }
                        }
                        break 'outer;
                    }
                }
                map
            } else {
                vec![None; n_children]
            };

            let mut out = Vec::with_capacity(n_children);
            let mut child_offset = self.offset;
            for (ix, &child) in node.children.iter().enumerate() {
                let width = self.alloc.width_of(child);
                let mut child_path = self.path.0.clone();
                child_path.push(ix);
                let child_view = Self::init(
                    self.grammar,
                    self.alloc.clone(),
                    self.source.clone(),
                    self.token_texts.clone(),
                    child,
                    child_offset,
                )
                .with_path(NodePath(child_path));
                let child_view = match field_map[ix] {
                    Some(name) => child_view.with_grammar_field_name(name),
                    None => child_view,
                };
                out.push(child_view);
                child_offset += width;
            }
            out
        })
    }

    pub fn try_first(&self) -> Option<&NodeView> {
        self.each().first()
    }

    pub fn try_nth(&self, index: usize) -> Option<&NodeView> {
        self.each().get(index)
    }

    pub fn try_last(&self) -> Option<&NodeView> {
        self.each().last()
    }

    pub fn nth(&self, index: usize) -> &NodeView {
        self.try_nth(index).unwrap_or_else(|| {
            panic!(
                "NodeView: missing child {} for node {}",
                index,
                self.name().unwrap_or("?")
            )
        })
    }

    pub fn last(&self) -> &NodeView {
        self.try_last().unwrap_or_else(|| {
            panic!(
                "NodeView: missing last child for node {}",
                self.name().unwrap_or("?")
            )
        })
    }

    pub fn first(&self) -> &NodeView {
        self.try_first().unwrap_or_else(|| {
            panic!(
                "NodeView: missing first child for node {}",
                self.name().unwrap_or("?")
            )
        })
    }

    pub fn each_with_rule<'a>(
        &'a self,
        rule_name: &'a str,
    ) -> impl Iterator<Item = &'a NodeView> + 'a {
        self.each()
            .iter()
            .filter(move |child| child.rule_name() == Some(rule_name))
    }

    pub fn each_with_field<'a>(
        &'a self,
        field_name: &'a str,
    ) -> impl Iterator<Item = &'a NodeView> + 'a {
        self.each()
            .iter()
            .filter(move |child| child.field_name() == Some(field_name))
    }

    pub fn each_with_token<'a>(
        &'a self,
        token_name: &'a str,
    ) -> impl Iterator<Item = &'a NodeView> + 'a {
        self.each()
            .iter()
            .filter(move |child| child.token_name() == Some(token_name))
    }

    pub fn try_first_with_rule<'a>(&'a self, rule_name: &'a str) -> Option<&'a NodeView> {
        self.each_with_rule(rule_name).next()
    }

    pub fn try_first_with_field<'a>(&'a self, field_name: &'a str) -> Option<&'a NodeView> {
        self.each_with_field(field_name).next()
    }

    pub fn try_first_with_token<'a>(&'a self, token_name: &'a str) -> Option<&'a NodeView> {
        self.each_with_token(token_name).next()
    }

    pub fn first_with_rule<'a>(&'a self, rule_name: &'a str) -> &'a NodeView {
        self.try_first_with_rule(rule_name).unwrap_or_else(|| {
            panic!(
                "NodeView: missing rule child {} for node {}",
                rule_name,
                self.name().unwrap_or("?")
            )
        })
    }

    pub fn first_with_field<'a>(&'a self, field_name: &'a str) -> &'a NodeView {
        self.try_first_with_field(field_name).unwrap_or_else(|| {
            panic!(
                "NodeView: missing field child {} for node {}",
                field_name,
                self.name().unwrap_or("?")
            )
        })
    }

    pub fn first_with_token<'a>(&'a self, token_name: &'a str) -> &'a NodeView {
        self.try_first_with_token(token_name).unwrap_or_else(|| {
            panic!(
                "NodeView: missing token child {} for node {}",
                token_name,
                self.name().unwrap_or("?")
            )
        })
    }

    pub fn view<T: 'static>(&self, viewer: &Viewer) -> T {
        viewer.view_at::<T>(self.green, self.offset)
    }
}

impl Display for NodeView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // ParseTreeIR-backed views may not carry full source text; rebuild a
        // local source slice from token_texts so token labels stay visible.
        let source = if self.source.is_empty() {
            self.text()
        } else {
            self.source.to_string()
        };
        write!(
            f,
            "{}",
            format_ast(
                self.grammar,
                &RedNode::root(self.green),
                &self.alloc,
                &source,
            )
        )
    }
}

impl Index<usize> for NodeView {
    type Output = NodeView;

    fn index(&self, index: usize) -> &Self::Output {
        self.nth(index)
    }
}

/// Heterogeneous typed parse-tree viewer.
#[derive(Clone)]
pub struct Viewer {
    grammar: &'static Grammar,
    alloc: TreeAllocRef,
    source: Arc<str>,
    token_texts: Arc<FxHashMap<usize, String>>,
    rule_visitors: FxHashMap<(TypeId, usize), ErasedHandler>,
    field_visitors: FxHashMap<(TypeId, &'static str), ErasedHandler>,
    token_visitors: FxHashMap<(TypeId, &'static str), ErasedHandler>,
    error_visitors: FxHashMap<TypeId, ErasedHandler>,
}

impl Viewer {
    pub(crate) fn new(
        grammar: &'static Grammar,
        alloc: TreeAllocRef,
        source: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            grammar,
            alloc,
            source: source.into(),
            token_texts: Arc::new(FxHashMap::default()),
            rule_visitors: FxHashMap::default(),
            field_visitors: FxHashMap::default(),
            token_visitors: FxHashMap::default(),
            error_visitors: FxHashMap::default(),
        }
    }

    pub fn with_token_texts(mut self, token_texts: FxHashMap<usize, String>) -> Self {
        self.token_texts = Arc::new(token_texts);
        self
    }

    pub fn node(&self, green: usize, offset: usize) -> NodeView {
        NodeView::init(
            self.grammar,
            self.alloc.clone(),
            self.source.clone(),
            self.token_texts.clone(),
            green,
            offset,
        )
    }

    pub fn on_rule<T: 'static, F>(mut self, rule_name: &str, visitor: F) -> Self
    where
        F: Fn(&Viewer, &NodeView) -> ViewAction<T> + 'static,
    {
        if let Some(rule_ix) = self
            .grammar
            .table
            .rules
            .iter()
            .position(|rule| rule.name == rule_name)
        {
            self.rule_visitors.insert(
                (TypeId::of::<T>(), rule_ix),
                Arc::new(move |viewer, node| match visitor(viewer, node) {
                    ViewAction::Relay => ErasedViewAction::Relay,
                    ViewAction::Exact(value) => ErasedViewAction::Exact(Box::new(value)),
                }),
            );
        }
        self
    }

    pub fn on_field<T: 'static, F>(mut self, field_name: &'static str, visitor: F) -> Self
    where
        F: Fn(&Viewer, &NodeView) -> ViewAction<T> + 'static,
    {
        self.field_visitors.insert(
            (TypeId::of::<T>(), field_name),
            Arc::new(move |viewer, node| match visitor(viewer, node) {
                ViewAction::Relay => ErasedViewAction::Relay,
                ViewAction::Exact(value) => ErasedViewAction::Exact(Box::new(value)),
            }),
        );
        self
    }

    pub fn on_token<T: 'static, F>(mut self, token_name: &'static str, visitor: F) -> Self
    where
        F: Fn(&Viewer, &NodeView) -> ViewAction<T> + 'static,
    {
        self.token_visitors.insert(
            (TypeId::of::<T>(), token_name),
            Arc::new(move |viewer, node| match visitor(viewer, node) {
                ViewAction::Relay => ErasedViewAction::Relay,
                ViewAction::Exact(value) => ErasedViewAction::Exact(Box::new(value)),
            }),
        );
        self
    }

    pub fn on_error<T: 'static, F>(mut self, visitor: F) -> Self
    where
        F: Fn(&Viewer, &NodeView) -> ViewAction<T> + 'static,
    {
        self.error_visitors.insert(
            TypeId::of::<T>(),
            Arc::new(move |viewer, node| match visitor(viewer, node) {
                ViewAction::Relay => ErasedViewAction::Relay,
                ViewAction::Exact(value) => ErasedViewAction::Exact(Box::new(value)),
            }),
        );
        self
    }

    pub fn view<T: 'static>(&self, green: usize, offset: usize) -> T {
        self.view_at::<T>(green, offset)
    }

    fn view_at<T: 'static>(&self, green: usize, offset: usize) -> T {
        let obj = self.node(green, offset);
        let green_node = self.alloc.node(green);
        let ty = TypeId::of::<T>();

        if let Tag::Rule { rule_ix, .. } = &green_node.tag {
            if let Some(handler) = self.rule_visitors.get(&(ty, *rule_ix)) {
                match handler(self, &obj) {
                    ErasedViewAction::Relay => {}
                    ErasedViewAction::Exact(value) => {
                        return *value
                            .downcast::<T>()
                            .expect("typed rule visitor returned mismatched value type");
                    }
                }
            }
        }

        if let Tag::Field { name, .. } = &green_node.tag {
            if let Some(handler) = self.field_visitors.get(&(ty, name)) {
                match handler(self, &obj) {
                    ErasedViewAction::Relay => {}
                    ErasedViewAction::Exact(value) => {
                        return *value
                            .downcast::<T>()
                            .expect("typed field visitor returned mismatched value type");
                    }
                }
            }
        }

        if let Some(token_name) = obj.token_name() {
            if let Some(handler) = self.token_visitors.get(&(ty, token_name)) {
                match handler(self, &obj) {
                    ErasedViewAction::Relay => {}
                    ErasedViewAction::Exact(value) => {
                        return *value
                            .downcast::<T>()
                            .expect("typed token visitor returned mismatched value type");
                    }
                }
            }
        }

        if obj.error().is_some() {
            if let Some(handler) = self.error_visitors.get(&ty) {
                match handler(self, &obj) {
                    ErasedViewAction::Relay => {}
                    ErasedViewAction::Exact(value) => {
                        return *value
                            .downcast::<T>()
                            .expect("typed error visitor returned mismatched value type");
                    }
                }
            }
        }

        let children = obj.each();

        if children.len() == 1 {
            return children[0].view::<T>(self);
        }

        if children.is_empty() {
            if ty == TypeId::of::<String>() {
                let value: Box<dyn Any> = Box::new(obj.text());
                return *value
                    .downcast::<T>()
                    .expect("default leaf String conversion returned mismatched value type");
            }

            if ty == TypeId::of::<usize>() {
                let value: Box<dyn Any> = Box::new(obj.text().trim().parse::<usize>().unwrap());
                return *value
                    .downcast::<T>()
                    .expect("default leaf usize conversion returned mismatched value type");
            }

            panic!(
                "Viewer: no handler for leaf node {} as {}",
                obj.name().unwrap_or("?"),
                std::any::type_name::<T>()
            );
        }

        panic!(
            "Viewer: no handler for node {} with {} children as {}",
            obj.name().unwrap_or("?"),
            children.len(),
            std::any::type_name::<T>()
        )
    }
}
