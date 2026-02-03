use crate::{
    grammar::{Grammar, ir::State},
    parsec::{
        tree::{RedNode, Tag, TreeAlloc},
        words::Matcher,
    },
};
use std::collections::HashSet;

pub struct Parser<'a> {
    pub(crate) text: &'a str,
    pub(crate) grammar: &'a Grammar,
    pub(crate) alloc: TreeAlloc,
    in_flight: HashSet<ParseKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ParseKey {
    state_id: usize,
    pos: usize,
}

struct ParseResult {
    pos: usize,
    ok: bool,
    cost: usize,
}

impl<'a> Parser<'a> {
    pub fn new(text: &'a str, grammar: &'a Grammar) -> Self {
        let alloc = TreeAlloc::new();
        Self {
            text,
            grammar,
            alloc,
            in_flight: HashSet::new(),
        }
    }

    pub fn parse_text(&mut self) -> RedNode {
        let start_state = self.grammar.analysis.start_state;

        let mut root = RedNode::new_root(&self.alloc, self.text);
        let root_green = root.green;

        self.parse(root_green, start_state, 0, None);

        if let Some(&child) = self.alloc.get_node(root_green).children.first() {
            root.green = child;
        }

        root
    }

    fn parse(
        &mut self,
        node_id: usize,
        state_id: usize,
        pos: usize,
        last_rule_ix: Option<usize>,
    ) -> ParseResult {
        let key = ParseKey { state_id, pos };
        if self.in_flight.contains(&key) {
            return ParseResult {
                pos,
                ok: false,
                cost: 0,
            };
        }

        self.in_flight.insert(key);
        let res = self.parse_inner(node_id, state_id, pos, last_rule_ix);
        self.in_flight.remove(&key);
        res
    }

    fn parse_child(
        &mut self,
        work_node: usize,
        child_state: usize,
        pos: usize,
        rule_ix: usize,
    ) -> ParseResult {
        self.parse(work_node, child_state, pos, Some(rule_ix))
    }

    fn make_work_node(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        should_create_node: bool,
    ) -> usize {
        if should_create_node {
            let tag = Tag::new_rule(rule_ix);
            self.alloc.alloc(tag, vec![], 0)
        } else {
            node_id
        }
    }

    fn parse_inner(
        &mut self,
        node_id: usize,
        state_id: usize,
        pos: usize,
        last_rule_ix: Option<usize>,
    ) -> ParseResult {
        let _ = last_rule_ix;
        let state = self.grammar.analysis.states[state_id].clone();
        let current_rule_ix = state.ref_ix();

        // Only create a new AST node when the rule changes and the rule is named
        let is_trivial_rule = self.grammar.name(current_rule_ix).starts_with('@');
        let should_create_node = !is_trivial_rule;

        match state {
            State::Tok(rule_ix, matcher) => {
                self.parse_token(node_id, rule_ix, matcher.as_ref(), pos, should_create_node)
            }

            State::Seq(rule_ix, children) => {
                self.parse_sequence(node_id, rule_ix, children, pos, should_create_node)
            }

            State::Alt(rule_ix, children, has_epsilon) => self.parse_alternative(
                node_id,
                rule_ix,
                children,
                has_epsilon,
                pos,
                should_create_node,
            ),

            State::Field(rule_ix, name, child) => {
                self.parse_field(node_id, rule_ix, name, child, pos)
            }

            State::LeftRec(rule_ix, base, tail, tail_fields) => self.parse_left_rec(
                node_id,
                rule_ix,
                base,
                tail,
                tail_fields,
                pos,
                should_create_node,
            ),
        }
    }

    fn parse_token(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        matcher: &dyn Matcher,
        pos: usize,
        _should_create_node: bool,
    ) -> ParseResult {
        let mut current_pos = pos;
        let matched = if current_pos <= self.text.len() {
            matcher.matches(self.text, &mut current_pos)
        } else {
            None
        };

        if let Some(width) = matched {
            if width == 0 {
                return ParseResult {
                    pos: current_pos,
                    ok: true,
                    cost: 0,
                };
            }
            let tag = Tag::new_token(rule_ix);
            let token_id = self.alloc.alloc_token(tag, width);
            self.alloc.get_node_mut(node_id).children.push(token_id);
            ParseResult {
                pos: current_pos,
                ok: true,
                cost: 0,
            }
        } else {
            ParseResult {
                pos,
                ok: false,
                cost: 0,
            }
        }
    }

    /// Parse n-ary sequence (all children in sequence)
    fn parse_sequence(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        children: Vec<usize>,
        pos: usize,
        should_create_node: bool,
    ) -> ParseResult {
        let work_node = self.make_work_node(node_id, rule_ix, should_create_node);
        let saved_children_len = self.alloc.get_node(work_node).children.len();

        let mut current_pos = pos;

        for &child_state in children.iter() {
            let res = self.parse_child(work_node, child_state, current_pos, rule_ix);

            if res.ok {
                current_pos = res.pos;
            } else {
                // Rollback on failure
                self.alloc
                    .get_node_mut(work_node)
                    .children
                    .truncate(saved_children_len);
                return ParseResult {
                    pos,
                    ok: false,
                    cost: 0,
                };
            }
        }

        if should_create_node {
            self.finalize_node(work_node, node_id);
        }

        ParseResult {
            pos: current_pos,
            ok: true,
            cost: 0,
        }
    }

