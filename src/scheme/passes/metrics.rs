/// Per-edit performance metrics collected during reparse and semantic phases
#[derive(Clone, Debug)]
pub struct EditMetrics {
    // Timings (microseconds)
    pub total_duration_us: u128,
    pub zipper_collection_us: u128,
    pub candidate_evaluation_us: u128,
    pub parse_rule_total_us: u128,
    pub semantic_diff_us: u128,

    // Counters
    pub candidates_collected: usize,
    pub candidates_evaluated: usize,
    pub parse_rule_calls: usize,
    pub parse_rule_cache_hits: usize,
    pub semantic_commands_emitted: usize,

    // Flags
    pub used_incremental_path: bool,
    pub fell_back_to_full_diff: bool,

    // Diagnostic
    pub message: String,
}

impl Default for EditMetrics {
    fn default() -> Self {
        Self {
            total_duration_us: 0,
            zipper_collection_us: 0,
            candidate_evaluation_us: 0,
            parse_rule_total_us: 0,
            semantic_diff_us: 0,

            candidates_collected: 0,
            candidates_evaluated: 0,
            parse_rule_calls: 0,
            parse_rule_cache_hits: 0,
            semantic_commands_emitted: 0,

            used_incremental_path: false,
            fell_back_to_full_diff: false,

            message: String::new(),
        }
    }
}
