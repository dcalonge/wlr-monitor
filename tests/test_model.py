from __future__ import annotations

import json
from pathlib import Path

from wlr_randr_tui.model import (
    COMMON_SCALES,
    SCALE_MAX,
    SCALE_MIN,
    Mode,
    Output,
    OutputState,
    clamp_scale,
    common_scales,
    format_refresh,
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
