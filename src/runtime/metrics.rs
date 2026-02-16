use std::time::Instant;

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

/// High-resolution timer for nested measurements
pub struct Timer {
    start: Instant,
    name: String,
}

impl Timer {
    pub fn start(name: &str) -> Self {
        Self {
            start: Instant::now(),
            name: name.to_string(),
        }
    }

    pub fn stop(self) -> u128 {
        self.start.elapsed().as_micros()
    }

    pub fn stop_and_record(self, dest: &mut u128) {
        let elapsed = self.stop();
        *dest += elapsed;
    }
}

/// RAII-style scoped timer that auto-records on drop
pub struct ScopedTimer {
    start: Instant,
    dest: *mut u128,
    _name: String,
}

impl ScopedTimer {
    pub fn new(dest: &mut u128, name: &str) -> Self {
        Self {
            start: Instant::now(),
            dest: dest as *mut u128,
            _name: name.to_string(),
        }
    }
}

impl Drop for ScopedTimer {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_micros();
        unsafe {
            *self.dest += elapsed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_timer() {
        let t = Timer::start("test");
        thread::sleep(std::time::Duration::from_millis(1));
        let us = t.stop();
        assert!(us >= 1000, "Expected >= 1000µs, got {}", us);
    }

    #[test]
    fn test_scoped_timer() {
        let mut dest = 0u128;
        {
            let _t = ScopedTimer::new(&mut dest, "test");
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(dest >= 1000, "Expected >= 1000µs, got {}", dest);
    }

    #[test]
    fn test_metrics_summary() {
        let mut m = EditMetrics::new();
        m.total_duration_us = 5000;
        m.parse_rule_calls = 10;
        m.parse_rule_cache_hits = 7;
        m.candidates_collected = 5;
        m.candidates_evaluated = 3;
        println!("{}", m.summary());
    }
}
