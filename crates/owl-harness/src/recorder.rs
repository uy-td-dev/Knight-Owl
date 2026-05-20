//! `TraceRecorder` — captures the [`WorkflowEvent`] stream of one workflow
//! run into a serialisable [`Trace`] that doubles as a regression baseline.
//!
//! ## Determinism strategy
//!
//! Wall-clock fields (`started_at`, `finished_at`) are stripped from the
//! recorded trace before persisting — they're noise from the harness's
//! perspective and would force a baseline rewrite on every CI run.
//! Everything else is kept verbatim:
//!
//! - Event sequence + types
//! - Step ids + agents + dependencies
//! - Per-step status + attempts
//! - Workflow outcome + success flag
//!
//! ## Comparison
//!
//! [`Trace::diff`] performs a structural diff against a stored baseline and
//! returns a list of [`TraceDiff`] mismatches.  CI tests fail on any
//! non-empty diff.  Adding new behaviour = re-record the baseline (write a
//! new `*.json` under `crates/owl-harness/baselines/`).

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use owl_orchestra::WorkflowEvent;

/// Captured trace of a single workflow run.
///
/// Stored as JSON in `crates/owl-harness/baselines/<scenario>.json` and
/// reloaded by harness tests for regression detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    /// Logical name — usually the scenario or workflow id.
    pub name:   String,
    /// Captured events in arrival order, with timestamps zeroed.
    pub events: Vec<WorkflowEvent>,
}

impl Trace {
    /// Drain a `WorkflowEvent` channel into a [`Trace`] until it closes.
    ///
    /// Run as a background task while the workflow executes:
    ///
    /// ```ignore
    /// let (tx, rx) = mpsc::channel(64);
    /// let recorder = tokio::spawn(Trace::record("refactor", rx));
    /// engine.run(spec, input, tx, cancel).await?;
    /// let trace = recorder.await.unwrap();
    /// ```
    pub async fn record(
        name:    impl Into<String>,
        mut rx:  mpsc::Receiver<WorkflowEvent>,
    ) -> Self {
        let mut events = Vec::new();
        while let Some(evt) = rx.recv().await {
            events.push(canonicalise(evt));
        }
        Self { name: name.into(), events }
    }

    /// Serialise to pretty JSON for baseline storage.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a previously-recorded baseline.
    pub fn from_json(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Structural diff against a baseline.  Wall-clock fields are
    /// already canonicalised so any difference is a real regression.
    /// Returns an empty `Vec` when the runs match.
    pub fn diff(&self, baseline: &Trace) -> Vec<TraceDiff> {
        let mut diffs = Vec::new();
        if self.events.len() != baseline.events.len() {
            diffs.push(TraceDiff::EventCount {
                expected: baseline.events.len(),
                actual:   self.events.len(),
            });
        }
        for (i, (got, want)) in self.events.iter().zip(baseline.events.iter()).enumerate() {
            if !events_equal(got, want) {
                diffs.push(TraceDiff::EventMismatch {
                    index:    i,
                    expected: format!("{want:?}"),
                    actual:   format!("{got:?}"),
                });
            }
        }
        diffs
    }
}

/// One mismatch between an actual run and its baseline.
#[derive(Debug, Clone)]
pub enum TraceDiff {
    EventCount    { expected: usize, actual: usize },
    EventMismatch { index: usize, expected: String, actual: String },
}

impl std::fmt::Display for TraceDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceDiff::EventCount { expected, actual } =>
                write!(f, "event count differs: expected {expected}, got {actual}"),
            TraceDiff::EventMismatch { index, expected, actual } =>
                write!(f, "event #{index} differs:\n  expected: {expected}\n  actual:   {actual}"),
        }
    }
}

// ─── Canonicalisation ────────────────────────────────────────────────────────

/// Zero out fields that change between runs even when the agent's behaviour
/// is identical (timestamps, prompt text trimming differences).  Returning a
/// `WorkflowEvent` keeps the public surface compact.
fn canonicalise(evt: WorkflowEvent) -> WorkflowEvent {
    use owl_orchestra::WorkflowEvent::*;
    match evt {
        StepCompleted { trace_id, step, mut result } => {
            result.started_at = 0;
            result.finished_at = 0;
            // Keep `attempts` + `status` + `output` — those are the
            // behaviour we want to lock in.
            StepCompleted { trace_id, step, result }
        }
        WorkflowCompleted { mut outcome } => {
            outcome.started_at = 0;
            outcome.finished_at = 0;
            for s in outcome.step_results.iter_mut() {
                s.started_at = 0;
                s.finished_at = 0;
            }
            WorkflowCompleted { outcome }
        }
        other => other,
    }
}

/// Variant-aware structural equality — compares everything in canonicalised
/// form using `serde_json::Value`.  Avoids needing to derive `PartialEq`
/// across every nested protocol type.
fn events_equal(a: &WorkflowEvent, b: &WorkflowEvent) -> bool {
    let av = serde_json::to_value(a).unwrap_or(serde_json::Value::Null);
    let bv = serde_json::to_value(b).unwrap_or(serde_json::Value::Null);
    av == bv
}
