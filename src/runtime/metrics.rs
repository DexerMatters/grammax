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

impl EditMetrics {
    pub fn new() -> Self {
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

    pub fn cache_hit_ratio(&self) -> f64 {
        if self.parse_rule_calls == 0 {
            0.0
        } else {
            self.parse_rule_cache_hits as f64 / self.parse_rule_calls as f64
        }
    }

    pub fn candidate_evaluation_rate(&self) -> f64 {
        if self.candidates_collected == 0 {
            0.0
        } else {
            self.candidates_evaluated as f64 / self.candidates_collected as f64
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "[EditMetrics] total={:.2}ms | zipper={:.2}µs | eval={:.2}ms | parse={:.2}ms | semantic={:.2}ms | \
             candidates={}/{} | parse_calls={} (hits={:.0}%) | commands={} | incremental={} | fallback={}",
            self.total_duration_us as f64 / 1000.0,
            self.zipper_collection_us,
            self.candidate_evaluation_us as f64 / 1000.0,
            self.parse_rule_total_us as f64 / 1000.0,
            self.semantic_diff_us as f64 / 1000.0,
            self.candidates_evaluated,
            self.candidates_collected,
            self.parse_rule_calls,
            self.cache_hit_ratio() * 100.0,
            self.semantic_commands_emitted,
            self.used_incremental_path,
            self.fell_back_to_full_diff,
        )
    }
}
