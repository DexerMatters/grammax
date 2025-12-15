pub struct EndOfInput;
pub struct StartOfInput;

pub struct Alternative<T, U>(T, U);

pub struct Sequence<T, U>(T, U);

pub struct State<'a> {
    input: &'a str,
    position: usize,
}

pub trait Matcher {
    fn matches(&self, state: &mut State) -> bool;
    fn display(&self) -> String {
        String::from("<terminal>")
    }
    fn is_nullable(&self) -> bool {
        false
    }
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

    fn display(&self) -> String {
        format!("\"{}\"", self)
    }

    fn is_nullable(&self) -> bool {
        self.len() == 0
    }
}

impl Matcher for EndOfInput {
    fn matches(&self, state: &mut State) -> bool {
        state.position >= state.input.len()
    }

    fn display(&self) -> String {
        String::from("EOF")
    }

    fn is_nullable(&self) -> bool {
        true
    }
}

impl Matcher for StartOfInput {
    fn matches(&self, state: &mut State) -> bool {
        state.position == 0
    }

    fn display(&self) -> String {
        String::from("SOF")
    }

    fn is_nullable(&self) -> bool {
        true
    }
}
impl<T, U> Matcher for Alternative<T, U>
where
    T: Matcher,
    U: Matcher,
{
    fn matches(&self, state: &mut State) -> bool {
        let original_position = state.position;
        if self.0.matches(state) {
            true
        } else {
            state.position = original_position;
            self.1.matches(state)
        }
    }
}

impl<T, U> Matcher for Sequence<T, U>
where
    T: Matcher,
    U: Matcher,
{
    fn matches(&self, state: &mut State) -> bool {
        self.0.matches(state) && self.1.matches(state)
    }
}
