use crate::grammar::Grammar;
use crate::grammar::analysis::{Action, EOF_TOKEN};
use crate::grammar::recovery::{ErrorRecoveryStrategy, RecoverySpecs};
use crate::parsec::msg::{ParserMessage, ParserMessages};
use crate::parsec::recovery::{RecoveryConfig, RepairOp, recover};
use crate::parsec::tree::{GreenId, ParsecError, RedNode, Tag, TreeAllocRef, TreeAllocRefExt};
use crate::utils::Span;

const UNKNOWN_TOKEN: usize = usize::MAX - 1;

#[derive(Debug, Clone)]
pub struct ParserConfig {
    pub simple_ast: bool,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self { simple_ast: true }
    }
}

#[derive(Default)]
pub struct ParserListener {
    // Callbacks
}

pub struct Result {
    pub root: RedNode,
    pub messages: ParserMessages,
}

pub struct Parser {
    pub grammar: Grammar,
    pub alloc: TreeAllocRef,
    pub messages: ParserMessages,
    pub newly_computed_nodes: Vec<Span>,
    pub newly_computed_tokens: Vec<Span>,

    config: ParserConfig,
    listener: Option<ParserListener>,

    text: String,
    pos: usize,

    // For incremental/reparsing (stubs)
    inc_insert_pos: Option<usize>,
}

impl Parser {
    pub fn new(grammar: Grammar) -> Self {
        Self {
            grammar,
            alloc: TreeAllocRef::create(),
            messages: vec![],
            newly_computed_nodes: vec![],
            newly_computed_tokens: vec![],
            config: ParserConfig::default(),
            listener: None,
            text: String::new(),
            pos: 0,
            inc_insert_pos: None,
        }
    }

