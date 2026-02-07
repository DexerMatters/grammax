use crate::grammar::Grammar;
use crate::grammar::analysis::{Action, EOF_TOKEN};
use crate::grammar::recovery::{ErrorRecoveryStrategy, RecoverySpecs};
use crate::parsec::msg::{ParserMessage, ParserMessages};
use crate::parsec::tree::{GreenId, ParsecError, RedNode, Tag, TreeAllocRef, TreeAllocRefExt};
use crate::utils::Span;

const UNKNOWN_TOKEN: usize = usize::MAX - 1;

#[derive(Debug, Clone, Default)]
pub struct ParserConfig {
    // Configuration options
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
            let (term_idx, token_len, token_node) = self.lex();

            let action = {
                let current_state = &self.grammar.analysis.states[current_state_idx];
                current_state.actions.get(&term_idx).cloned()
            };

            if let Some(action) = action {
                match action {
                    Action::Shift(next_state) => {
                        self.consume(token_len);
                        state_stack.push(next_state);
                        node_stack.push(token_node);
                    }
                    Action::Reduce(prod_idx) => {
                        let prod = &self.grammar.table.productions[prod_idx];
                        let lhs = prod.lhs;
                        let rhs_len = prod.rhs.len();

                        let mut children = Vec::with_capacity(rhs_len);
                        for _ in 0..rhs_len {
                            state_stack.pop();
                            children.push(node_stack.pop().unwrap());
                        }
                        children.reverse();

                        let width: usize = children
                            .iter()
                            .map(|id| self.alloc.get_node(*id).width)
                            .sum();
                        let new_node = self.alloc.alloc(Tag::new_rule(lhs), children, width);
                        node_stack.push(new_node);

                        let top_state_idx = *state_stack.last().unwrap();
                        let top_state = &self.grammar.analysis.states[top_state_idx];

                        if let Some(goto_state) = top_state.goto.get(&lhs) {
                            state_stack.push(*goto_state);
                        } else {
                            self.messages.push(ParserMessage::new_unexpected(
                                Span::new(self.pos, self.pos),
                                vec!["Valid Goto".to_string()],
                            ));
                            break;
                        }
                    }
                    Action::Accept => {
                        // println!("Accepted!");
                        break;
                    }
                    Action::Error => {
                        // println!("Error at pos {} with token {}", self.pos, term_idx);
                        self.messages.push(ParserMessage::new_unexpected(
                            Span::new(self.pos, self.pos),
                            vec!["Valid Token".to_string()],
                        ));
                        break;
                    }
                }
            } else {
                // println!("No action for token {} at state {}", term_idx, current_state_ix);
                self.messages.push(ParserMessage::new_unexpected(
                    Span::new(self.pos, self.pos),
                    vec!["Valid Action".to_string()],
                ));
                break;
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

    // Helper for Reparser
    pub fn parse_rule(&mut self, _rule_ix: usize, _pos: usize) -> Option<GreenId> {
        None
    }

    fn lex(&self) -> (usize, usize, GreenId) {
        let rest = &self.text[self.pos..];
        if rest.is_empty() {
            return (
                EOF_TOKEN,
                0,
                self.alloc.alloc_token(Tag::new_token(EOF_TOKEN), 0),
            );
        }

        let mut best_match: Option<(usize, usize)> = None;

        for (idx, matcher) in self.grammar.table.terminals.iter().enumerate() {
            let mut pos = 0;
            if let Some(len) = matcher.matches(rest, &mut pos) {
                if best_match.iter().all(|&(_, best_len)| len > best_len) {
                    best_match = Some((idx, len));
                }
            }
        }

        if let Some((idx, len)) = best_match {
            let node = self.alloc.alloc_token(Tag::new_token(idx), len);
            (idx, len, node)
        } else {
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
}
