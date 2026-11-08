//! wlr-randr plus the compositor integration, as one thing to read and apply.

use std::thread;

use anyhow::Result;

use crate::apply::Applied;
use crate::compositor::{Compositor, Written};
use crate::output::{Output, OutputState};
use crate::wlr_randr::WlrRandr;

pub struct Backend {
    pub wlr: WlrRandr,
    pub compositor: Compositor,
}

impl Backend {
    pub fn new(wlr: WlrRandr, compositor: Compositor) -> Self {
        Self { wlr, compositor }
    }

    /// The live outputs, with the compositor's extra settings filled in.
    pub fn read_outputs(&mut self) -> Result<Vec<Output>> {
        // The IPC query does not depend on wlr-randr's answer, so run both
        // tools at the same time.
        let (outputs, live) = thread::scope(|scope| {
            let live = scope.spawn(|| self.compositor.query_live());
            (self.wlr.read(), live.join().ok().flatten())
        });
        let mut outputs = outputs?;
        self.compositor.enrich(&mut outputs, live.as_deref());
        Ok(outputs)
    }

    /// The full layout after applying `states` to the live outputs. Empty if
    /// the compositor has no include to write.
    fn target_layout(&mut self, states: &[OutputState]) -> Vec<Output> {
        if self.compositor.include_path().is_none() {
            return Vec::new();
        }
        let mut outputs = self.read_outputs().unwrap_or_default();
        for output in &mut outputs {
            if let Some(state) = states.iter().rev().find(|state| state.name == output.name) {
                state.apply_to(output, true);
            }
        }
        outputs
    }

    pub fn apply(&mut self, states: &[OutputState], profile: Option<&str>) -> Result<Applied> {
        let layout = self.target_layout(states);
        self.compositor.apply(&self.wlr, states, &layout, profile)
    }

    /// Write the include from the live layout (`--init-include`).
    pub fn write_include(&mut self, profile: Option<&str>) -> Result<Option<Written>> {
        let outputs = self.read_outputs()?;
        self.compositor.write_include(&outputs, profile)
    }
}
