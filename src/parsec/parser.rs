use crate::{
    grammar::{Grammar, ir::State},
    parsec::{
        tree::{ParsecError, RedNode, Tag, TreeAlloc},
        words::Matcher,
    },
};

pub struct Parser<'a> {
    text: &'a str,
    grammar: &'a Grammar,
    pub(crate) alloc: TreeAlloc,
}

struct ParseResult {
    pos: usize,
    ok: bool,
}

impl<'a> Parser<'a> {
    pub fn new(text: &'a str, grammar: &'a Grammar) -> Self {
        let alloc = TreeAlloc::new();
        Self {
            text,
            grammar,
            alloc,
        }
    }

    pub fn parse_text(&mut self) -> RedNode {
        let mut root = RedNode::new_root(&self.alloc, self.text);
        let root_green = root.green;

        let start_state = self.grammar.analysis.start_state;
        self.parse(root_green, start_state, 0, None, true);

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
        emit_errors: bool,
    ) -> ParseResult {
        let state = self.grammar.analysis.states[state_id].clone();
        let current_rule_ix = state.ref_ix();

        // Only create a new AST node when the rule changes
        let should_create_node = last_rule_ix.map_or(true, |last| last != current_rule_ix);

        match state {
            State::Tok(rule_ix, matcher) => self.parse_token(
                node_id,
                rule_ix,
                matcher.as_ref(),
                pos,
                should_create_node,
                emit_errors,
            ),

            State::Seq(rule_ix, children) => {
                self.parse_sequence(
                    node_id,
                    rule_ix,
                    children,
                    pos,
                    should_create_node,
                    emit_errors,
                )
            }

            State::Alt(rule_ix, children) => {
                self.parse_alternative(
                    node_id,
                    rule_ix,
                    children,
                    pos,
                    should_create_node,
                    emit_errors,
                )
            }

            State::LeftRec(rule_ix, base, tail) => {
                self.parse_left_rec(
                    node_id,
                    rule_ix,
                    base,
                    tail,
                    pos,
                    should_create_node,
                    emit_errors,
                )
            }
        }
    }

    fn parse_token(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        matcher: &dyn Matcher,
        pos: usize,
        _should_create_node: bool,
        emit_errors: bool,
    ) -> ParseResult {
        let mut current_pos = pos;
        let matched = current_pos < self.text.len() && matcher.matches(self.text, &mut current_pos);

        if matched {
            let width = current_pos - pos;
            let tag = Tag::new_token(rule_ix, matcher.display());
            let token_id = self.alloc.alloc_token(tag, width);
            self.alloc.get_node_mut(node_id).children.push(token_id);
            ParseResult {
                pos: current_pos,
                ok: true,
            }
        } else if emit_errors {
            // Error recovery: skip one character and continue
            let error_tag = Tag::new_error(ParsecError::UnexpectedToken);
            let skip_width = if pos < self.text.len() { 1 } else { 0 };
            let error_id = self.alloc.alloc_token(error_tag, skip_width);
            self.alloc.get_node_mut(node_id).children.push(error_id);
            ParseResult {
                pos: pos + skip_width,
                ok: true,
            }
        } else {
            ParseResult { pos, ok: false }
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
        emit_errors: bool,
    ) -> ParseResult {
        let work_node = if should_create_node {
            let tag = Tag::new_rule(rule_ix);
            self.alloc.alloc(tag, vec![], 0)
        } else {
            node_id
        };

        let saved_children_len = self.alloc.get_node(work_node).children.len();

        // Parse all children in sequence
        let mut current_pos = pos;
        for child_state in children {
            let res = self.parse(work_node, child_state, current_pos, Some(rule_ix), emit_errors);
            if !emit_errors && !res.ok {
                self.alloc
                    .get_node_mut(work_node)
                    .children
                    .truncate(saved_children_len);
                return ParseResult { pos, ok: false };
            }
            current_pos = res.pos;
        }

        if should_create_node {
            self.finalize_node(work_node, node_id);
        }

        ParseResult {
            pos: current_pos,
            ok: true,
        }
    }

    /// Parse n-ary alternative (try each child until one succeeds)
    fn parse_alternative(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        children: Vec<usize>,
        pos: usize,
        should_create_node: bool,
        emit_errors: bool,
    ) -> ParseResult {
        let work_node = if should_create_node {
            let tag = Tag::new_rule(rule_ix);
            self.alloc.alloc(tag, vec![], 0)
        } else {
            node_id
        };

        // Try each alternative child until one succeeds
        for child_state in children {
            let saved_children_len = self.alloc.get_node(work_node).children.len();
            let res = self.parse(work_node, child_state, pos, Some(rule_ix), false);

            if res.ok {
                if should_create_node {
                    self.finalize_node(work_node, node_id);
                }
                return ParseResult { pos: res.pos, ok: true };
            }

            // Reset children and try next alternative
            self.alloc
                .get_node_mut(work_node)
                .children
                .truncate(saved_children_len);
        }

        if emit_errors {
            // No alternatives matched, record error
            let error_tag = Tag::new_error(ParsecError::UnexpectedToken);
            let error_id = self.alloc.alloc_token(error_tag, 0);
            self.alloc.get_node_mut(work_node).children.push(error_id);
        }

        if should_create_node {
            self.finalize_node(work_node, node_id);
        }

        ParseResult {
            pos,
            ok: emit_errors,
        }
    }

    fn parse_left_rec(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        base_states: Vec<usize>,
        tail_states: Vec<usize>,
        pos: usize,
        should_create_node: bool,
        emit_errors: bool,
    ) -> ParseResult {
        let work_node = if should_create_node {
            let tag = Tag::new_rule(rule_ix);
            self.alloc.alloc(tag, vec![], 0)
        } else {
            node_id
        };

        let mut current_pos = pos;
        let mut base_ok = false;

        for base_state in base_states {
            let saved_children_len = self.alloc.get_node(work_node).children.len();
            let res = self.parse(work_node, base_state, pos, Some(rule_ix), false);
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
            if emit_errors {
                let error_tag = Tag::new_error(ParsecError::UnexpectedToken);
                let error_id = self.alloc.alloc_token(error_tag, 0);
                self.alloc.get_node_mut(work_node).children.push(error_id);

                if should_create_node {
                    self.finalize_node(work_node, node_id);
                }

                return ParseResult { pos, ok: true };
            }

            if should_create_node {
                self.finalize_node(work_node, node_id);
            }
            return ParseResult { pos, ok: false };
        }

        if should_create_node {
            self.update_width(work_node);
        }

        let mut current_node = work_node;

        loop {
            let mut progressed = false;

            for tail_state in &tail_states {
                if should_create_node {
                    let tag = Tag::new_rule(rule_ix);
                    let new_node = self.alloc.alloc(tag, vec![current_node], 0);
                    let res =
                        self.parse(new_node, *tail_state, current_pos, Some(rule_ix), false);
                    if res.ok && res.pos > current_pos {
                        self.update_width(new_node);
                        current_node = new_node;
                        current_pos = res.pos;
                        progressed = true;
                        break;
                    }
                } else {
                    let saved_children_len = self.alloc.get_node(current_node).children.len();
                    let res = self.parse(
                        current_node,
                        *tail_state,
                        current_pos,
                        Some(rule_ix),
                        false,
                    );
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
