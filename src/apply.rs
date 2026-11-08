//! What applying settings to the outputs reports back.

use crate::output::OutputState;

/// The outcome for one output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyResult {
    pub name: String,
    pub error: Option<String>,
}

impl ApplyResult {
    pub fn new(name: &str, result: anyhow::Result<()>) -> Self {
        Self {
            name: name.to_string(),
            error: result.err().map(|error| error.to_string()),
        }
    }
}

/// Per-output results plus messages that are not about a single output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub results: Vec<ApplyResult>,
    pub warnings: Vec<String>,
}

impl Applied {
    pub fn failures(&self) -> impl Iterator<Item = &ApplyResult> {
        self.results.iter().filter(|result| result.error.is_some())
    }

    pub fn succeeded(&self) -> bool {
        self.failures().next().is_none()
    }
}

/// Enabled outputs first, so a session never has none while applying.
pub fn apply_order(states: &[OutputState]) -> Vec<&OutputState> {
    let (enabled, disabled): (Vec<_>, Vec<_>) = states.iter().partition(|state| state.enabled);
    enabled.into_iter().chain(disabled).collect()
}
