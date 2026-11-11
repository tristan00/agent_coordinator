//! Shared definitions for workspace and host summaries. No task bodies or logs leave this module.
use crate::model::Snapshot;
use serde::Serialize;

#[derive(Default, Serialize)]
pub struct Metrics {
    pub tasks: u64,
    pub completed: u64,
    pub attention: u64,
    pub running_attempts: u64,
    pub attempts: u64,
    pub failed_attempts: u64,
    pub sendbacks: u64,
    pub duration_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub input_token_attempts: u64,
    pub output_token_attempts: u64,
    pub cost_attempts: u64,
}

impl Metrics {
    pub fn from_snapshot(state: &Snapshot) -> Self {
        let mut result = Self::default();
        for task in state.tasks.iter().filter(|task| task.kind != "epic") {
            result.sendbacks += u64::from(task.sendbacks);
            if !task.archived {
                result.tasks += 1;
                result.completed += u64::from(task.status == "completed");
                result.attention += u64::from(matches!(
                    task.status.as_str(),
                    "blocked" | "needs_input" | "failed"
                ));
            }
        }
        for attempt in &state.attempts {
            result.attempts += 1;
            result.running_attempts += u64::from(attempt.status == "running");
            result.failed_attempts += u64::from(attempt.status == "failed");
            result.duration_ms += attempt.duration_ms.unwrap_or(0);
            if let Some(value) = attempt.usage.input_tokens {
                *result.input_tokens.get_or_insert(0) += value;
                result.input_token_attempts += 1;
            }
            if let Some(value) = attempt.usage.output_tokens {
                *result.output_tokens.get_or_insert(0) += value;
                result.output_token_attempts += 1;
            }
            if let Some(value) = attempt.usage.cost_usd {
                *result.cost_usd.get_or_insert(0.0) += value;
                result.cost_attempts += 1;
            }
        }
        result
    }

    pub fn add(&mut self, other: &Self) {
        self.tasks += other.tasks;
        self.completed += other.completed;
        self.attention += other.attention;
        self.running_attempts += other.running_attempts;
        self.attempts += other.attempts;
        self.failed_attempts += other.failed_attempts;
        self.sendbacks += other.sendbacks;
        self.duration_ms += other.duration_ms;
        self.input_token_attempts += other.input_token_attempts;
        self.output_token_attempts += other.output_token_attempts;
        self.cost_attempts += other.cost_attempts;
        if let Some(value) = other.input_tokens {
            *self.input_tokens.get_or_insert(0) += value;
        }
        if let Some(value) = other.output_tokens {
            *self.output_tokens.get_or_insert(0) += value;
        }
        if let Some(value) = other.cost_usd {
            *self.cost_usd.get_or_insert(0.0) += value;
        }
    }
}
