"""Geometry, snapping and alignment-guide helpers for the layout map."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import IntEnum

from .model import Output

# A terminal cell is roughly twice as tall as it is wide. Multiplying the
# horizontal scale by this factor keeps monitor boxes looking like monitors.
CELL_ASPECT = 2.0


class SnapMode(IntEnum):
    """How a moved output should align with its neighbours."""

    OFF = 0
    EDGES = 1
    CENTERS = 2
    BOTH = 3

    @property
    def label(self) -> str:
        return {
            SnapMode.OFF: "Off",
            SnapMode.EDGES: "Edges",
            SnapMode.CENTERS: "Centers",
            SnapMode.BOTH: "Both",
        }[self]

    def cycled(self) -> SnapMode:
        return SnapMode((int(self) + 1) % len(SnapMode))

    @classmethod
    def from_label(cls, label: str) -> SnapMode:
        for mode in cls:
            if mode.label.lower() == label.lower():
                return mode
        return cls.EDGES


@dataclass(frozen=True)
class Guide:
    """An alignment guide expressed in logical output coordinates."""

    orientation: str  # "vertical" or "horizontal"
    value: int


@dataclass(frozen=True)
class Rect:
    x: int
    y: int
    width: int
    height: int

    @property
    def right(self) -> int:
        return self.x + self.width

    @property
    def bottom(self) -> int:
        return self.y + self.height

    @property
    def center_x(self) -> float:
        return self.x + self.width / 2

    @property
    def center_y(self) -> float:
        return self.y + self.height / 2

    def contains(self, px: int, py: int) -> bool:
        return self.x <= px < self.right and self.y <= py < self.bottom


@dataclass(frozen=True)
class LayoutGeometry:
    """Mapping between logical output coordinates and terminal cells."""

    scale: float
    min_x: int
    min_y: int
    offset_x: float
    offset_y: float
    boxes: dict[str, Rect] = field(default_factory=dict)
    cell_aspect: float = CELL_ASPECT

    def cell_delta_to_pixels(self, dx: int, dy: int) -> tuple[float, float]:
        if self.scale <= 0:
            return (0.0, 0.0)
        return (dx / (self.scale * self.cell_aspect), dy / self.scale)

    def world_to_cell(self, x: int, y: int) -> tuple[int, int]:
        """Convert logical pixels to cell coordinates (may be off-screen)."""
        column = round(self.offset_x + (x - self.min_x) * self.cell_aspect * self.scale)
        row = round(self.offset_y + (y - self.min_y) * self.scale)
        return column, row


def compute_layout(
    outputs: list[Output],
    width: int,
    height: int,
    *,
    padding: int = 1,
    cell_aspect: float = CELL_ASPECT,
) -> LayoutGeometry | None:
    """Compute cell rectangles for every output that has a mode.

    Disabled outputs are included so they remain visible (and can be
    re-enabled with a right click), just like hyprmon.
    """
    drawable = [
        output
        for output in outputs
        if output.logical_width > 0 and output.logical_height > 0
    ]
    if not drawable or width <= 0 or height <= 0:
        return None

    min_x = min(output.x for output in drawable)
    min_y = min(output.y for output in drawable)
    max_x = max(output.x + output.logical_width for output in drawable)
    max_y = max(output.y + output.logical_height for output in drawable)
    span_x = max(1, max_x - min_x)
    span_y = max(1, max_y - min_y)

    available_w = max(1, width - 2 * padding)
    available_h = max(1, height - 2 * padding)
    scale = min(
        available_w / (span_x * cell_aspect),
        available_h / span_y,
    )
    if scale <= 0:
        return None

    drawn_w = span_x * cell_aspect * scale
    drawn_h = span_y * scale
    offset_x = padding + max(0.0, (available_w - drawn_w) / 2)
    offset_y = padding + max(0.0, (available_h - drawn_h) / 2)

    boxes: dict[str, Rect] = {}
    for output in drawable:
        rel_x = output.x - min_x
        rel_y = output.y - min_y
        left = round(offset_x + rel_x * cell_aspect * scale)
        top = round(offset_y + rel_y * scale)
        right = round(offset_x + (rel_x + output.logical_width) * cell_aspect * scale)
        bottom = round(offset_y + (rel_y + output.logical_height) * scale)
        left = max(0, min(width - 1, left))
        top = max(0, min(height - 1, top))
        right = max(left + 1, min(width, right))
        bottom = max(top + 1, min(height, bottom))
        boxes[output.name] = Rect(left, top, right - left, bottom - top)

    return LayoutGeometry(
        scale=scale,
        min_x=min_x,
        min_y=min_y,
        offset_x=offset_x,
        offset_y=offset_y,
        boxes=boxes,
        cell_aspect=cell_aspect,
    )


def _add_guide(guides: list[Guide], orientation: str, value: int) -> None:
    guide = Guide(orientation, value)
    if guide not in guides:
        guides.append(guide)


def snap_position(
    output: Output,
    others: list[Output],
    x: int,
    y: int,
    *,
    mode: SnapMode = SnapMode.BOTH,
    threshold: int = 10,
) -> tuple[int, int, list[Guide]]:
    """Snap a proposed position and return the guides that should be shown.

    The algorithm follows hyprmon: edges first, then centres, then the origin.
    """
    if mode == SnapMode.OFF or threshold <= 0:
        return x, y, []

    guides: list[Guide] = []
    new_x, new_y = x, y
    width = output.logical_width
    height = output.logical_height

    for other in others:
        if other is output or not other.enabled:
            continue
        other_width = other.logical_width
        other_height = other.logical_height

        if mode in (SnapMode.EDGES, SnapMode.BOTH):
            if abs(x - (other.x + other_width)) < threshold:
                new_x = other.x + other_width
                _add_guide(guides, "vertical", new_x)
            elif abs(x + width - other.x) < threshold:
                new_x = other.x - width
                _add_guide(guides, "vertical", other.x)
            elif abs(x - other.x) < threshold:
                new_x = other.x
                _add_guide(guides, "vertical", new_x)

            if abs(y - (other.y + other_height)) < threshold:
                new_y = other.y + other_height
                _add_guide(guides, "horizontal", new_y)
            elif abs(y + height - other.y) < threshold:
                new_y = other.y - height
                _add_guide(guides, "horizontal", other.y)
            elif abs(y - other.y) < threshold:
                new_y = other.y
                _add_guide(guides, "horizontal", new_y)

        if mode in (SnapMode.CENTERS, SnapMode.BOTH):
            mon_center_x = x + width // 2
            mon_center_y = y + height // 2
            other_center_x = other.x + other_width // 2
            other_center_y = other.y + other_height // 2
            if abs(mon_center_x - other_center_x) < threshold:
                new_x = other_center_x - width // 2
                _add_guide(guides, "vertical", other_center_x)
            if abs(mon_center_y - other_center_y) < threshold:
                new_y = other_center_y - height // 2
                _add_guide(guides, "horizontal", other_center_y)

    # The origin acts as an additional snap target in every snapping mode.
    if abs(x) < threshold:
        new_x = 0
        _add_guide(guides, "vertical", 0)
    if abs(y) < threshold:
        new_y = 0
        _add_guide(guides, "horizontal", 0)

    return new_x, new_y, guides
