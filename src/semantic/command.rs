#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Declare a newly introduced green node (and its structural metadata)
    CreateGreen { green: usize },

    /// Replace a green child under a parent position.
    /// parent_green is None when the root green changes.
    ReplaceGreen {
        parent_green: Option<usize>,
        child_index: usize,
        new_green: usize,
    },

    /// Insert a green child at an index under a parent green.
    InsertGreen {
        parent_green: usize,
        child_index: usize,
        green: usize,
    },

    /// Delete a green child at an index under a parent green.
    DeleteGreen {
        parent_green: usize,
        child_index: usize,
        green: usize,
    },
}
