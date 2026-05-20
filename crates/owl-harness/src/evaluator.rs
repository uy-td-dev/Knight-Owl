//! Evaluator for scoring agent reasoning runs.

use owl_protocol::tools::ToolCall;

/// Scores an agent run by comparing the actual tool call sequence to expected.
pub struct Evaluator {
    expected: Vec<String>,
}

impl Evaluator {
    /// Construct an evaluator with the expected tool-call sequence (by name).
    pub fn new(expected: Vec<&str>) -> Self {
        Self { expected: expected.iter().map(|s| s.to_string()).collect() }
    }

    /// Compute score = correct_steps / total_expected_steps.
    ///
    /// Returns a value in `[0.0, 1.0]`.
    pub fn score(&self, actual: &[ToolCall]) -> f64 {
        if self.expected.is_empty() {
            return 1.0;
        }
        let correct = actual
            .iter()
            .zip(self.expected.iter())
            .filter(|(a, e)| &a.name == *e)
            .count();
        correct as f64 / self.expected.len() as f64
    }
}
