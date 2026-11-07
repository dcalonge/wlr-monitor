from __future__ import annotations

import json
from pathlib import Path

import pytest

from wlr_randr_tui.model import (
    COMMON_SCALES,
    SCALE_MAX,
    SCALE_MIN,
    Mode,
    Output,
    OutputState,
    clamp_scale,
    common_scales,
    disables_everything,
    format_refresh,
    match_state,
    refresh_arg,
    step_scale,
)

FIXTURES = Path(__file__).parent / "fixtures"


def load_outputs() -> list[Output]:
    data = json.loads((FIXTURES / "outputs.json").read_text())
    return [Output.from_json(item) for item in data]


def test_parse_outputs() -> None:
    hdmi, edp = load_outputs()
    assert hdmi.name == "HDMI-A-1"
    assert hdmi.enabled
    assert hdmi.mode is not None
    assert hdmi.mode.size_text == "1920x1080"
    assert hdmi.mode.refresh == 100.0
    assert hdmi.x == 0
    assert edp.scale == 1.2
    assert edp.adaptive_sync
    assert edp.logical_width == 1600
    assert edp.logical_height == 900


def test_hardware_id_uses_edid_fields() -> None:
    hdmi = load_outputs()[0]
    assert hdmi.hardware_id == "Microstep/MSI MP275/PC3M394200857"
    assert "Microstep" in hdmi.description


def test_rotated_logical_size() -> None:
    output = load_outputs()[1]
    output.transform = "90"
    assert output.logical_width == 900
    assert output.logical_height == 1600


def test_mode_labels_and_cli_values() -> None:
    mode = Mode(width=1920, height=1080, refresh=59.939999)
    assert mode.label == "1920x1080 @ 59.94 Hz"
    assert mode.cli_value() == "1920x1080@59.939999"
    assert format_refresh(100.0) == "100"
    assert refresh_arg(100.0) == "100"


def test_mode_options_prefers_current_then_preferred() -> None:
    output = load_outputs()[0]
    output.modes.append(Mode(width=1920, height=1080, refresh=100.0))
    modes = output.mode_options()
    assert modes[0].refresh == 100.0
    assert len(modes) == len({mode.key() for mode in modes})


def test_state_round_trip() -> None:
    output = load_outputs()[0]
    state = output.state()
    raw = state.to_dict()
    restored = OutputState.from_dict(output.name, raw)
    assert restored.to_dict() == raw
    assert restored.mode is not None
    assert restored.mode.key() == state.mode.key()
    assert restored.hardware_id == output.hardware_id


def test_state_apply_to_output_uses_matching_mode() -> None:
    output = load_outputs()[0]
    state = output.state()
    state.mode = Mode(width=1280, height=720, refresh=60.0)
    state.x = 100
    state.scale = 1.5
    state.apply_to(output)
    assert output.x == 100
    assert output.scale == 1.5
    assert output.mode is not None
    assert output.mode.key() == (1280, 720, 60.0)


def test_state_matches_config_ignores_identity_fields() -> None:
    output = load_outputs()[0]
    first = output.state()
    second = output.state()
    second.hardware_id = "different"
    second.description = "different"
    assert first.matches_config(second)
    second.x += 1
    assert not first.matches_config(second)


def test_common_scales_matches_hyprmon_presets() -> None:
    assert COMMON_SCALES == (
        0.50,
        0.75,
        0.90,
        1.00,
        1.10,
        1.25,
        1.33,
        1.50,
        1.66,
        1.75,
        2.00,
        2.25,
        2.50,
        2.75,
        3.00,
    )
    assert common_scales() == list(COMMON_SCALES)


def test_common_scales_includes_custom_current() -> None:
    scales = common_scales(1.33)
    assert 1.33 in scales
    assert 1.0 in scales
    assert scales == sorted(set(scales))


