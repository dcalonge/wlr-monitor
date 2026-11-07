"""Data model for wlr-randr output information."""

from __future__ import annotations

import math
from collections.abc import Iterable, Mapping
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


# ----------------------------------------------------------------------
# Strict readers for profile / state JSON (hand-editable, so untrusted)
# ----------------------------------------------------------------------


def _number(
    value: Any, what: str, *, positive: bool = False, non_negative: bool = False
) -> float:
    """A finite float. Rejects NaN/Infinity (Python's json accepts them)."""
    if isinstance(value, bool) or not isinstance(value, int | float | str):
        raise TypeError(f"invalid {what} {value!r}")
    try:
        number = float(value)
    except ValueError:
        raise ValueError(f"invalid {what} {value!r}") from None
    if (
        not math.isfinite(number)
        or (positive and number <= 0)
        or (non_negative and number < 0)
    ):
        raise ValueError(f"invalid {what} {value!r}")
    return number


def _integer(value: Any, what: str, *, positive: bool = False) -> int:
    number = _number(value, what, positive=positive)
    if number != int(number):
        raise ValueError(f"invalid {what} {value!r}")
    return int(number)


def _flag(value: Any, what: str) -> bool:
    """A JSON boolean; 0/1 are accepted, strings such as "false" are not."""
    if isinstance(value, bool):
        return value
    if isinstance(value, int) and value in (0, 1):
        return bool(value)
    raise ValueError(f"invalid {what} {value!r}")


def disables_everything(
    current: dict[str, bool], states: list[OutputState] | tuple[OutputState, ...]
) -> bool:
    """True if applying ``states`` would leave no output enabled.

    ``current`` maps every connected output name to its enabled state now;
    outputs without a state keep that value.
    """
    if not current and not states:
        return False
    result = dict(current)
    result.update({state.name: state.enabled for state in states})
    return not any(result.values())


