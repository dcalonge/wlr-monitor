//! niri: `output "…" { … }` blocks, through `niri msg` and an include file.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use super::{Capabilities, Compositor, RenderContext, quote};
use crate::apply::{Applied, ApplyResult, apply_order};
use crate::exec::{APPLY_TIMEOUT, run_tool};
use crate::output::{Output, OutputState, fmt_num};
use crate::wlr_randr::WlrRandr;

pub const LIVE_ARGS: &[&str] = &["msg", "--json", "outputs"];

/// Top-level output nodes the include generates; anything else is kept.
const MANAGED: [&str; 7] = [
    "off",
    "on",
    "mode",
    "scale",
    "transform",
    "position",
    "max-bpc",
];

#[derive(Deserialize)]
struct Live {
    vrr_enabled: Option<bool>,
    max_bpc: Option<Value>,
}

/// The `output` block of one output.
fn render_output(c: &Compositor, output: &Output, context: &RenderContext) -> Result<Vec<String>> {
    let target = Compositor::target(output, context)?;
    let preserved = context.preserved(&target);
    let on_demand = is_on_demand(preserved);
    let mut lines = vec![format!("output {} {{", quote(&target))];
    if output.enabled {
        if let Some(mode) = &output.mode {
            lines.push(format!("    mode \"{}\"", mode.config_text()));
        }
        lines.push(format!("    scale {}", fmt_num(output.scale, 6)));
        lines.push(format!("    transform \"{}\"", output.transform.as_str()));
        lines.push(format!(
            "    position x={} y={}",
            output.position.x, output.position.y
        ));
        if output.adaptive_sync && !on_demand {
            lines.push("    variable-refresh-rate".to_string());
        }
        // Same gate as the live command: niri up to 25.11 rejects the whole
        // config on an unknown max-bpc node.
        if let Some(depth) = output
            .bit_depth
            .filter(|_| c.bit_depth_supported(output.bit_depth))
        {
            lines.push(format!("    max-bpc {depth}"));
        }
    } else {
        lines.push("    off".to_string());
    }
    for line in preserved {
        // VRR was switched off in the TUI.
        let stale_vrr =
            on_demand && !output.adaptive_sync && line.trim().starts_with("variable-refresh-rate");
        if !stale_vrr {
            lines.push(line.clone());
        }
    }
    lines.push("}".to_string());
    Ok(lines)
}

pub fn render(c: &Compositor, outputs: &[Output], context: &RenderContext) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for output in outputs {
        lines.extend(render_output(c, output, context)?);
    }
    Ok(lines)
}

/// The `niri msg output <name> …` arguments that apply `state`.
fn commands(c: &Compositor, state: &OutputState, on_demand: bool) -> Result<Vec<Vec<String>>> {
    Compositor::check_name(&state.name)?;
    let words = |items: &[&str]| {
        items
            .iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>()
    };
    if !state.enabled {
        return Ok(vec![words(&["off"])]);
    }
    let mut commands = vec![words(&["on"])];
    if let Some(mode) = &state.mode {
        commands.push(vec!["mode".into(), mode.config_text()]);
    }
    commands.push(vec!["scale".into(), fmt_num(state.scale, 6)]);
    commands.push(vec!["transform".into(), state.transform.as_str().into()]);
    commands.push(vec![
        "position".into(),
        "set".into(),
        "--".into(),
        state.position.x.to_string(),
        state.position.y.to_string(),
    ]);
    let mut vrr = words(&["vrr", if state.adaptive_sync { "on" } else { "off" }]);
    if state.adaptive_sync && on_demand {
        vrr.push("--on-demand".into());
    }
    commands.push(vrr);
    if let Some(depth) = state
        .bit_depth
        .filter(|_| c.bit_depth_supported(state.bit_depth))
    {
        commands.push(vec!["max-bpc".into(), depth.to_string()]);
    }
    Ok(commands)
}