def test_clamp_and_step_scale() -> None:
    assert clamp_scale(0.1) == SCALE_MIN
    assert clamp_scale(9.0) == SCALE_MAX
    assert clamp_scale(1.234) == 1.23
    assert step_scale(1.0, 1) == 1.05
    assert step_scale(2.98, 1) == SCALE_MAX
    assert step_scale(0.52, -1) == SCALE_MIN


def test_compositor_settings_are_optional_in_state_dicts() -> None:
    output = load_outputs()[0]
    raw = output.state().to_dict()
    assert "hdr" not in raw and "bit_depth" not in raw and "color_profile" not in raw

    output.hdr = False
    output.bit_depth = 10
    output.color_profile = "icc:/x.icc"
    raw = output.state().to_dict()
    assert raw["hdr"] is False
    assert raw["bit_depth"] == 10
    restored = OutputState.from_dict(output.name, raw)
    assert restored.extras() == {
        "hdr": False,
        "bit_depth": 10,
        "color_profile": "icc:/x.icc",
    }


def test_apply_to_keeps_unmanaged_settings_unless_restoring() -> None:
    output = load_outputs()[0]
    output.hdr = True
    old_profile_state = OutputState.from_dict(output.name, {"scale": 2.0})
    old_profile_state.apply_to(output)
    assert output.scale == 2.0
    assert output.hdr is True
    old_profile_state.apply_to(output, restore=True)
    assert output.hdr is None


def test_identifier_needs_all_edid_fields() -> None:
    hdmi, edp = load_outputs()
    assert hdmi.identifier == "Microstep MSI MP275 PC3M394200857"
    assert edp.identifier == ""


def test_state_values_are_validated_strictly() -> None:
    base = load_outputs()[0].state().to_dict()
    bad_values = [
        {"scale": float("nan")},
        {"scale": float("inf")},
        {"scale": 0},
        {"position": {"x": float("inf"), "y": 0}},
        {"position": {"x": 1.5, "y": 0}},
        {"position": "0,0"},
        {"hdr": "false"},
        {"enabled": "yes"},
        {"adaptive_sync": "off"},
        {"bit_depth": True},
        {"color_profile": 8},
        {"mode": {"width": 1920, "height": 1080, "refresh": float("nan")}},
        {"mode": {"width": 0, "height": 1080, "refresh": 60}},
    ]
    for override in bad_values:
        with pytest.raises((TypeError, ValueError)):
            OutputState.from_dict("HDMI-A-1", {**base, **override})

    # JSON integers 0/1 are fine for flags, and virtual outputs may have a
    # refresh rate of 0.
    state = OutputState.from_dict(
        "HEADLESS-1",
        {
            **base,
            "hdr": 1,
            "enabled": 1,
            "mode": {"width": 1920, "height": 1080, "refresh": 0},
        },
    )
    assert state.hdr is True and state.enabled is True


def test_match_state_never_guesses_between_twins() -> None:
    first, second = load_outputs()
    shared = first.hardware_id
    saved = {"DP-1": first.state(), "DP-2": second.state()}
    saved["DP-1"].name, saved["DP-2"].name = "DP-1", "DP-2"
    saved["DP-2"].hardware_id = shared  # identical monitors

    # Unique on both sides: the renamed connector is found by EDID.
    assert match_state({"DP-1": saved["DP-1"]}, "DP-9", shared, [shared]) is not None
    # Two saved twins: no guess.
    assert match_state(saved, "DP-9", shared, [shared]) is None
    # Two connected twins: no guess either.
    assert (
        match_state({"DP-1": saved["DP-1"]}, "DP-9", shared, [shared, shared]) is None
    )
    # The connector name always wins.
    assert match_state(saved, "DP-2", shared, [shared, shared]) is saved["DP-2"]


def test_disables_everything() -> None:
    state = load_outputs()[0].state()
    state.enabled = False
    assert disables_everything({"HDMI-A-1": True}, [state])
    assert not disables_everything({"HDMI-A-1": True, "eDP-1": True}, [state])
    other = load_outputs()[1].state()
    assert not disables_everything({"HDMI-A-1": True, "eDP-1": False}, [state, other])
