pub struct EndOfInput;
pub struct StartOfInput;

pub struct State<'a> {
    input: &'a str,
    position: usize,
}

pub trait Matcher {
    fn matches(&self, state: &mut State) -> bool;
}

impl Matcher for &str {
    fn matches(&self, state: &mut State) -> bool {
        let end_pos = state.position + self.len();
        if end_pos <= state.input.len() && &state.input[state.position..end_pos] == *self {
            state.position = end_pos;
            true
        } else {
            false
        }
    }
}

impl Matcher for EndOfInput {
    fn matches(&self, state: &mut State) -> bool {
        state.position >= state.input.len()
    }
}

impl Matcher for StartOfInput {
    fn matches(&self, state: &mut State) -> bool {
        state.position == 0
    }
}
