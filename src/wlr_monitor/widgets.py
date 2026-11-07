"""Textual widgets: the outputs table, layout map, details block and help bar.

The look follows impala / bluetui: stacked bordered sections titled
``Outputs``/``Layout``/…, the active section drawn with a heavy green border,
table headers with a blank line below them, a dark-gray highlight for the
selected row and a centered blue help line with bold keys. Only ANSI colors
are used, so everything follows the terminal palette (foot, kitty, …).
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

from rich.text import Text
from textual import events
from textual.message import Message
from textual.widget import Widget
from textual.widgets import Static

from .compositor import Capabilities, color_profile_label
from .layout import Guide, LayoutGeometry, Rect, compute_layout
from .model import Output, OutputState, format_refresh

HIGHLIGHT = "bold white on bright_black"


class OutputSelected(Message):
    """Sent when the user selects an output."""

    def __init__(self, name: str | None) -> None:
        super().__init__()
        self.name = name


class OutputMoved(Message):
    """Sent while dragging an output in the layout map."""

    def __init__(self, name: str, dx: int, dy: int) -> None:
        super().__init__()
        self.name = name
        self.dx = dx
        self.dy = dy


class DragFinished(Message):
    """Sent when a mouse drag ends."""

    def __init__(self, name: str) -> None:
        super().__init__()
        self.name = name


class OutputToggled(Message):
    """Sent when the user right-clicks an output."""

    def __init__(self, name: str) -> None:
        super().__init__()
        self.name = name


class OutputScaleStep(Message):
    """Sent when the user scrolls over an output."""

    def __init__(self, name: str, steps: int) -> None:
        super().__init__()
        self.name = name
        self.steps = steps


# ----------------------------------------------------------------------
# Outputs table
# ----------------------------------------------------------------------


@dataclass(frozen=True)
class Column:
    header: str
    width: int
    priority: int  # lower survives longer when the terminal is narrow
    value: Callable[[Output], str]


def _mode_text(output: Output) -> str:
    if output.mode is None:
        return "-"
    return f"{output.mode.size_text}@{format_refresh(output.mode.refresh)}"


COLUMNS = (
    Column("Name", 12, 0, lambda output: output.name),
    Column("Mode", 15, 1, _mode_text),
    Column("State", 5, 2, lambda output: "On" if output.enabled else "Off"),
    Column("Scale", 5, 3, lambda output: f"{output.scale:g}"),
    Column("Position", 11, 4, lambda output: f"{output.x},{output.y}"),
    Column("Rotation", 11, 6, lambda output: output.transform),
    Column("VRR", 3, 5, lambda output: "On" if output.adaptive_sync else "Off"),
)
MARKER_WIDTH = 2


def visible_columns(width: int) -> list[Column]:
    """Drop the least important columns until the table fits ``width``."""
    columns = list(COLUMNS)
    while len(columns) > 1:
        needed = (
            MARKER_WIDTH + sum(column.width for column in columns) + len(columns) - 1
        )
        if needed <= width:
            break
        columns.remove(max(columns, key=lambda column: column.priority))
    return columns


def column_spacing(width: int, columns: list[Column]) -> tuple[int, int]:
    """(leading space, gap) that spreads columns like ratatui's SpaceAround."""
    free = max(0, width - MARKER_WIDTH - sum(column.width for column in columns))
    gap = max(1, free // (len(columns) + 1))
    lead = max(0, (free - gap * (len(columns) - 1)) // 2)
    return lead, gap


def _cell(value: str, width: int) -> str:
    if len(value) > width:
        value = value[: max(0, width - 1)] + "…"
    return value.center(width)


class OutputList(Widget):
    """impala-style table of outputs. Selection is controlled by the app."""

    can_focus = False

    def __init__(self, **kwargs: object) -> None:
        super().__init__(**kwargs)
        self.outputs: list[Output] = []
        self.selected = 0
        self.dirty: set[str] = set()
        self.border_title = "Outputs"

    def set_outputs(
        self, outputs: list[Output], selected: int, dirty: set[str]
    ) -> None:
        self.outputs = outputs
        self.selected = selected
        self.dirty = dirty
        self.refresh(layout=True)

    def get_content_height(self, container, viewport, width: int) -> int:  # type: ignore[no-untyped-def]
        # Header, blank line, then one row per output.
        return 2 + max(1, len(self.outputs))

    def render(self) -> Text:
        width = max(1, self.content_size.width)
        text = Text(no_wrap=True, overflow="crop")
        if not self.outputs:
            text.append("No outputs found".center(width), style="dim")
            return text

        columns = visible_columns(width)
        lead, gap = column_spacing(width, columns)

        text.append(" " * (MARKER_WIDTH + lead))
        for position, column in enumerate(columns):
            if position:
                text.append(" " * gap)
            text.append(_cell(column.header, column.width), style="bold")
        text.append("\n\n")

        for index, output in enumerate(self.outputs):
            selected = index == self.selected
            if output.name in self.dirty:
                marker, marker_style = "●", "yellow"
            elif selected:
                marker, marker_style = "▸", "bold"
            else:
                marker, marker_style = " ", ""
            row = Text(no_wrap=True, overflow="crop")
            row.append(f"{marker} ", style=marker_style)
            row.append(" " * lead)
            for position, column in enumerate(columns):
                if position:
                    row.append(" " * gap)
                style = "dim" if not output.enabled else ""
                if column.header == "State" and not output.enabled:
                    style = "red"
                if column.header == "Name" and output.name in self.dirty:
                    style = "yellow"
                row.append(_cell(column.value(output), column.width), style=style)
            row.pad_right(max(0, width - row.cell_len))
            if selected:
                row.stylize(HIGHLIGHT)
            text.append_text(row)
            if index != len(self.outputs) - 1:
                text.append("\n")
        return text

    def on_click(self, event: events.Click) -> None:
        if not self.outputs:
            return
        index = event.y - self.content_offset.y - 2
        if 0 <= index < len(self.outputs):
            self.post_message(OutputSelected(self.outputs[index].name))
            event.stop()


# ----------------------------------------------------------------------
# Layout map
# ----------------------------------------------------------------------


class LayoutMap(Widget):
    """A proportionally scaled map of the current output arrangement."""

    can_focus = False

    def __init__(self, **kwargs: object) -> None:
        super().__init__(**kwargs)
        self.outputs: list[Output] = []
        self.selected_name: str | None = None
        self.dirty: set[str] = set()
        self.guides: list[Guide] = []
        self._geometry: LayoutGeometry | None = None
        self._draw_order: list[str] = []
        self._drag_name: str | None = None
        self._drag_accum_x = 0.0
        self._drag_accum_y = 0.0
        self.border_title = "Layout"

    def set_outputs(
        self,
        outputs: list[Output],
        selected_name: str | None,
        dirty: set[str],
        guides: list[Guide] | None = None,
    ) -> None:
        self.outputs = outputs
        self.selected_name = selected_name
        self.dirty = dirty
        self.guides = guides or []
        self.refresh(layout=True)

    def _put(
        self,
        chars: list[list[str]],
        styles: list[list[str | None]],
        x: int,
        y: int,
        value: str,
        style: str | None,
    ) -> None:
        height = len(chars)
        width = len(chars[0]) if height else 0
        if not (0 <= y < height):
            return
        for offset, char in enumerate(value):
            px = x + offset
            if 0 <= px < width:
                chars[y][px] = char
                styles[y][px] = style

    def render(self) -> Text:
        width = self.content_size.width
        height = self.content_size.height
        if width <= 0 or height <= 0:
            return Text("")
        if width < 12 or height < 3:
            return Text("Too small for the layout map", style="dim")

        geometry = compute_layout(self.outputs, width, height)
        self._geometry = geometry
        if geometry is None:
            return Text("No outputs with a mode", style="dim")

        chars = [[" " for _ in range(width)] for _ in range(height)]
        styles: list[list[str | None]] = [
            [None for _ in range(width)] for _ in range(height)
        ]

        self._draw_guides(chars, styles, geometry, width, height)

        drawable = [output for output in self.outputs if output.name in geometry.boxes]
        # Draw larger boxes first, the selected output last, so overlaps remain readable.
        drawable.sort(
            key=lambda output: (
                output.name == self.selected_name,
                -output.logical_width * output.logical_height,
            )
        )
        self._draw_order = [output.name for output in drawable]

        for output in drawable:
            rect = geometry.boxes[output.name]
            selected = output.name == self.selected_name
            dirty = output.name in self.dirty
            if not output.enabled and not selected:
                border_style = label_style = detail_style = "dim"
            elif dirty:
                border_style = "bold yellow" if selected else "yellow"
                label_style = "bold yellow"
                detail_style = "yellow"
            elif selected:
                border_style = "bold green"
                label_style = "bold"
                detail_style = ""
            else:
                border_style = ""
                label_style = "bold"
                detail_style = "dim"

            self._draw_box(
                chars,
                styles,
                rect,
                border_style,
                "dim" if not output.enabled else None,
                heavy=selected,
                dotted=not output.enabled,
            )
            self._draw_label(chars, styles, rect, output, label_style, detail_style)

        text = Text()
        for y in range(height):
            run_style = styles[y][0]
            run_start = 0
            for x in range(1, width):
                if styles[y][x] != run_style:
                    text.append("".join(chars[y][run_start:x]), style=run_style)
                    run_start = x
                    run_style = styles[y][x]
            text.append("".join(chars[y][run_start:width]), style=run_style)
            if y != height - 1:
                text.append("\n")
        return text

    def _draw_guides(
        self,
        chars: list[list[str]],
        styles: list[list[str | None]],
        geometry: LayoutGeometry,
        width: int,
        height: int,
    ) -> None:
        for guide in self.guides:
            if guide.orientation == "vertical":
                column, _ = geometry.world_to_cell(guide.value, geometry.min_y)
                if 0 <= column < width:
                    for row in range(height):
                        chars[row][column] = "┊"
                        styles[row][column] = "blue"
            else:
                _, row = geometry.world_to_cell(geometry.min_x, guide.value)
                if 0 <= row < height:
                    for column in range(width):
                        chars[row][column] = "┈"
                        styles[row][column] = "blue"

    def _draw_box(
        self,
        chars: list[list[str]],
        styles: list[list[str | None]],
        rect: Rect,
        border_style: str,
        fill_style: str | None,
        *,
        heavy: bool = False,
        dotted: bool = False,
    ) -> None:
        if rect.width < 2 or rect.height < 2:
            for y in range(rect.y, rect.bottom):
                self._put(chars, styles, rect.x, y, "█", border_style)
            return

        fill = "·" if dotted else " "
        for y in range(rect.y + 1, rect.bottom - 1):
            for x in range(rect.x + 1, rect.right - 1):
                self._put(chars, styles, x, y, fill, fill_style)

        # The same plain/thick pair ratatui uses for unfocused/focused blocks.
        if heavy:
            horizontal, vertical = "━", "┃"
            top_left, top_right = "┏", "┓"
            bottom_left, bottom_right = "┗", "┛"
        else:
            horizontal, vertical = "─", "│"
            top_left, top_right = "┌", "┐"
            bottom_left, bottom_right = "└", "┘"

        for x in range(rect.x + 1, rect.right - 1):
            self._put(chars, styles, x, rect.y, horizontal, border_style)
            self._put(chars, styles, x, rect.bottom - 1, horizontal, border_style)
        for y in range(rect.y + 1, rect.bottom - 1):
            self._put(chars, styles, rect.x, y, vertical, border_style)
            self._put(chars, styles, rect.right - 1, y, vertical, border_style)

        self._put(chars, styles, rect.x, rect.y, top_left, border_style)
        self._put(chars, styles, rect.right - 1, rect.y, top_right, border_style)
        self._put(chars, styles, rect.x, rect.bottom - 1, bottom_left, border_style)
        self._put(
            chars, styles, rect.right - 1, rect.bottom - 1, bottom_right, border_style
        )

    def _draw_label(
        self,
        chars: list[list[str]],
        styles: list[list[str | None]],
        rect: Rect,
        output: Output,
        label_style: str,
        detail_style: str,
    ) -> None:
        interior_width = rect.width - 2
        if interior_width < 3 or rect.height < 3:
            return

        state = "[ON]" if output.enabled else "[OFF]"
        name = f"{output.name} {state}"
        if len(name) > interior_width:
            name = output.name
        if len(name) > interior_width:
            name = name[: max(0, interior_width - 1)] + "…"
        name_x = rect.x + 1 + max(0, (interior_width - len(name)) // 2)
        self._put(chars, styles, name_x, rect.y + 1, name, label_style)

        if rect.height < 4 or interior_width < 8:
            return
        if output.mode is not None:
            detail = f"{output.mode.size_text}@{format_refresh(output.mode.refresh)}Hz"
            if len(detail) > interior_width:
                detail = detail[: max(0, interior_width - 1)] + "…"
            detail_x = rect.x + 1 + max(0, (interior_width - len(detail)) // 2)
            self._put(chars, styles, detail_x, rect.y + 2, detail, detail_style)

        if rect.height < 5 or interior_width < 6:
            return
        badges: list[str] = []
        if output.adaptive_sync:
            badges.append("VRR")
        if output.hdr:
            badges.append("HDR")
        if output.transform != "normal":
            badges.append(
                f"{output.transform}°"
                if output.transform.isdigit()
                else output.transform
            )
        if abs(output.scale - 1.0) > 0.0001:
            badges.append(f"x{output.scale:g}")
        if not badges:
            return
        indicator = " ".join(badges)
        if len(indicator) > interior_width:
            indicator = indicator[: max(0, interior_width - 1)] + "…"
        indicator_x = rect.x + 1 + max(0, (interior_width - len(indicator)) // 2)
        self._put(chars, styles, indicator_x, rect.y + 3, indicator, detail_style)

    def _hit(self, x: int, y: int) -> str | None:
        if self._geometry is None:
            return None
        for name in reversed(self._draw_order):
            rect = self._geometry.boxes.get(name)
            if rect is not None and rect.contains(x, y):
                return name
        return None

    def _local(self, event: events.MouseEvent) -> tuple[int, int]:
        """Translate widget-relative mouse coordinates to content cells."""
        offset = self.content_offset
        return event.x - offset.x, event.y - offset.y

    def on_mouse_down(self, event: events.MouseDown) -> None:
        x, y = self._local(event)
        name = self._hit(x, y)
        if name is None:
            return
        if event.button == 3:
            self.post_message(OutputToggled(name))
            event.stop()
            return
        if event.button != 1:
            return
        self.post_message(OutputSelected(name))
        self._drag_name = name
        self._drag_accum_x = 0.0
        self._drag_accum_y = 0.0
        self.capture_mouse()
        event.stop()

    def on_mouse_move(self, event: events.MouseMove) -> None:
        if self._drag_name is None or self._geometry is None:
            return
        if self._geometry.scale <= 0:
            return
        dx, dy = self._geometry.cell_delta_to_pixels(event.delta_x, event.delta_y)
        self._drag_accum_x += dx
        self._drag_accum_y += dy
        step_x = round(self._drag_accum_x)
        step_y = round(self._drag_accum_y)
        if step_x == 0 and step_y == 0:
            return
        self._drag_accum_x -= step_x
        self._drag_accum_y -= step_y
        self.post_message(OutputMoved(self._drag_name, step_x, step_y))
        event.stop()

    def on_mouse_up(self, event: events.MouseUp) -> None:
        if self._drag_name is None:
            return
        name = self._drag_name
        self._drag_name = None
        self.release_mouse()
        self.post_message(DragFinished(name))
        event.stop()

    def on_mouse_scroll_up(self, event: events.MouseScrollUp) -> None:
        self._scale_with_wheel(event, 1)

    def on_mouse_scroll_down(self, event: events.MouseScrollDown) -> None:
        self._scale_with_wheel(event, -1)

    def _scale_with_wheel(self, event: events.MouseEvent, steps: int) -> None:
        x, y = self._local(event)
        name = self._hit(x, y)
        if name is None:
            name = self.selected_name
        if name is None:
            return
        self.post_message(OutputScaleStep(name, steps))
        event.stop()


# ----------------------------------------------------------------------
# Details block
# ----------------------------------------------------------------------

DETAIL_CELL = 32
DETAIL_LABEL = 15


def _bool_label(value: bool | None) -> str:
    return "On" if value else "Off"


def _optional_label(value: bool | None) -> str:
    """Like _bool_label, but ``None`` (unknown / compositor default) stays visible."""
    if value is None:
        return "default"
    return _bool_label(value)


def _depth_label(value: int | None) -> str:
    return f"{value} bit" if value else "default"


def detail_rows(
    output: Output, capabilities: Capabilities | None = None
) -> list[tuple[str, str]]:
    caps = capabilities or Capabilities()
    mode = output.current_mode
    rows = [
        ("State", "On" if output.enabled else "Off"),
        ("Mode", mode.label if mode is not None else "none"),
        ("Position", f"{output.x},{output.y}"),
        ("Logical", f"{output.logical_width}x{output.logical_height}"),
        ("Scale", f"{output.scale:g}"),
        ("Rotation", output.transform),
        ("VRR", _bool_label(output.adaptive_sync)),
    ]
    if caps.hdr:
        rows.append(("HDR", _optional_label(output.hdr)))
    if caps.bit_depths:
        rows.append(("Color depth", _depth_label(output.bit_depth)))
    if caps.color_profile:
        rows.append(("Color profile", color_profile_label(output.color_profile)))
    rows.append(("Physical", f"{output.physical_width}x{output.physical_height} mm"))
    return rows


class DetailsPanel(Static):
    """Shows the selected output's current and pending settings."""

    def __init__(self, **kwargs: object) -> None:
        super().__init__(**kwargs)
        self.border_title = "Output"
        self._last: tuple[object, ...] | None = None

    def show_output(
        self,
        output: Output | None,
        baseline: dict[str, object] | None,
        *,
        error: str | None = None,
        profile_name: str | None = None,
        capabilities: Capabilities | None = None,
    ) -> None:
        self._last = (output, baseline, error, profile_name, capabilities)
        self._render_details()

    def on_resize(self, event: events.Resize) -> None:
        if self._last is not None:
            self._render_details()

    def _render_details(self) -> None:
        assert self._last is not None
        output, baseline, error, profile_name, capabilities = self._last
        assert output is None or isinstance(output, Output)
        width = max(DETAIL_CELL, self.content_size.width or DETAIL_CELL)
        text = Text()
        if error:
            text.append("Error: ", style="bold red")
            text.append(str(error), style="red")
            if output is None:
                self.update(text)
                return
            text.append("\n")

        if output is None:
            self.border_title = "Output"
            text.append("No outputs detected. ", style="bold yellow")
            text.append(
                "Make sure wlr-randr is installed and this terminal runs inside "
                "a wlroots compositor."
            )
            self.update(text)
            return

        title = output.name
        if output.title and output.title != output.name:
            title = f"{output.name} · {output.title}"
        self.border_title = title

        rows = detail_rows(output, capabilities)  # type: ignore[arg-type]
        if profile_name:
            rows.append(("Profile", f"{profile_name} (active)"))
        per_line = max(1, width // DETAIL_CELL)
        cell = width // per_line
        for start in range(0, len(rows), per_line):
            line = Text(no_wrap=True, overflow="ellipsis")
            for label, value in rows[start : start + per_line]:
                chunk = Text(no_wrap=True, overflow="ellipsis")
                chunk.append(f"{label:<{DETAIL_LABEL}}", style="bold")
                value_style = "green" if label == "Profile" else ""
                chunk.append(value, style=value_style)
                chunk.truncate(cell - 1, overflow="ellipsis", pad=True)
                line.append_text(chunk)
                line.append(" ")
            text.append_text(line)
            text.append("\n")

        pending = _pending_changes(output, baseline)  # type: ignore[arg-type]
        if pending:
            text.append("Pending  ", style="bold yellow")
            text.append(" · ".join(pending), style="yellow")
        else:
            text.append("No pending changes", style="dim")
        self.update(text)


def _pending_changes(output: Output, baseline: dict[str, object] | None) -> list[str]:
    if not baseline:
        return []
    try:
        old = OutputState.from_dict(output.name, baseline)
    except (KeyError, TypeError, ValueError):
        return []
    current = output.state()
    changes: list[str] = []

    if current.enabled != old.enabled:
        changes.append(
            f"state {'on' if old.enabled else 'off'} → {'on' if current.enabled else 'off'}"
        )
    old_mode = old.mode.key() if old.mode is not None else None
    new_mode = current.mode.key() if current.mode is not None else None
    if old_mode != new_mode and current.mode is not None:
        changes.append(
            f"mode {old.mode.label if old.mode else 'none'} → {current.mode.label}"
        )
    if (current.x, current.y) != (old.x, old.y):
        changes.append(f"position {old.x},{old.y} → {current.x},{current.y}")
    if abs(current.scale - old.scale) > 0.0001:
        changes.append(f"scale {old.scale:g} → {current.scale:g}")
    if current.transform != old.transform:
        changes.append(f"rotation {old.transform} → {current.transform}")
    if current.adaptive_sync != old.adaptive_sync:
        changes.append(
            f"VRR {_bool_label(old.adaptive_sync).lower()} → "
            f"{_bool_label(current.adaptive_sync).lower()}"
        )
    if current.hdr != old.hdr:
        changes.append(
            f"HDR {_optional_label(old.hdr).lower()} → "
            f"{_optional_label(current.hdr).lower()}"
        )
    if current.bit_depth != old.bit_depth:
        changes.append(
            f"color depth {_depth_label(old.bit_depth)} → "
            f"{_depth_label(current.bit_depth)}"
        )
    if current.color_profile != old.color_profile:
        changes.append(
            f"color profile {color_profile_label(old.color_profile)} → "
            f"{color_profile_label(current.color_profile)}"
        )
    return changes


# ----------------------------------------------------------------------
# Help bar
# ----------------------------------------------------------------------


@dataclass(frozen=True)
class HelpItem:
    key: str
    label: str
    priority: int = 5  # lower survives longer on narrow terminals


def layout_help(
    items: list[HelpItem], width: int, max_lines: int = 2
) -> list[list[HelpItem]]:
    """Wrap help items into at most ``max_lines`` lines of ``width`` cells.

    Low-priority items are dropped until everything fits, like impala's
    narrow-terminal help line.
    """
    separator = 3  # " | "
    remaining = list(items)
    while True:
        lines: list[list[HelpItem]] = [[]]
        used = 0
        for item in remaining:
            size = len(item.key) + 1 + len(item.label)
            extra = size if not lines[-1] else size + separator
            if lines[-1] and used + extra > width:
                lines.append([item])
                used = size
            else:
                lines[-1].append(item)
                used += extra
        if len(lines) <= max_lines or len(remaining) <= 1:
            return lines[:max_lines]
        remaining.remove(max(remaining, key=lambda item: item.priority))


class HelpBar(Widget):
    """Centered, blue, impala-style key hints."""

    can_focus = False

    def __init__(self, **kwargs: object) -> None:
        super().__init__(**kwargs)
        self.items: list[HelpItem] = []

    def set_items(self, items: list[HelpItem]) -> None:
        self.items = items
        self.refresh(layout=True)

    def get_content_height(self, container, viewport, width: int) -> int:  # type: ignore[no-untyped-def]
        return max(1, len(layout_help(self.items, max(1, width))))

    def render(self) -> Text:
        width = max(1, self.content_size.width)
        text = Text(justify="center", no_wrap=True, overflow="crop")
        for index, line in enumerate(layout_help(self.items, width)):
            if index:
                text.append("\n")
            for position, item in enumerate(line):
                if position:
                    text.append(" | ", style="blue")
                text.append(item.key, style="bold blue")
                text.append(f" {item.label}", style="blue")
        return text
