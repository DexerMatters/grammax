use crate::{
    grammar::{Grammar, analysis::State as AnalysisState},
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
        let root = RedNode::new_root(&self.alloc, self.text);
        let root_green = root.green;

        self.parse(root_green, 0, 0, None);

        root
    }

    fn is_left_recursive(&self, state_id: usize) -> bool {
        !self.grammar.analysis.left_transitive_closure()[(state_id, state_id)].is_nan()
    }

    fn parse(
        &mut self,
        node_id: usize,
        state_id: usize,
        pos: usize,
        last_rule_ix: Option<usize>,
    ) -> usize {
        let state = self.grammar.analysis.states[state_id].clone();
        let current_rule_ix = state.ref_ix();

        // Only create a new AST node when the rule changes
        let should_create_node = last_rule_ix.map_or(true, |last| last != current_rule_ix);

        match state {
            AnalysisState::Tok(rule_ix, matcher) => {
                self.parse_token(node_id, rule_ix, matcher.as_ref(), pos, should_create_node)
            }

            AnalysisState::Seq(rule_ix, left_state, right_state) => self.parse_sequence(
                node_id,
                rule_ix,
                left_state,
                right_state,
                pos,
                should_create_node,
            ),

            AnalysisState::Alt(rule_ix, left_state, right_state) => self.parse_alternative(
                state_id,
                node_id,
                rule_ix,
                left_state,
                right_state,
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
        should_create_node: bool,
    ) -> usize {
        let mut current_pos = pos;
        let matched = current_pos < self.text.len() && matcher.matches(self.text, &mut current_pos);

        if matched {
            let width = current_pos - pos;
            if should_create_node {
                let tag = Tag::new_rule(rule_ix);
                let token_id = self.alloc.alloc_token(tag, width);
                self.alloc.get_node_mut(node_id).children.push(token_id);
            }
            current_pos
        } else {
            // Error recovery: skip one character and continue
            let error_tag = Tag::new_error(ParsecError::UnexpectedToken);
            let skip_width = if pos < self.text.len() { 1 } else { 0 };
            let error_id = self.alloc.alloc_token(error_tag, skip_width);
            self.alloc.get_node_mut(node_id).children.push(error_id);
            pos + skip_width
        }
    }

    fn parse_sequence(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        left_state: usize,
        right_state: usize,
        pos: usize,
        should_create_node: bool,
    ) -> usize {
        let work_node = if should_create_node {
            let tag = Tag::new_rule(rule_ix);
            self.alloc.alloc(tag, vec![], 0)
        } else {
            node_id
        };

        // Parse left child - continue even on error
        let mut current_pos = self.parse(work_node, left_state, pos, Some(rule_ix));

        // Parse right child - continue even on error
        current_pos = self.parse(work_node, right_state, current_pos, Some(rule_ix));

        if should_create_node {
            self.finalize_node(work_node, node_id);
        }

        current_pos
    }

    fn parse_alternative(
        &mut self,
        state_id: usize,
        node_id: usize,
        rule_ix: usize,
        left_state: usize,
        right_state: usize,
        pos: usize,
        should_create_node: bool,
    ) -> usize {
        let work_node = if should_create_node {
            let tag = Tag::new_rule(rule_ix);
            self.alloc.alloc(tag, vec![], 0)
        } else {
            node_id
        };

        let parent_left_recursive = self.is_left_recursive(state_id);
        unimplemented!()
    }

    fn finalize_node(&mut self, work_node: usize, parent_node: usize) {
        // Calculate total width from children
        let total_width: usize = self
            .alloc
            .get_node(work_node)
            .children
            .iter()
            .map(|&child_id| self.alloc.get_node(child_id).width)
            .sum();

        self.alloc.get_node_mut(work_node).width = total_width;
        self.alloc
            .get_node_mut(parent_node)
            .children
            .push(work_node);
    }
}
