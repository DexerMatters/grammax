#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Self-contained incremental tree delta.
    ///
    /// - `changed_green`: the deepest replaced green (incremental reparse result)
    /// - `changed_offset`: absolute source offset of `changed_green`
    /// - `lineage`: immediate-parent-to-root chain as `(parent_green, child_index)`
    /// - `new_root`: resulting parse root after this edit
    TreeChanged {
        changed_green: usize,
        changed_offset: usize,
        lineage: Vec<(usize, usize)>,
        new_root: usize,
    },

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

    /// Update an entire zipper path (leaf to root) efficiently.
    /// Encodes multiple parent updates as a compact path.
    /// Each step is (parent_green: Option, child_index, new_child_green).
    /// The last step has parent_green=None (root update).
    PathUpdate { path: Vec<(Option<usize>, usize, usize)> },
}
