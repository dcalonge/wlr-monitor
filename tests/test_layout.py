from __future__ import annotations

import json
from pathlib import Path

from wlr_monitor.layout import (
    Guide,
    SnapMode,
    compute_layout,
    snap_position,
)
from wlr_monitor.model import Mode, Output

FIXTURES = Path(__file__).parent / "fixtures"


def load_outputs() -> list[Output]:
    data = json.loads((FIXTURES / "outputs.json").read_text())
    return [Output.from_json(item) for item in data]


def test_single_output_map_is_wide() -> None:
    output = load_outputs()[0]
    geometry = compute_layout([output], 100, 30)
    assert geometry is not None
    rect = geometry.boxes[output.name]
    assert rect.width > rect.height
    assert rect.x >= 0
    assert rect.right <= 100
    assert rect.y >= 0
    assert rect.bottom <= 30


def test_side_by_side_outputs_do_not_overlap() -> None:
    outputs = load_outputs()
    geometry = compute_layout(outputs, 120, 36)
    assert geometry is not None
    first = geometry.boxes["HDMI-A-1"]
    second = geometry.boxes["eDP-1"]
    assert first.right <= second.x + 1
    assert first.y == second.y


def test_disabled_outputs_stay_on_map() -> None:
    output = load_outputs()[0]
    output.enabled = False
    geometry = compute_layout([output], 100, 30)
    assert geometry is not None
    assert output.name in geometry.boxes


def test_world_to_cell_matches_layout_origin() -> None:
    output = load_outputs()[1]
    geometry = compute_layout([output], 100, 30)
    assert geometry is not None
    column, row = geometry.world_to_cell(output.x, output.y)
    assert column >= 0
    assert row >= 0


def test_snap_to_left_edge_and_origin() -> None:
    outputs = load_outputs()
    _first, second = outputs
    x, y, guides = snap_position(second, outputs, 1915, 4, threshold=8)
    assert x == 1920
    assert y == 0
    assert Guide("vertical", 1920) in guides
    assert Guide("horizontal", 0) in guides


def test_snap_to_centered() -> None:
    first, second = load_outputs()
    first.x = 0
    first.y = 0
    second.mode = Mode(width=1000, height=1000, refresh=60.0)
    second.scale = 1.0
    second.x = 460
    second.y = 1080
    x, y, guides = snap_position(second, [first, second], 458, 1084, threshold=8)
    assert x == 460
    assert y == 1080
    assert Guide("vertical", 960) in guides


def test_snap_leaves_far_positions_alone() -> None:
    outputs = load_outputs()
    second = outputs[1]
    x, y, guides = snap_position(second, outputs, 3000, 4000, threshold=8)
    assert (x, y) == (3000, 4000)
    assert guides == []


def test_snap_off_never_moves_or_draws_guides() -> None:
    outputs = load_outputs()
    second = outputs[1]
    x, y, guides = snap_position(
        second, outputs, 1915, 4, mode=SnapMode.OFF, threshold=8
    )
    assert (x, y) == (1915, 4)
    assert guides == []


def test_snap_centers_only() -> None:
    first, second = load_outputs()
    first.x = 0
    first.y = 0
    second.mode = Mode(width=1000, height=1000, refresh=60.0)
    second.scale = 1.0
    # Their centers are 2px apart, but no edges are within the threshold.
    x, y, guides = snap_position(
        second,
        [first, second],
        458,
        2000,
        mode=SnapMode.CENTERS,
        threshold=8,
    )
    assert x == 460
    assert y == 2000
    assert Guide("vertical", 960) in guides


def test_snap_both_combines_edge_and_center_guides() -> None:
    first, second = load_outputs()
    first.x = 0
    first.y = 0
    second.mode = Mode(width=1000, height=1000, refresh=60.0)
    second.scale = 1.0
    x, y, guides = snap_position(
        second,
        [first, second],
        458,
        1076,
        mode=SnapMode.BOTH,
        threshold=8,
    )
    assert x == 460
    assert y == 1080
    assert Guide("vertical", 960) in guides
    assert Guide("horizontal", 1080) in guides


def test_disabled_outputs_are_not_snap_targets() -> None:
    first, second = load_outputs()
    first.enabled = False
    x, y, guides = snap_position(second, [first, second], 5, 5, threshold=10)
    # Only the origin remains as a snap target.
    assert (x, y) == (0, 0)
    assert Guide("vertical", 0) in guides
    assert Guide("horizontal", 0) in guides


def test_snap_mode_labels_and_cycling() -> None:
    assert SnapMode.OFF.label == "Off"
    assert SnapMode.EDGES.label == "Edges"
    assert SnapMode.CENTERS.label == "Centers"
    assert SnapMode.BOTH.label == "Both"
    assert SnapMode.OFF.cycled() is SnapMode.EDGES
    assert SnapMode.BOTH.cycled() is SnapMode.OFF
    assert SnapMode.from_label("centers") is SnapMode.CENTERS
    assert SnapMode.from_label("nonsense") is SnapMode.EDGES
