pub trait IR {
    type Ix;
    type Value;
}

pub enum Command<Repr: IR> {
    Replace { index: Repr::Ix, value: Repr::Value },
    Insert { index: Repr::Ix, value: Repr::Value },
    Delete { index: Repr::Ix },
}

pub struct Transaction<Repr: IR> {
    pub commands: Vec<Command<Repr>>,
}
