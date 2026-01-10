use std::rc::Rc;

use indexmap::IndexSet;

use crate::{grammar::Rule, words::Matcher};

pub enum ThreadedNode {
    Terminal(Rc<dyn Matcher>, Rc<ThreadedNode>),
    Scope(usize, Rc<ThreadedNode>),
    Try(Rc<ThreadedNode>),
    End,
}