    pub fn with_config(mut self, config: ParserConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_listener(mut self, listener: ParserListener) -> Self {
        self.listener = Some(listener);
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn recovery_specs(&self) -> Option<&RecoverySpecs> {
        None // TODO
    }

    pub fn recovery_strategy(&self) -> Option<&ErrorRecoveryStrategy> {
        None // TODO
    }

    pub fn newly_computed_nodes(&self) -> Vec<Span> {
        self.newly_computed_nodes.clone()
    }

    pub fn newly_computed_tokens(&self) -> Vec<Span> {
        self.newly_computed_tokens.clone()
    }

    pub fn set_incremental_insert_pos(&mut self, pos: Option<usize>) {
        self.inc_insert_pos = pos;
    }

    pub fn parse_text(&mut self, text: &str) -> Result {
        self.text = text.to_string();
        self.pos = 0;
        self.messages.clear();

        // LR Parsing Loop
        let start_state = self.grammar.analysis.start_state;
        let mut state_stack = vec![start_state];
        let mut node_stack: Vec<GreenId> = vec![];

        loop {
            let current_state_idx = *state_stack.last().unwrap();
            let current_state = &self.grammar.analysis.states[current_state_idx];
            let (term_idx, token_len, token_node) = self.lex(Some(&current_state.actions));

            let action = current_state.actions.get(&term_idx).cloned();

            if let Some(action) = action {
                match action {
                    Action::Shift(next_state) => {
                        self.consume(token_len);
                        state_stack.push(next_state);
                        node_stack.push(token_node);
                    }
                    Action::Reduce(prod_idx) => {
                        let (lhs, rhs_len, field_positions) = {
                            let prod = &self.grammar.table.productions[prod_idx];
                            (prod.lhs, prod.rhs.len(), prod.field_positions.clone())
                        };

                        let mut children = Vec::with_capacity(rhs_len);
                        for _ in 0..rhs_len {
                            state_stack.pop();
                            children.push(node_stack.pop().unwrap());
                        }
                        children.reverse();

                        // Apply fields
                        for (pos, name) in field_positions {
                            if pos < children.len() {
                                let child_id = children[pos];
                                let width = {
                                    let child_node = self.alloc.get_node(child_id);
                                    child_node.width
                                };
                                // Wrap in Field node
                                let field_id = self.alloc.alloc(
                                    Tag::Field { rule_ix: lhs, name },
                                    vec![child_id],
                                    width,
                                );
                                children[pos] = field_id;
                            }
                        }

                        let builder = crate::parsec::builder::TreeBuilder::new(
                            &self.grammar,
                            &self.alloc,
                            &self.config,
                        );
                        let new_node = builder.build_node(lhs, children);

                        node_stack.push(new_node);

                        let top_state_idx = *state_stack.last().unwrap();
                        let top_state = &self.grammar.analysis.states[top_state_idx];

                        if let Some(goto_state) = top_state.goto.get(&lhs) {
                            state_stack.push(*goto_state);
                        } else {
                            self.messages.push(ParserMessage::new_unexpected(
                                Span::new(self.pos, self.pos),
                                Vec::new(),
                            ));
                            break;
                        }
                    }
                    Action::Accept => {
                        // println!("Accepted!");
                        break;
                    }
                }
            } else {
                let recovery_config = RecoveryConfig::default();
                let maybe_ops = recover(
                    &self.grammar.analysis,
                    &self.grammar.table.productions,
                    &self.grammar.table.terminals,
                    &self.text,
                    self.pos,
                    &state_stack,
                    &recovery_config,
                );

                if let Some(ops) = maybe_ops {
                    if ops.is_empty() {
                        if !self.panic_consume_until_action(&state_stack) {
                            break;
                        }
                        continue;
                    }
                    let old_pos = self.pos;
                    let old_state_len = state_stack.len();
                    let old_node_len = node_stack.len();
                    if !self.apply_repair_ops(&ops, &mut state_stack, &mut node_stack) {
                        if !self.panic_consume_until_action(&state_stack) {
                            break;
                        }
                        continue;
                    }
                    if self.pos == old_pos
                        && state_stack.len() == old_state_len
                        && node_stack.len() == old_node_len
                    {
                        if !self.panic_consume_until_action(&state_stack) {
                            break;
                        }
                        continue;
                    }
                } else {
                    if !self.panic_consume_until_action(&state_stack) {
                        break;
                    }
                    continue;
                }
            }
        }

        let root_green = node_stack
            .pop()
            .unwrap_or_else(|| self.alloc.new_placeholder(0));

        Result {
            root: RedNode::root(&self.alloc, root_green),
            messages: self.messages.clone(),
        }
    }

    fn panic_consume_until_action(&mut self, state_stack: &[usize]) -> bool {
        loop {
            let (term, len, _node) = self.lex(None);
            if term == EOF_TOKEN || len == 0 {
                return false;
            }

            let current_state_idx = *state_stack.last().unwrap();
            let current_state = &self.grammar.analysis.states[current_state_idx];

            if current_state.actions.contains_key(&term) {
                return true;
            }

            self.messages.push(ParserMessage::new_unexpected(
                Span::new(self.pos, self.pos + len),
                self.expected_ids(current_state_idx),
            ));
            self.consume(len);
        }
    }

    // Helper for Reparser
    pub fn parse_rule(&mut self, _rule_ix: usize, _pos: usize) -> Option<GreenId> {
        None
    }

    fn lex(
        &self,
        expected: Option<&rustc_hash::FxHashMap<usize, crate::grammar::analysis::Action>>,
    ) -> (usize, usize, GreenId) {
        let rest = &self.text[self.pos..];
        if rest.is_empty() {
            return (
                EOF_TOKEN,
                0,
                self.alloc.alloc_token(Tag::new_token(EOF_TOKEN), 0),
            );
        }

        let mut best_match: Option<(usize, usize)> = None;

        let mut consider = |idx: usize, matcher: &crate::parsec::words::MatcherRef| {
            let mut pos = 0;
            if let Some(len) = matcher.matches(rest, &mut pos) {
                if best_match.iter().all(|&(_, best_len)| len > best_len) {
                    best_match = Some((idx, len));
                }
            }
        };

        if let Some(expected) = expected {
            for (&idx, _) in expected.iter() {
                if idx == EOF_TOKEN {
                    continue;
                }
                if let Some(matcher) = self.grammar.table.terminals.get(idx) {
                    consider(idx, matcher);
                }
            }
        } else {
            for (idx, matcher) in self.grammar.table.terminals.iter().enumerate() {
                consider(idx, matcher);
            }
        }

        if let Some((idx, len)) = best_match {
            let node = self.alloc.alloc_token(Tag::new_token(idx), len);
            (idx, len, node)
        } else {
            if rest.trim().is_empty() {
                return (
                    EOF_TOKEN,
                    rest.len(),
                    self.alloc.alloc_token(Tag::new_token(EOF_TOKEN), rest.len()),
                );
            }

            let len = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            let node =
                self.alloc
                    .alloc(Tag::Error(vec![ParsecError::UnexpectedToken]), vec![], len);
            (UNKNOWN_TOKEN, len, node)
        }
    }

    fn consume(&mut self, len: usize) {
        self.pos += len;
    }

    fn apply_repair_ops(
        &mut self,
        ops: &[RepairOp],
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<GreenId>,
    ) -> bool {
        // Track indices of Delete ops that have been consumed by a Replace merge
        let mut skipped_deletes = std::collections::HashSet::new();

        let mut i = 0;
        while i < ops.len() {
            if skipped_deletes.contains(&i) {
                i += 1;
                continue;
            }

            let op = ops[i];

            // Check for Replace pattern (Insert(T)... + Delete)
            // We search ahead for a Delete that is separated only by Inserts
            if let RepairOp::Insert(term_ix) = op {
                let mut j = i + 1;
                let mut found_delete_idx = None;

                while j < ops.len() {
                    match ops[j] {
                        RepairOp::Insert(_) => j += 1, // Skip intervening inserts
                        RepairOp::Delete => {
                            found_delete_idx = Some(j);
                            break;
                        }
                        _ => break, // Shift breaks the chain
                    }
                }

                if let Some(delete_idx) = found_delete_idx {
                    // Found a Delete to pair with!
                    // Mark it as skipped so we don't process it again
                    skipped_deletes.insert(delete_idx);

                    // This is a replacement: Replace(term_ix, bad_token)
                    let (_bad_term, bad_len, _bad_node) = self.lex(None);
                    if bad_len == 0 {
                        // EOF or empty, can't delete/replace
                        return false;
                    }

                    // Report error
                    let expected = self.expected_ids(*state_stack.last().unwrap());
                    self.messages.push(ParserMessage::new_unexpected(
                        Span::new(self.pos, self.pos + bad_len),
                        expected,
                    ));

                    // Create Error Node (Width = bad_len)
                    let error_node = self.alloc.alloc(
                        Tag::new_error(ParsecError::UnexpectedToken),
                        vec![],
                        bad_len,
                    );

                    // Apply INSERTED terminal logic with CONSUMED length
                    if !self.apply_terminal(
                        term_ix,
                        bad_len,
                        error_node,
                        true,
                        state_stack,
                        node_stack,
                    ) {
                        return false;
                    }

                    i += 1;
                    continue;
                }
            }

            match op {
                RepairOp::Insert(term_ix) => {
                    self.messages.push(ParserMessage::new_missing(
                        Span::new(self.pos, self.pos),
                        vec![term_ix],
                    ));
                    let token_node =
                        self.alloc
                            .alloc(Tag::new_error(ParsecError::MissingToken), vec![], 0);
                    if !self.apply_terminal(term_ix, 0, token_node, false, state_stack, node_stack)
                    {
                        return false;
                    }
                }
                RepairOp::Delete => {
                    let expected = self.expected_ids(*state_stack.last().unwrap());
                    self.messages.push(ParserMessage::new_unexpected(
                        Span::new(self.pos, self.pos),
                        expected,
                    ));
                    let (_term, len, _node) = self.lex(None);
                    if len == 0 {
                        return false;
                    }
                    self.consume(len);
                }
                RepairOp::Shift => {
                    let current_state_idx = *state_stack.last().unwrap();
                    let current_state = &self.grammar.analysis.states[current_state_idx];
                    let (term, len, node) = self.lex(Some(&current_state.actions));
                    if !self.apply_terminal(term, len, node, true, state_stack, node_stack) {
                        return false;
                    }
                }
            }
            i += 1;
        }
        true
    }

    fn apply_terminal(
        &mut self,
        term: usize,
        len: usize,
        token_node: GreenId,
        consume_input: bool,
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<GreenId>,
    ) -> bool {
        loop {
            let current_state_idx = *state_stack.last().unwrap();
            let action = {
                let current_state = &self.grammar.analysis.states[current_state_idx];
                current_state.actions.get(&term).cloned()
            };

            match action {
                Some(Action::Shift(next_state)) => {
                    if consume_input {
                        self.consume(len);
                    }
                    state_stack.push(next_state);
                    node_stack.push(token_node);
                    return true;
                }
                Some(Action::Reduce(prod_idx)) => {
                    let (lhs, rhs_len, field_positions) = {
                        let prod = &self.grammar.table.productions[prod_idx];
                        (prod.lhs, prod.rhs.len(), prod.field_positions.clone())
                    };

                    let mut children = Vec::with_capacity(rhs_len);
                    for _ in 0..rhs_len {
                        state_stack.pop();
                        children.push(node_stack.pop().unwrap());
                    }
                    children.reverse();

                    for (pos, name) in field_positions {
                        if pos < children.len() {
                            let child_id = children[pos];
                            let width = {
                                let child_node = self.alloc.get_node(child_id);
                                child_node.width
                            };
                            let field_id = self.alloc.alloc(
                                Tag::Field { rule_ix: lhs, name },
                                vec![child_id],
                                width,
                            );
                            children[pos] = field_id;
                        }
                    }

                    let builder = crate::parsec::builder::TreeBuilder::new(
                        &self.grammar,
                        &self.alloc,
                        &self.config,
                    );
                    let new_node = builder.build_node(lhs, children);

                    node_stack.push(new_node);

                    let top_state_idx = *state_stack.last().unwrap();
                    let top_state = &self.grammar.analysis.states[top_state_idx];

                    if let Some(goto_state) = top_state.goto.get(&lhs) {
                        state_stack.push(*goto_state);
                    } else {
                        return false;
                    }
                }
                Some(Action::Accept) => return true,
                None => return false,
            }
        }
    }

    fn expected_ids(&self, state_idx: usize) -> Vec<usize> {
        self.grammar.analysis.states[state_idx]
            .actions
            .keys()
            .copied()
            .filter(|id| *id != EOF_TOKEN)
            .collect()
    }
}
