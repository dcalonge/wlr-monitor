"""Data model for wlr-randr output information."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

TRANSFORMS = (
    "normal",
    "90",
    "180",
    "270",
    "flipped",
    "flipped-90",
    "flipped-180",
    "flipped-270",
)

ROTATED_TRANSFORMS = {"90", "270", "flipped-90", "flipped-270"}

SCALE_MIN = 0.5
SCALE_MAX = 3.0
SCALE_STEP = 0.05

# The same scale presets hyprmon offers, including its DPI-friendly values.
COMMON_SCALES = (
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


def clamp_scale(value: float) -> float:
    """Clamp a scale factor to the range wlr-randr handles well."""
    return round(min(SCALE_MAX, max(SCALE_MIN, value)), 2)


def step_scale(value: float, steps: int = 1) -> float:
    """Move a scale factor by ``steps`` increments of 0.05."""
    return clamp_scale(value + SCALE_STEP * steps)


def format_refresh(value: float) -> str:
    """Format a refresh rate for display."""
    if abs(value - round(value)) < 0.005:
        return f"{value:.0f}"
    return f"{value:.2f}".rstrip("0").rstrip(".")


def refresh_arg(value: float) -> str:
    """Format a refresh rate for the wlr-randr command line."""
    text = f"{value:.6f}".rstrip("0").rstrip(".")
    return text or "0"


@dataclass(frozen=True)
class Mode:
    """A display mode reported by wlr-randr."""

    width: int
    height: int
    refresh: float
    preferred: bool = False
    current: bool = False

    @property
    def size_text(self) -> str:
        return f"{self.width}x{self.height}"

    @property
    def label(self) -> str:
        return f"{self.size_text} @ {format_refresh(self.refresh)} Hz"

    def cli_value(self) -> str:
        return f"{self.size_text}@{refresh_arg(self.refresh)}"

    def key(self) -> tuple[int, int, float]:
        """Comparable key that ignores the current/preferred flags."""
        return (self.width, self.height, round(self.refresh, 3))

    def to_dict(self) -> dict[str, Any]:
        return {
            "width": self.width,
            "height": self.height,
            "refresh": round(self.refresh, 6),
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Mode:
        return cls(
            width=int(data["width"]),
            height=int(data["height"]),
            refresh=float(data["refresh"]),
        )

    @classmethod
    def from_json(cls, data: dict[str, Any]) -> Mode:
        return cls(
            width=int(data["width"]),
            height=int(data["height"]),
            refresh=float(data["refresh"]),
            preferred=bool(data.get("preferred", False)),
            current=bool(data.get("current", False)),
        )


@dataclass
class Output:
    """A wlroots output and its current settings."""

    name: str
    description: str = ""
    make: str = ""
    model: str = ""
    serial: str = ""
    physical_width: int = 0
    physical_height: int = 0
    enabled: bool = True
    modes: list[Mode] = field(default_factory=list)
    mode: Mode | None = None
    x: int = 0
    y: int = 0
    transform: str = "normal"
    scale: float = 1.0
    adaptive_sync: bool = False

    @property
    def title(self) -> str:
        maker_model = " ".join(part for part in (self.make, self.model) if part)
        return maker_model or self.description or self.name

    @property
    def hardware_id(self) -> str:
        """A best-effort stable identity built from EDID fields."""
        return "/".join(part for part in (self.make, self.model, self.serial) if part)

    @property
    def is_rotated(self) -> bool:
        return self.transform in ROTATED_TRANSFORMS

    @property
    def logical_width(self) -> int:
        if self.mode is None:
            return 0
        width = self.mode.width / max(self.scale, 0.01)
        height = self.mode.height / max(self.scale, 0.01)
        if self.is_rotated:
            width, height = height, width
        return max(1, round(width))

    @property
    def logical_height(self) -> int:
        if self.mode is None:
            return 0
        width = self.mode.width / max(self.scale, 0.01)
        height = self.mode.height / max(self.scale, 0.01)
        if self.is_rotated:
            width, height = height, width
        return max(1, round(height))

    @property
    def current_mode(self) -> Mode | None:
        return self.mode

    def state(self) -> OutputState:
        return OutputState(
            name=self.name,
            enabled=self.enabled,
            mode=self.mode,
            x=self.x,
            y=self.y,
            scale=self.scale,
            transform=self.transform,
            adaptive_sync=self.adaptive_sync,
            hardware_id=self.hardware_id,
            description=self.description,
        )

    def mode_options(self) -> list[Mode]:
        """Return unique modes in a stable, useful order."""
        seen: set[tuple[int, int, float]] = set()

        def sort_key(mode: Mode) -> tuple[bool, bool, int, int, float]:
            return (
                not mode.current,
                not mode.preferred,
                -(mode.width * mode.height),
                -mode.refresh,
                mode.width,
            )

        result: list[Mode] = []
        for mode in sorted(self.modes, key=sort_key):
            if mode.key() in seen:
                continue
            seen.add(mode.key())
            result.append(mode)
        return result

    @classmethod
    def from_json(cls, data: dict[str, Any]) -> Output:
        modes = [Mode.from_json(item) for item in data.get("modes", [])]
        mode = next((item for item in modes if item.current), None)
        if mode is None:
            mode = next((item for item in modes if item.preferred), None)
        if mode is None and modes:
            mode = modes[0]

        physical = data.get("physical_size") or {}
        position = data.get("position") or {}
        return cls(
            name=str(data["name"]),
            description=str(data.get("description") or ""),
            make=str(data.get("make") or ""),
            model=str(data.get("model") or ""),
            serial=str(data.get("serial") or ""),
            physical_width=int(physical.get("width") or 0),
            physical_height=int(physical.get("height") or 0),
            enabled=bool(data.get("enabled", True)),
            modes=modes,
            mode=mode,
            x=int(position.get("x") or 0),
            y=int(position.get("y") or 0),
            transform=str(data.get("transform") or "normal"),
            scale=float(data.get("scale") or 1.0),
            adaptive_sync=bool(data.get("adaptive_sync", False)),
        )


@dataclass
class OutputState:
    """The settings that can be applied to an output."""

    name: str
    enabled: bool
    mode: Mode | None
    x: int
    y: int
    scale: float
    transform: str
    adaptive_sync: bool
    hardware_id: str = ""
    description: str = ""

    def to_dict(self) -> dict[str, Any]:
        data: dict[str, Any] = {
            "enabled": self.enabled,
            "mode": self.mode.to_dict() if self.mode is not None else None,
            "position": {"x": self.x, "y": self.y},
            "scale": round(self.scale, 6),
            "transform": self.transform,
            "adaptive_sync": self.adaptive_sync,
        }
        if self.hardware_id:
            data["hardware_id"] = self.hardware_id
        if self.description:
            data["description"] = self.description
        return data

    @classmethod
    def from_dict(cls, name: str, data: dict[str, Any]) -> OutputState:
        mode_data = data.get("mode")
        position = data.get("position") or {}
        return cls(
            name=name,
            enabled=bool(data.get("enabled", True)),
            mode=Mode.from_dict(mode_data) if isinstance(mode_data, dict) else None,
            x=int(position.get("x", data.get("x", 0)) or 0),
            y=int(position.get("y", data.get("y", 0)) or 0),
            scale=float(data.get("scale", 1.0)),
            transform=str(data.get("transform", "normal")),
            adaptive_sync=bool(data.get("adaptive_sync", False)),
            hardware_id=str(data.get("hardware_id") or ""),
            description=str(data.get("description") or ""),
        )

    def comparable(self) -> dict[str, Any]:
        """A normalized representation used to detect changes."""
        return self.to_dict()

    def matches_config(self, other: OutputState) -> bool:
        """Compare everything that affects the on-screen layout."""
        return (
            self.enabled == other.enabled
            and _mode_key(self.mode) == _mode_key(other.mode)
            and (self.x, self.y) == (other.x, other.y)
            and abs(self.scale - other.scale) < 0.0001
            and self.transform == other.transform
            and self.adaptive_sync == other.adaptive_sync
        )

    def apply_to(self, output: Output) -> None:
        """Copy this state onto an output object."""
        output.enabled = self.enabled
        output.x = self.x
        output.y = self.y
        output.scale = self.scale
        output.transform = self.transform
        output.adaptive_sync = self.adaptive_sync
        if self.mode is not None:
            matching = next(
                (item for item in output.modes if item.key() == self.mode.key()),
                None,
            )
            output.mode = matching or self.mode


def _mode_key(mode: Mode | None) -> tuple[int, int, float] | None:
    return mode.key() if mode is not None else None


def common_scales(current: float | None = None) -> list[float]:
    scales = list(COMMON_SCALES)
    if current is not None and all(abs(current - item) > 0.0001 for item in scales):
        scales.append(round(current, 3))
    return sorted(set(scales))