/// Which outputs the include configures with on-demand VRR.
fn on_demand_for(c: &Compositor, outputs: &[Output]) -> Result<HashMap<String, bool>> {
    let preserved = c.preserved_options()?;
    if preserved.is_empty() {
        return Ok(HashMap::new());
    }
    let context = c.context(outputs, preserved);
    Ok(outputs
        .iter()
        .filter_map(|output| {
            let target = Compositor::target(output, &context).ok()?;
            Some((
                output.name.clone(),
                is_on_demand(context.preserved(&target)),
            ))
        })
        .collect())
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
    // An unreadable include only loses the on-demand hint here; writing it
    // afterwards fails and is reported as a warning.
    let on_demand = on_demand_for(c, layout).unwrap_or_default();
    // Build (and validate) every command first, so a bad state cannot leave
    // earlier outputs applied and the include unwritten.
    let plan = apply_order(states)
        .into_iter()
        .map(|state| {
            let flag = on_demand.get(&state.name).copied().unwrap_or(false);
            Ok((&state.name, commands(c, state, flag)?))
        })
        .collect::<Result<Vec<_>>>()?;
    let results = plan
        .into_iter()
        .map(|(name, plan)| {
            let result = plan.iter().try_for_each(|args| {
                let mut argv = vec!["msg", "output", name.as_str()];
                argv.extend(args.iter().map(String::as_str));
                run_tool(&tool, &argv, APPLY_TIMEOUT).map(drop)
            });
            ApplyResult::new(name, result)
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
    let Ok(live) = serde_json::from_str::<HashMap<String, Live>>(json) else {
        return;
    };
    // max-bpc arrived after niri 25.11, together with this IPC field.
    if !live.is_empty() && live.values().all(|item| item.max_bpc.is_none()) {
        c.capabilities = Capabilities {
            bit_depths: &[],
            ..c.kind.default_capabilities()
        };
    }
    let on_demand = on_demand_for(c, outputs).unwrap_or_default();
    for output in outputs {
        if on_demand.get(&output.name).copied().unwrap_or(false) {
            // With on-demand VRR, vrr_enabled only reflects the focused
            // window; the include says VRR is on.
            output.adaptive_sync = true;
        } else if let Some(vrr) = live.get(&output.name).and_then(|item| item.vrr_enabled) {
            output.adaptive_sync = vrr;
        }
    }
}

fn is_on_demand(preserved: &[String]) -> bool {
    preserved
        .iter()
        .any(|line| line.trim().starts_with("variable-refresh-rate") && line.contains("on-demand"))
}

/// The target of a generated `output "…" {` line at column 0.
fn block_target(line: &str) -> Option<String> {
    let rest = line.strip_prefix("output")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let body = rest.trim_start().strip_prefix('"')?;
    let mut escaped = false;
    let end = body.char_indices().find_map(|(index, character)| {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some(index),
            _ => {}
        }
        None
    })?;
    let tail = body[end + 1..].trim();
    (tail == "{").then(|| body[..end].replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// Unmanaged lines of each generated `output` block.
///
/// The generated file has a fixed shape (`output "…" {` at column 0, `}`
/// closing it), so a brace-depth scan is enough. Nested blocks such as
/// `layout { … }` are kept verbatim, as is a hand-written
/// `variable-refresh-rate on-demand=true`.
pub fn preserved(text: &str) -> HashMap<String, Vec<String>> {
    let mut result = HashMap::new();
    let mut block: Option<(String, Vec<String>)> = None;
    let mut depth = 0i32;
    for line in text.lines() {
        let Some((target, kept)) = &mut block else {
            if let Some(target) = block_target(line) {
                block = Some((target, Vec::new()));
                depth = 1;
            }
            continue;
        };
        let stripped = line.trim();
        if depth == 1 && stripped == "}" {
            result.insert(target.clone(), std::mem::take(kept));
            block = None;
            continue;
        }
        let node = stripped.split_whitespace().next().unwrap_or("");
        let managed = depth == 1
            && (MANAGED.contains(&node)
                || (node == "variable-refresh-rate" && !stripped.contains("on-demand")));
        if !stripped.is_empty() && !managed {
            kept.push(line.trim_end().to_string());
        }
        depth += stripped.matches('{').count() as i32 - stripped.matches('}').count() as i32;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_unmanaged_lines_and_on_demand_vrr() {
        let text = r#"// header
output "HDMI-A-1" {
    scale 1
    variable-refresh-rate on-demand=true
    layout {
        gaps 8
    }
    focus-at-startup
}
output "eDP-1" {
    off
}
"#;
        let kept = preserved(text);
        assert_eq!(
            kept["HDMI-A-1"],
            [
                "    variable-refresh-rate on-demand=true",
                "    layout {",
                "        gaps 8",
                "    }",
                "    focus-at-startup"
            ]
        );
        assert!(kept["eDP-1"].is_empty());
        assert!(is_on_demand(&kept["HDMI-A-1"]));
    }
}
