//! mango: `monitor_rule=` lines, through `mmsg` and an include file.

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde::Deserialize;

use super::{Compositor, RenderContext};
use crate::apply::{Applied, ApplyResult, apply_order};
use crate::exec::{APPLY_TIMEOUT, run_tool};
use crate::output::{Output, OutputState, Transform, fmt_num, is_printable};
use crate::wlr_randr::WlrRandr;

/// mango reads config lines into a 512-byte buffer.
const MAX_LINE: usize = 510;
const MATCH_KEYS: [&str; 4] = ["name", "make", "model", "serial"];
/// `monitor_rule` keys the include generates; anything else is kept.
const MANAGED: [&str; 15] = [
    "name", "make", "model", "serial", "width", "height", "refresh", "x", "y", "scale", "rr",
    "vrr", "hdr", "icc", "disable",
];

#[derive(Deserialize)]
struct Live {
    monitors: Vec<Monitor>,
}

#[derive(Deserialize)]
struct Monitor {
    name: String,
    is_hdr: Option<bool>,
}

/// How a rule finds its output: EDID make/model/serial when that is
/// unambiguous and safe to write, else the connector name.
fn rule_match(output: &Output, context: &RenderContext) -> Result<String> {
    let parts = [&output.make, &output.model, &output.serial];
    if parts
        .iter()
        .all(|part| !part.is_empty() && !part.contains(',') && is_printable(part))
        && !context.ambiguous.contains(&output.identifier())
    {
        return Ok(format!(
            "make:{},model:{},serial:{}",
            output.make, output.model, output.serial
        ));
    }
    Compositor::check_name(&output.name)?;
    let mut escaped = String::new();
    for character in output.name.chars() {
        if ".^$*+?()[]{}|\\".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    Ok(format!("name:^{escaped}$"))
}

fn render_output(c: &Compositor, output: &Output, context: &RenderContext) -> Result<String> {
    let matcher = rule_match(output, context)?;
    let preserved = context.preserved(&matcher);
    let mut fields = vec![matcher];
    if output.enabled {
        if let Some(mode) = &output.mode {
            fields.push(format!("width:{}", mode.width));
            fields.push(format!("height:{}", mode.height));
            fields.push(format!("refresh:{}", fmt_num(mode.refresh, 3)));
        }
        let rotation = Transform::ALL
            .iter()
            .position(|t| *t == output.transform)
            .unwrap_or(0);
        fields.push(format!("x:{}", output.position.x));
        fields.push(format!("y:{}", output.position.y));
        fields.push(format!("scale:{}", fmt_num(output.scale, 6)));
        fields.push(format!("rr:{rotation}"));
        fields.push(format!("vrr:{}", u8::from(output.adaptive_sync)));
        if let Some(hdr) = output.hdr {
            fields.push(format!("hdr:{}", u8::from(hdr)));
        }
        if let Some(path) = output
            .color_profile
            .as_deref()
            .filter(|profile| c.color_profile_supported(profile))
            .and_then(|profile| profile.strip_prefix("icc:"))
        {
            fields.push(format!("icc:{path}"));
        }
    } else {
        fields.push("disable:1".to_string());
    }
    fields.extend(preserved.iter().cloned());
    let line = format!("monitor_rule={}", fields.join(","));
    if line.len() > MAX_LINE {
        bail!(
            "The monitor rule for {} is longer than mango's {MAX_LINE}-character line limit \
             (shorten the ICC path).",
            output.name
        );
    }
    Ok(line)
}

pub fn render(c: &Compositor, outputs: &[Output], context: &RenderContext) -> Result<Vec<String>> {
    outputs
        .iter()
        .map(|output| render_output(c, output, context))
        .collect()
}

/// Run `mmsg`. It reports some failures on stdout with exit status 0.
fn mmsg(tool: &str, args: &[&str]) -> Result<()> {
    let stdout = run_tool(tool, args, APPLY_TIMEOUT)?;
    if stdout.contains("\"error\"") || stdout.contains("Error:") {
        bail!("mmsg failed: {}", stdout.trim());
    }
    Ok(())
}

pub fn apply(
    c: &Compositor,
    wlr: &WlrRandr,
    states: &[OutputState],
    layout: &[Output],
    profile: Option<&str>,
) -> Result<Applied> {
    let tool = c.tool_path();
    if !layout.is_empty() && c.include_enabled && c.include_is_sourced() {
        let mut warnings = Vec::new();
        warnings.extend(
            c.write_include(layout, profile)?
                .and_then(|written| written.warning),
        );
        let result = match &tool {
            Some(tool) => mmsg(tool, &["dispatch", "reload_config"]),
            None => {
                // mango reloads sourced files itself (auto_reload_config=1).
                warnings.push(
                    "mmsg was not found; relying on mango's auto_reload_config to pick up the \
                     new monitor rules."
                        .to_string(),
                );
                Ok(())
            }
        };
        return Ok(Applied {
            results: super::same_result(states, result),
            warnings,
        });
    }

    // Not sourced yet: apply what the protocol can do and explain the rest.
    let mut applied = wlr.apply(states);
    if let Some(tool) = tool.filter(|_| applied.succeeded()) {
        for state in apply_order(states) {
            if let (true, Some(hdr)) = (state.enabled, state.hdr) {
                let dispatch = format!(
                    "togglehdr,{},{}",
                    if hdr { "on" } else { "off" },
                    state.name
                );
                let result = mmsg(&tool, &["dispatch", &dispatch]);
                applied.results.push(ApplyResult::new(&state.name, result));
            }
        }
    }
    let mut applied = c.finish(applied, layout, profile);
    if states
        .iter()
        .any(|state| state.enabled && state.adaptive_sync)
    {
        let advice = if c.include_enabled {
            format!(
                "Add `{}` to your mango config so the generated monitor rules carry VRR.",
                c.include_line()
            )
        } else {
            "With --no-include, VRR will not stick.".to_string()
        };
        applied.warnings.push(format!(
            "mango switches VRR off again unless it comes from a monitor_rule. {advice}"
        ));
    }
    Ok(applied)
}

pub fn enrich(outputs: &mut [Output], json: &str) {
    let Ok(live) = serde_json::from_str::<Live>(json) else {
        return;
    };
    for output in outputs {
        let hdr = live
            .monitors
            .iter()
            .rev()
            .find(|monitor| monitor.name == output.name);
        if let Some(hdr) = hdr.and_then(|monitor| monitor.is_hdr) {
            output.hdr = Some(hdr);
        }
    }
}

/// Unmanaged `key:value` fields of each generated monitor rule, by match.
pub fn preserved(text: &str) -> HashMap<String, Vec<String>> {
    let mut result = HashMap::new();
    for rule in text
        .lines()
        .filter_map(|line| line.strip_prefix("monitor_rule="))
    {
        let fields: Vec<&str> = rule.split(',').collect();
        let key = |field: &str| field.split(':').next().unwrap_or("").trim().to_string();
        let matcher = fields
            .iter()
            .filter(|field| MATCH_KEYS.contains(&key(field).as_str()))
            .copied()
            .collect::<Vec<_>>()
            .join(",");
        let kept = fields
            .iter()
            .filter(|field| field.contains(':') && !MANAGED.contains(&key(field).as_str()))
            .map(|field| field.trim().to_string())
            .collect();
        if !matcher.is_empty() {
            result.insert(matcher, kept);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_unmanaged_rule_fields() {
        let kept = preserved("monitor_rule=name:^DP-1$,width:1920,x:0,hdr_max_lum:600,primary:1\n");
        assert_eq!(kept["name:^DP-1$"], ["hdr_max_lum:600", "primary:1"]);
    }
}