    /// Parse n-ary alternative (try each child until one succeeds)
    fn parse_alternative(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        children: Vec<usize>,
        has_epsilon: bool,
        pos: usize,
        should_create_node: bool,
    ) -> ParseResult {
        let work_node = self.make_work_node(node_id, rule_ix, should_create_node);

        // Try each child until one succeeds
        for child_state in children.iter().copied() {
            let saved_len = self.alloc.get_node(work_node).children.len();
            let res = self.parse_child(work_node, child_state, pos, rule_ix);

            if res.ok && res.pos > pos {
                if should_create_node {
                    self.finalize_node(work_node, node_id);
                }
                return res;
            }

            // Backtrack
            self.alloc
                .get_node_mut(work_node)
                .children
                .truncate(saved_len);
        }

        // Try epsilon if available
        if has_epsilon {
            let rule_name = self.grammar.name(rule_ix);
            let is_repeat_rule = rule_name.starts_with("@rep")
                || rule_name.starts_with("@sep")
                || rule_name.starts_with("@sep_tail");
            if is_repeat_rule && pos >= self.text.len() {
                return ParseResult {
                    pos,
                    ok: false,
                    cost: 0,
                };
            }
            if should_create_node {
                self.finalize_node(work_node, node_id);
            }
            return ParseResult {
                pos,
                ok: true,
                cost: 0,
            };
        }

        // All alternatives failed
        ParseResult {
            pos,
            ok: false,
            cost: 0,
        }
    }

    fn parse_left_rec(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        base_states: Vec<usize>,
        tail_states: Vec<usize>,
        tail_fields: Vec<Option<&'static str>>,
        pos: usize,
        should_create_node: bool,
    ) -> ParseResult {
        let work_node = self.make_work_node(node_id, rule_ix, should_create_node);

        let mut current_pos = pos;
        let mut base_ok = false;

        for base_state in base_states {
            let saved_children_len = self.alloc.get_node(work_node).children.len();
            let res = self.parse_child(work_node, base_state, pos, rule_ix);
            if res.ok {
                current_pos = res.pos;
                base_ok = true;
                break;
            }
            self.alloc
                .get_node_mut(work_node)
                .children
                .truncate(saved_children_len);
        }

        if !base_ok {
            // If no base matched, try with epsilon base (start at current position)
            // This allows left-recursive patterns like A -> A "a" | epsilon to work
            // by trying to match tail states directly
            current_pos = pos;
        }

        if should_create_node {
            self.update_width(work_node);
        }

        let mut current_node = work_node;

        loop {
            let mut progressed = false;

            for (tail_state, tail_field) in tail_states.iter().zip(tail_fields.iter()) {
                if should_create_node {
                    let base_child = if let Some(name) = tail_field {
                        let field_node =
                            self.alloc
                                .alloc(Tag::new_field(rule_ix, *name), vec![current_node], 0);
                        self.update_width(field_node);
                        field_node
                    } else {
                        current_node
                    };
                    let tag = Tag::new_rule(rule_ix);
                    let new_node = self.alloc.alloc(tag, vec![base_child], 0);
                    let res = self.parse_child(new_node, *tail_state, current_pos, rule_ix);
                    if res.ok && res.pos > current_pos {
                        self.update_width(new_node);
                        current_node = new_node;
                        current_pos = res.pos;
                        progressed = true;
                        break;
                    }
                } else {
                    let saved_children_len = self.alloc.get_node(current_node).children.len();
                    let res = self.parse_child(current_node, *tail_state, current_pos, rule_ix);
                    if res.ok && res.pos > current_pos {
                        current_pos = res.pos;
                        progressed = true;
                        break;
                    }
                    self.alloc
                        .get_node_mut(current_node)
                        .children
                        .truncate(saved_children_len);
                }
            }

            if !progressed {
                break;
            }
        }

        if should_create_node {
            self.finalize_node(current_node, node_id);
        }

        ParseResult {
            pos: current_pos,
            ok: true,
            cost: 0,
        }
    }

    fn parse_field(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        name: &'static str,
        child_state: usize,
        pos: usize,
    ) -> ParseResult {
        let field_node = self.alloc.alloc(Tag::new_field(rule_ix, name), vec![], 0);
        let res = self.parse(field_node, child_state, pos, Some(rule_ix));

        if !res.ok {
            return ParseResult {
                pos,
                ok: false,
                cost: 0,
            };
        }

        self.finalize_node(field_node, node_id);

        ParseResult {
            pos: res.pos,
            ok: res.ok,
            cost: res.cost,
        }
    }

    fn finalize_node(&mut self, work_node: usize, parent_node: usize) {
        // Calculate total width from children
        self.update_width(work_node);
        self.alloc
            .get_node_mut(parent_node)
            .children
            .push(work_node);
    }

    fn update_width(&mut self, node_id: usize) {
        let total_width: usize = self
            .alloc
            .get_node(node_id)
            .children
            .iter()
            .map(|&child_id| self.alloc.get_node(child_id).width)
            .sum();

        self.alloc.get_node_mut(node_id).width = total_width;
    }
}
