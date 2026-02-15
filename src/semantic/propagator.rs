use super::command::Command;
use crate::parsec::tree::{GreenId, Tag, TreeAllocRef, TreeAllocRefExt};
use crate::utils::Span;
use std::collections::VecDeque;

/// Type of change to a raw AST node
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Inserted,
    Deleted,
    Updated,
}

pub struct CommandPropagator {
    commands: Vec<Command>,
    pending_queue: VecDeque<GreenId>,
}

impl CommandPropagator {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            pending_queue: VecDeque::new(),
        }
    }

    pub fn propagate(
        &mut self,
        changed_raw_nodes: Vec<(GreenId, ChangeKind)>,
        lowered_rules: &[usize],
        alloc: &TreeAllocRef,
        spans: &impl Fn(GreenId) -> Span,
    ) -> Vec<Command> {
        for (green_id, change_kind) in changed_raw_nodes {
            let node = alloc.get_node(green_id);
            let should_lower = match &node.tag {
                Tag::Rule { rule_ix } | Tag::Token { rule_ix } => lowered_rules.contains(rule_ix),
                _ => false,
            };

            if !should_lower {
                continue;
            }

            match change_kind {
                ChangeKind::Inserted => {
                    self.commands.push(Command::Insert {
                        green_id,
                        span: spans(green_id),
                    });
                    self.pending_queue.push_back(green_id);
                }
                ChangeKind::Deleted => {
                    self.commands.push(Command::Delete {
                        green_id,
                        span: spans(green_id),
                    });
                    self.pending_queue.push_back(green_id);
                }
                ChangeKind::Updated => {
                    self.commands.push(Command::Update {
                        green_id,
                        span: spans(green_id),
                    });
                    self.pending_queue.push_back(green_id);
                }
            }
        }

        std::mem::take(&mut self.commands)
    }
}

impl Default for CommandPropagator {
    fn default() -> Self {
        Self::new()
    }
}
