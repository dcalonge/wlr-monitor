"""Textual widgets for the monitor list, layout map and details panel."""

from __future__ import annotations

from rich.text import Text
from textual import events
from textual.message import Message
from textual.widget import Widget
from textual.widgets import Static

from .layout import Guide, LayoutGeometry, Rect, compute_layout
from .model import Output, OutputState, format_refresh


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


class OutputList(Widget):
    """Compact list of outputs. Selection is controlled by the app."""

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
        self.refresh()

    def render(self) -> Text:
        text = Text()
        if not self.outputs:
            text.append("No outputs found", style="dim")
            return text

        for index, output in enumerate(self.outputs):
            selected = index == self.selected
            name_style = "bold reverse" if selected else "bold"
            if output.name in self.dirty:
                marker = "●"
                marker_style = "yellow"
            elif selected:
                marker = "▸"
                marker_style = "bold reverse"
            else:
                marker = " "
                marker_style = ""

            text.append(f"{marker} ", style=marker_style)
            text.append(f"{output.name:<12}", style=name_style)
            if output.mode is not None:
                mode = f"{output.mode.size_text}@{format_refresh(output.mode.refresh)}"
            else:
                mode = "no mode"
            text.append(f" {mode:<14}", style="dim")
            text.append(f" {output.scale:g}x", style="dim")
            if not output.enabled:
                text.append(" off", style="dim red")
            if index != len(self.outputs) - 1:
                text.append("\n")
        return text

    def on_click(self, event: events.Click) -> None:
        if not self.outputs:
            return
        index = event.y - self.content_offset.y
        if 0 <= index < len(self.outputs):
            self.post_message(OutputSelected(self.outputs[index].name))
            event.stop()


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
        if width < 12 or height < 4:
            return Text("Terminal too small for the layout map", style="dim")

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
            if selected:
                border_style = "bold yellow"
                label_style = "bold yellow"
                detail_style = "yellow"
                fill_style = None
            elif not output.enabled:
                border_style = "dim"
                label_style = "dim"
                detail_style = "dim"
                fill_style = "dim"
            elif dirty:
                border_style = "bold yellow"
                label_style = "bold yellow"
                detail_style = "yellow"
                fill_style = None
            else:
                border_style = "green"
                label_style = "bold green"
                detail_style = "dim"
                fill_style = None

            self._draw_box(
                chars,
                styles,
                rect,
                border_style,
                fill_style,
                double=selected,
                dotted=not output.enabled,
            )
            self._draw_label(
                chars,
                styles,
                rect,
                output,
                label_style,
                detail_style,
            )

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
                        chars[row][column] = "│"
                        styles[row][column] = "dim"
            else:
                _, row = geometry.world_to_cell(geometry.min_x, guide.value)
                if 0 <= row < height:
                    for column in range(width):
                        chars[row][column] = "─"
                        styles[row][column] = "dim"

    def _draw_box(
        self,
        chars: list[list[str]],
        styles: list[list[str | None]],
        rect: Rect,
        border_style: str,
        fill_style: str | None,
        *,
        double: bool = False,
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

        if double:
            horizontal, vertical = "═", "║"
            top_left, top_right = "╔", "╗"
            bottom_left, bottom_right = "╚", "╝"
        else:
            horizontal, vertical = "─", "│"
            top_left, top_right = "╭", "╮"
            bottom_left, bottom_right = "╰", "╯"

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


class DetailsPanel(Static):
    """Shows the selected output's current and pending settings."""

    def __init__(self, **kwargs: object) -> None:
        super().__init__(**kwargs)
        self.border_title = "Selected output"

    def show_output(
        self,
        output: Output | None,
        baseline: dict[str, object] | None,
        *,
        error: str | None = None,
        profile_name: str | None = None,
    ) -> None:
        text = Text()
        if error:
            text.append("wlr-randr error\n", style="bold red")
            text.append(error, style="red")
            if output is None:
                self.update(text)
                return
            text.append("\n\n")

        if output is None:
            text.append("No outputs detected.\n", style="bold yellow")
            text.append(
                "Make sure wlr-randr is installed and this terminal is running inside a wlroots compositor.",
                style="dim",
            )
            self.update(text)
            return

        text.append(f"{output.name}", style="bold")
        if output.title and output.title != output.name:
            text.append(f"  {output.title}", style="dim")
        text.append("\n")

        mode = output.current_mode
        badges: list[str] = []
        if output.adaptive_sync:
            badges.append("VRR")
        if output.transform != "normal":
            badges.append(output.transform)
        if abs(output.scale - 1.0) > 0.0001:
            badges.append(f"{output.scale:g}x")
        rows = [
            ("State", "on" if output.enabled else "off"),
            ("Mode", mode.label if mode is not None else "none"),
            (
                "Position",
                f"{output.x},{output.y}  (logical {output.logical_width}x{output.logical_height})",
            ),
            ("Scale", f"{output.scale:g}"),
            ("Transform", output.transform),
            ("Adaptive sync", "on" if output.adaptive_sync else "off"),
            ("Physical", f"{output.physical_width}x{output.physical_height} mm"),
            ("Modes", f"{len(output.modes)} available"),
        ]
        for label, value in rows:
            text.append(f"  {label:<14}", style="dim")
            text.append(f"{value}\n")

        if badges:
            text.append(f"  {'Badges':<14}", style="dim")
            text.append(" ".join(badges), style="yellow")
            text.append("\n")

        if profile_name:
            text.append(f"  {'Profile':<14}", style="dim")
            text.append(f"{profile_name} (active)\n", style="green")

        pending = _pending_changes(output, baseline)
        text.append("\n")
        if pending:
            text.append("Pending changes", style="bold yellow")
            text.append("  (a apply · z reset · Z revert live)\n", style="dim")
            for change in pending:
                text.append(f"  • {change}\n", style="yellow")
        else:
            text.append("No pending changes", style="dim")
            text.append("\n")

        hints = "c advanced · [ ] scale · r scale picker · f mode picker"
        if not output.enabled:
            hints = "Output is off — press space to enable. " + hints
        text.append(hints, style="dim")
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
            f"state: {'off' if old.enabled else 'on'} → {'off' if not current.enabled else 'on'}"
        )
    old_mode = old.mode.key() if old.mode is not None else None
    new_mode = current.mode.key() if current.mode is not None else None
    if old_mode != new_mode and current.mode is not None:
        changes.append(
            f"mode: {old.mode.label if old.mode else 'none'} → {current.mode.label}"
        )
    if (current.x, current.y) != (old.x, old.y):
        changes.append(f"position: {old.x},{old.y} → {current.x},{current.y}")
    if abs(current.scale - old.scale) > 0.0001:
        changes.append(f"scale: {old.scale:g} → {current.scale:g}")
    if current.transform != old.transform:
        changes.append(f"transform: {old.transform} → {current.transform}")
    if current.adaptive_sync != old.adaptive_sync:
        changes.append(
            f"adaptive sync: {'on' if old.adaptive_sync else 'off'} → {'on' if current.adaptive_sync else 'off'}"
        )
    return changes
