//! sway: `output` commands, through `swaymsg` and an include file.

use anyhow::Result;
use serde::Deserialize;

use super::{Capabilities, Compositor, RenderContext, quote};
use crate::apply::{Applied, ApplyResult, apply_order};
use crate::exec::{APPLY_TIMEOUT, run_tool};
use crate::output::{Output, OutputState, fmt_num};
use crate::wlr_randr::WlrRandr;

pub const LIVE_ARGS: &[&str] = &["-t", "get_outputs", "-r"];

#[derive(Deserialize)]
struct Live {
    name: String,
    hdr: Option<bool>,
    adaptive_sync_status: Option<String>,
}

/// The arguments after `output <name>`.
///
/// The result is a sway command string: `;` or `,` in a value would chain
/// more commands, so everything that is not a number is validated or quoted.
fn output_args(c: &Compositor, state: &OutputState) -> Result<String> {
    Compositor::check_name(&state.name)?;
    if !state.enabled {
        return Ok("disable".to_string());
    }
    let mut parts = vec!["enable".to_string()];
    if let Some(mode) = &state.mode {
        parts.push(format!("mode {}Hz", mode.config_text()));
    }
    parts.push(format!("pos {} {}", state.position.x, state.position.y));
    parts.push(format!("scale {}", fmt_num(state.scale, 6)));
    parts.push(format!("transform {}", state.transform.as_str()));
    parts.push(format!("adaptive_sync {}", on_off(state.adaptive_sync)));
    if let Some(depth) = state
        .bit_depth
        .filter(|_| c.bit_depth_supported(state.bit_depth))
    {
        parts.push(format!("render_bit_depth {depth}"));
    }
    if let Some(hdr) = state.hdr.filter(|_| c.capabilities.hdr) {
        parts.push(format!("hdr {}", on_off(hdr)));
    }
    if let Some(profile) = state
        .color_profile
        .as_deref()
        .filter(|profile| c.color_profile_supported(profile))
    {
        match profile.strip_prefix("icc:") {
            Some(path) => parts.push(format!("color_profile icc {}", quote(path))),
            None => parts.push(format!("color_profile {profile}")),
        }
    }
    Ok(parts.join(" "))
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

pub fn render(c: &Compositor, outputs: &[Output], context: &RenderContext) -> Result<Vec<String>> {
    outputs
        .iter()
        .map(|output| {
            let target = Compositor::target(output, context)?;
            Ok(format!(
                "output {} {}",
                quote(&target),
                output_args(c, &output.state())?
            ))
        })
        .collect()
}

pub fn apply(
    c: &Compositor,
    wlr: &WlrRandr,
    states: &[OutputState],
    layout: &[Output],
    profile: Option<&str>,
) -> Result<Applied> {
    let Some(tool) = c.tool_path() else {
        return Ok(c.apply_without_tool(wlr, states, layout, profile));
    };
    // Build (and validate) every command before running any of them.
    let commands = apply_order(states)
        .into_iter()
        .map(|state| {
            let command = format!("output {} {}", quote(&state.name), output_args(c, state)?);
            Ok((&state.name, command))
        })
        .collect::<Result<Vec<_>>>()?;
    let results = commands
        .into_iter()
        .map(|(name, command)| {
            ApplyResult::new(
                name,
                run_tool(&tool, &["--", &command], APPLY_TIMEOUT).map(drop),
            )
        })
        .collect();
    Ok(c.finish(
        Applied {
            results,
            warnings: Vec::new(),
        },
        layout,
        profile,
    ))
}

pub fn enrich(c: &mut Compositor, outputs: &mut [Output], json: &str) {
    let Ok(live) = serde_json::from_str::<Vec<Live>>(json) else {
        return;
    };
    // get_outputs gained "hdr" in sway 1.11, together with the hdr command
    // and the gamma22/srgb color profiles.
    if !live.is_empty() && live.iter().all(|item| item.hdr.is_none()) {
        c.capabilities = Capabilities {
            hdr: false,
            color_presets: &[],
            ..c.kind.default_capabilities()
        };
    }
    for output in outputs {
        let Some(item) = live.iter().rev().find(|item| item.name == output.name) else {
            continue;
        };
        if item.hdr.is_some() {
            output.hdr = item.hdr;
        }
        match item.adaptive_sync_status.as_deref() {
            Some("enabled") => output.adaptive_sync = true,
            Some("disabled") => output.adaptive_sync = false,
            _ => {}
        }
    }
}