def match_state(
    saved: Mapping[str, OutputState],
    name: str,
    hardware_id: str,
    connected_hardware: Iterable[str],
) -> OutputState | None:
    """The saved state for an output: by connector name, else by EDID id.

    The EDID fallback (for connectors renamed after a replug) is only used
    when it is unambiguous: exactly one saved state and exactly one connected
    output carry that id. Identical monitors often share an EDID serial, and
    guessing would give both the same settings.
    """
    state = saved.get(name)
    if state is not None or not hardware_id:
        return state
    if sum(1 for item in connected_hardware if item == hardware_id) != 1:
        return None
    candidates = [item for item in saved.values() if item.hardware_id == hardware_id]
    return candidates[0] if len(candidates) == 1 else None


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
            width=_integer(data["width"], "mode width", positive=True),
            height=_integer(data["height"], "mode height", positive=True),
            # Virtual/headless outputs may report a refresh rate of 0.
            refresh=_number(data["refresh"], "refresh rate", non_negative=True),
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
    # Compositor-specific settings. ``None`` means "unknown / not managed",
    # so nothing is sent to the compositor for that setting.
    hdr: bool | None = None
    bit_depth: int | None = None
    color_profile: str | None = None

    @property
    def title(self) -> str:
        maker_model = " ".join(part for part in (self.make, self.model) if part)
        return maker_model or self.description or self.name

    @property
    def hardware_id(self) -> str:
        """A best-effort stable identity built from EDID fields."""
        return "/".join(part for part in (self.make, self.model, self.serial) if part)

    @property
    def identifier(self) -> str:
        """``make model serial`` as used by sway and niri output matching.

        Empty unless all three EDID fields are known, because a partial
        identifier would not match anything in those compositors.
        """
        if self.make and self.model and self.serial:
            return f"{self.make} {self.model} {self.serial}"
        return ""

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
            hdr=self.hdr,
            bit_depth=self.bit_depth,
            color_profile=self.color_profile,
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
    hdr: bool | None = None
    bit_depth: int | None = None
    color_profile: str | None = None

    def extras(self) -> dict[str, Any]:
        """Compositor-specific settings that are set (not ``None``)."""
        data: dict[str, Any] = {}
        if self.hdr is not None:
            data["hdr"] = self.hdr
        if self.bit_depth is not None:
            data["bit_depth"] = self.bit_depth
        if self.color_profile is not None:
            data["color_profile"] = self.color_profile
        return data

    def to_dict(self) -> dict[str, Any]:
        data: dict[str, Any] = {
            "enabled": self.enabled,
            "mode": self.mode.to_dict() if self.mode is not None else None,
            "position": {"x": self.x, "y": self.y},
            "scale": round(self.scale, 6),
            "transform": self.transform,
            "adaptive_sync": self.adaptive_sync,
        }
        data.update(self.extras())
        if self.hardware_id:
            data["hardware_id"] = self.hardware_id
        if self.description:
            data["description"] = self.description
        return data

    @classmethod
    def from_dict(cls, name: str, data: dict[str, Any]) -> OutputState:
        mode_data = data.get("mode")
        position = data.get("position") or {}
        if not isinstance(position, dict):
            raise TypeError(f"invalid position {position!r}")
        hdr = data.get("hdr")
        bit_depth = data.get("bit_depth")
        color_profile = data.get("color_profile")
        transform = data.get("transform", "normal")
        if transform not in TRANSFORMS:
            # Profiles are plain JSON; never pass unknown strings on to the
            # compositor (they end up in commands and config files).
            raise ValueError(f"invalid transform {transform!r}")
        if color_profile is not None and not isinstance(color_profile, str):
            raise TypeError(f"invalid color profile {color_profile!r}")
        return cls(
            name=name,
            enabled=_flag(data.get("enabled", True), "enabled"),
            mode=Mode.from_dict(mode_data) if isinstance(mode_data, dict) else None,
            x=_integer(position.get("x", data.get("x", 0)) or 0, "x position"),
            y=_integer(position.get("y", data.get("y", 0)) or 0, "y position"),
            scale=_number(data.get("scale", 1.0), "scale", positive=True),
            transform=transform,
            adaptive_sync=_flag(data.get("adaptive_sync", False), "adaptive_sync"),
            hardware_id=str(data.get("hardware_id") or ""),
            description=str(data.get("description") or ""),
            hdr=_flag(hdr, "hdr") if hdr is not None else None,
            bit_depth=(
                _integer(bit_depth, "bit depth", positive=True)
                if bit_depth is not None
                else None
            ),
            color_profile=color_profile,
        )

    def matches_config(self, other: OutputState) -> bool:
        """Compare everything that affects the on-screen layout."""
        return (
            self.enabled == other.enabled
            and _mode_key(self.mode) == _mode_key(other.mode)
            and (self.x, self.y) == (other.x, other.y)
            and abs(self.scale - other.scale) < 0.0001
            and self.transform == other.transform
            and self.adaptive_sync == other.adaptive_sync
            and _same_if_known(self.hdr, other.hdr)
            and _same_if_known(self.bit_depth, other.bit_depth)
            and _same_if_known(self.color_profile, other.color_profile)
        )

    def apply_to(self, output: Output, *, restore: bool = False) -> None:
        """Copy this state onto an output object.

        Compositor-specific settings are only copied when this state manages
        them, so older profiles do not reset HDR or color settings. With
        ``restore=True`` they are copied verbatim (used to undo edits).
        """
        output.enabled = self.enabled
        output.x = self.x
        output.y = self.y
        output.scale = self.scale
        output.transform = self.transform
        output.adaptive_sync = self.adaptive_sync
        if restore or self.hdr is not None:
            output.hdr = self.hdr
        if restore or self.bit_depth is not None:
            output.bit_depth = self.bit_depth
        if restore or self.color_profile is not None:
            output.color_profile = self.color_profile
        if self.mode is not None:
            matching = next(
                (item for item in output.modes if item.key() == self.mode.key()),
                None,
            )
            output.mode = matching or self.mode


def _mode_key(mode: Mode | None) -> tuple[int, int, float] | None:
    return mode.key() if mode is not None else None


def _same_if_known(first: object, second: object) -> bool:
    """Compare optional settings, treating ``None`` (unknown) as a wildcard."""
    return first is None or second is None or first == second


def common_scales(current: float | None = None) -> list[float]:
    scales = list(COMMON_SCALES)
    if current is not None and all(abs(current - item) > 0.0001 for item in scales):
        scales.append(round(current, 3))
    return sorted(set(scales))
