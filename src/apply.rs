//! What applying settings to the outputs reports back.

use crate::output::{Mode, Output, OutputState};

/// How far the live scale reported by the compositor may differ from the
/// requested scale before verification fails. wlroots quantizes fractional
/// scales (1.1 → 1.1015625), so a small tolerance is needed.
pub const SCALE_TOLERANCE: f64 = 0.01;

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
/// Check that the requested layout settings actually took effect, by reading
/// the live state back from wlr-randr.
/// `live` is the output of `wlr-randr --json`. Outputs we cannot read are
/// not reported: without evidence they must not fail an apply that may well
/// have worked.
///
/// Returns the names of outputs whose live settings differ from the request.
pub fn verify_layout(
    states: &[OutputState],
    live: Option<&[Output]>,
    applied: &[ApplyResult],
) -> Vec<String> {
    let Some(live) = live else { return Vec::new() };
    let mut failed = Vec::new();
    for state in states.iter().filter(|s| s.enabled) {
        // An output that already failed its command does not need a second failure.
        if applied
            .iter()
            .any(|r| r.name == state.name && r.error.is_some())
        {
            continue;
        }
        let Some(live_output) = live.iter().find(|o| o.name == state.name) else {
            continue;
        };
        let mode_mismatch = state.mode.is_some()
            && live_output.mode.is_some()
            && state.mode.as_ref().map(Mode::key) != live_output.mode.as_ref().map(Mode::key);
        // wlroots quantizes fractional scales to 1/128 (1.1 becomes
        // 1.1015625), so accept a small tolerance instead of demanding
        // bit-exact equality.
        if state.enabled != live_output.enabled
            || mode_mismatch
            || state.position != live_output.position
            || (state.scale - live_output.scale).abs() > SCALE_TOLERANCE
            || state.transform != live_output.transform
            || state.adaptive_sync != live_output.adaptive_sync
        {
            failed.push(state.name.clone());
        }
    }
    failed
}
