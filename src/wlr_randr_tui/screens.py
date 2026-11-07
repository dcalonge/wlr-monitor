"""Modal screens used by the TUI.

Dialogs are drawn like impala / bluetui popups: centered, a heavy green
border with the title in it, and a short key hint at the bottom.
"""

from __future__ import annotations

import os
from collections.abc import Callable
from typing import ClassVar

from rich.text import Text
from textual import on
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical, VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Input, OptionList, Static
from textual.widgets.option_list import Option

from .compositor import Capabilities, color_profile_label
from .model import TRANSFORMS, Output, OutputState
from .profiles import ProfileError, ProfileStore

HELP_TEXT = """\
[b]Move[/b]
  ←↓↑→ / hjkl            move by the grid size
  Shift + arrows         move by 10× the grid size
  g                      cycle grid size (1, 8, 16, 32, 64 px)
  L                      cycle snap mode (Off, Edges, Centers, Both)

[b]Display[/b]
  Tab / Shift+Tab        select next / previous output
  r                      pick scale (0.50x – 3.00x, custom)
  [ / ]                  decrease / increase scale by 0.05
  f                      pick resolution and refresh rate
  t                      pick rotation
  v                      toggle adaptive sync (VRR)
  c / d                  advanced settings (HDR, color depth, color profile)
  Space / Enter          enable / disable the selected output

[b]Layout[/b]
  a                      apply pending changes
  Ctrl+R                 reload outputs, discarding pending changes
  z                      reset pending changes to the last applied state
  Z                      revert to the layout before the last apply
  S                      save the current layout as the startup profile

[b]Profiles[/b]
  p                      save the current layout as a named profile
  o                      open the profile browser
  Enter (in browser)     apply a profile
  r (in browser)         rename a profile
  d (in browser)         delete a profile
  Shift+↑ / Shift+↓      reorder profiles

[b]Mouse[/b]
  left click / drag      select and move an output
  right click            enable / disable an output
  scroll wheel           adjust scale by 0.05

[b]Other[/b]
  ?                      show this help
  q / Ctrl+C             quit
"""


class DialogStatic(Static):
    """A focusable Static used to trap keys inside a modal dialog."""

    can_focus = True


def _dialog(title: str, classes: str = "") -> Vertical:
    container = Vertical(classes=f"dialog {classes}".strip())
    container.border_title = title
    return container


class ChoiceScreen(ModalScreen[str | None]):
    """A generic option picker."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape,q", "cancel", "Cancel", show=False)
    ]

    def __init__(
        self,
        title: str,
        choices: list[tuple[str, str]],
        *,
        selected: int | None = None,
        subtitle: str = "",
    ) -> None:
        super().__init__()
        self.dialog_title = title
        self.subtitle = subtitle
        self.choices = choices
        self.selected = selected

    def compose(self) -> ComposeResult:
        with _dialog(self.dialog_title, "choice-dialog"):
            if self.subtitle:
                yield Static(self.subtitle, classes="dialog-subtitle", markup=False)
            yield OptionList(
                *[Option(label, id=option_id) for option_id, label in self.choices],
                id="choice-list",
            )
            yield Static("[b]↵[/b] Select | [b]Esc[/b] Cancel", classes="dialog-help")

    def on_mount(self) -> None:
        option_list = self.query_one(OptionList)
        option_list.focus()
        if self.selected is not None and 0 <= self.selected < len(self.choices):
            option_list.highlighted = self.selected

    @on(OptionList.OptionSelected)
    def option_selected(self, event: OptionList.OptionSelected) -> None:
        self.dismiss(event.option_id)

    def action_cancel(self) -> None:
        self.dismiss(None)


class TextPrompt(ModalScreen[str | None]):
    """A one-line text prompt."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape", "cancel", "Cancel", show=False)
    ]

    def __init__(self, title: str, *, placeholder: str = "", value: str = "") -> None:
        super().__init__()
        self.dialog_title = title
        self.placeholder = placeholder
        self.value = value

    def compose(self) -> ComposeResult:
        with _dialog(self.dialog_title, "prompt-dialog"):
            yield Input(
                value=self.value, placeholder=self.placeholder, id="prompt-input"
            )
            yield Static("[b]↵[/b] Confirm | [b]Esc[/b] Cancel", classes="dialog-help")

    def on_mount(self) -> None:
        prompt = self.query_one(Input)
        prompt.focus()
        prompt.cursor_position = len(prompt.value)

    def on_input_submitted(self, event: Input.Submitted) -> None:
        value = event.value.strip()
        self.dismiss(value or None)

    def action_cancel(self) -> None:
        self.dismiss(None)


class ConfirmScreen(ModalScreen[bool]):
    """A small yes/no confirmation dialog."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("y", "yes", "Yes", show=False),
        Binding("Y", "yes", show=False),
        Binding("n,N,escape", "no", "No", show=False),
        Binding("enter", "yes", show=False),
    ]

    def __init__(self, title: str, message: str) -> None:
        super().__init__()
        self.dialog_title = title
        self.message = message

    def compose(self) -> ComposeResult:
        with _dialog(self.dialog_title, "prompt-dialog"):
            yield DialogStatic(self.message, markup=False)
            yield Static(
                "[b]y[/b] Yes | [b]n[/b] No | [b]Esc[/b] Cancel",
                classes="dialog-help",
            )

    def on_mount(self) -> None:
        self.query_one(DialogStatic).focus()

    def action_yes(self) -> None:
        self.dismiss(True)

    def action_no(self) -> None:
        self.dismiss(False)


class AdvancedSettingsScreen(ModalScreen[bool]):
    """Rotation, VRR, on/off and the compositor-specific color settings.

    HDR, color depth and color profiles are only offered when the running
    compositor integration supports them (see ``compositor.Capabilities``).
    """

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape", "cancel", "Cancel", show=False),
        Binding("enter", "apply", "Apply", show=False),
        Binding("up,k,shift+tab", "focus_previous", show=False),
        Binding("down,j,tab", "focus_next", show=False),
        Binding("left,h", "adjust(-1)", show=False),
        Binding("right,l", "adjust(1)", show=False),
        Binding("space", "toggle", show=False),
        Binding("t", "cycle_transform", show=False),
        Binding("v", "toggle_vrr", show=False),
        Binding("i", "pick_icc", show=False),
    ]

    def __init__(
        self,
        output: Output,
        capabilities: Capabilities | None = None,
        *,
        compositor_label: str = "wlroots",
        color_ok: Callable[[str], bool] | None = None,
        can_disable: bool = True,
    ) -> None:
        super().__init__()
        self.output = output
        self.capabilities = capabilities or Capabilities()
        self.compositor_label = compositor_label
        self.color_ok = color_ok or (lambda value: True)
        # False when this is the only enabled output (same rule as Space).
        self.can_disable = can_disable
        fields = ["transform", "vrr", "enabled"]
        if self.capabilities.hdr:
            fields.append("hdr")
        if self.capabilities.bit_depths:
            fields.append("bit_depth")
        if self.capabilities.color_profile:
            fields.append("color")
        self.fields = tuple(fields)
        self.focus_index = 0
        self._snapshot = (
            output.transform,
            output.adaptive_sync,
            output.enabled,
            output.hdr,
            output.bit_depth,
            output.color_profile,
        )

    def compose(self) -> ComposeResult:
        with _dialog(f"Advanced · {self.output.name}", "advanced-dialog"):
            yield DialogStatic("", id="advanced-body")
            hint = (
                "[b]↑↓[/b] Nav | [b]←→[/b] Adjust | [b]Space[/b] Toggle | "
                "[b]↵[/b] Done | [b]Esc[/b] Cancel"
            )
            if self.capabilities.icc:
                hint += " | [b]i[/b] ICC file"
            yield Static(hint, classes="dialog-help")

    def on_mount(self) -> None:
        body = self.query_one("#advanced-body", DialogStatic)
        body.focus()
        self._refresh()

    def _value(self, field: str) -> tuple[str, str]:
        output = self.output
        if field == "transform":
            return "Rotation", output.transform
        if field == "vrr":
            return "VRR", "On" if output.adaptive_sync else "Off"
        if field == "enabled":
            return "Output", "On" if output.enabled else "Off"
        if field == "hdr":
            if output.hdr is None:
                return "HDR", "default"
            return "HDR", "On" if output.hdr else "Off"
        if field == "bit_depth":
            depth = output.bit_depth
            return "Color depth", f"{depth} bit" if depth else "default"
        return "Color profile", color_profile_label(output.color_profile)

    def _refresh(self) -> None:
        text = Text()
        if self.output.title and self.output.title != self.output.name:
            text.append(f"{self.output.title}\n\n", style="dim")

        for index, field in enumerate(self.fields):
            focused = index == self.focus_index
            label, value = self._value(field)
            marker = "▸ " if focused else "  "
            line = Text(no_wrap=True, overflow="ellipsis")
            line.append(marker, style="bold")
            line.append(f"{label:<16}", style="bold yellow" if focused else "bold")
            line.append(value, style="" if focused else "dim")
            if focused:
                line.stylize("on bright_black")
            text.append_text(line)
            text.append("\n")

        missing = []
        caps = self.capabilities
        if not caps.hdr:
            missing.append("HDR")
        if not caps.bit_depths:
            missing.append("color depth")
        if not caps.color_profile:
            missing.append("color profile")
        if missing:
            text.append("\n")
            text.append(
                f"Not supported by {self.compositor_label}: {', '.join(missing)}.\n",
                style="dim italic",
            )
        text.append("\n")
        text.append("Changes stay pending until you press ", style="dim")
        text.append("a", style="bold")
        text.append(" in the main view.", style="dim")
        self.query_one("#advanced-body", DialogStatic).update(text)

    def action_focus_next(self) -> None:
        self.focus_index = (self.focus_index + 1) % len(self.fields)
        self._refresh()

    def action_focus_previous(self) -> None:
        self.focus_index = (self.focus_index - 1) % len(self.fields)
        self._refresh()

    def _cycle(self, options: list, current: object, delta: int) -> object:
        index = options.index(current) if current in options else 0
        return options[(index + delta) % len(options)]

    def action_adjust(self, delta: int) -> None:
        field = self.fields[self.focus_index]
        output = self.output
        if field == "transform":
            output.transform = str(
                self._cycle(list(TRANSFORMS), output.transform, delta)
            )
        elif field == "vrr":
            output.adaptive_sync = not output.adaptive_sync
        elif field == "enabled":
            if output.enabled and not self.can_disable:
                self.notify(
                    "Cannot disable the last active monitor.",
                    title="Warning",
                    severity="warning",
                )
                return
            output.enabled = not output.enabled
        elif field == "hdr":
            output.hdr = not output.hdr
        elif field == "bit_depth":
            depths: list[int | None] = [None, *self.capabilities.bit_depths]
            output.bit_depth = self._cycle(depths, output.bit_depth, delta)  # type: ignore[assignment]
        else:
            profiles: list[str | None] = [None, *self.capabilities.color_presets]
            if output.color_profile and output.color_profile not in profiles:
                profiles.append(output.color_profile)
            output.color_profile = self._cycle(profiles, output.color_profile, delta)  # type: ignore[assignment]
        self._refresh()

    def action_toggle(self) -> None:
        field = self.fields[self.focus_index]
        if (
            field == "color"
            and self.capabilities.icc
            and not self.capabilities.color_presets
        ):
            self.action_pick_icc()
            return
        self.action_adjust(1)

    def action_cycle_transform(self) -> None:
        self.output.transform = str(
            self._cycle(list(TRANSFORMS), self.output.transform, 1)
        )
        self._refresh()

    def action_toggle_vrr(self) -> None:
        self.output.adaptive_sync = not self.output.adaptive_sync
        self._refresh()

    def action_pick_icc(self) -> None:
        if not self.capabilities.icc:
            return
        current = self.output.color_profile or ""
        value = current[4:] if current.startswith("icc:") else ""

        def done(path: str | None) -> None:
            if path is None:
                return
            # Compositors open the path verbatim, so store it absolute.
            path = os.path.expanduser(path)
            if not os.path.isabs(path):
                self.notify(
                    "Use an absolute path (or ~/…) for the ICC profile.",
                    title="Error",
                    severity="error",
                )
                return
            candidate = f"icc:{path}"
            if not self.color_ok(candidate):
                self.notify(
                    f"{self.compositor_label} cannot use that ICC path.",
                    title="Error",
                    severity="error",
                )
                return
            if not os.path.isfile(path):
                self.notify(
                    f"{path} does not exist on this machine.",
                    title="Warning",
                    severity="warning",
                )
            self.output.color_profile = candidate
            if "color" in self.fields:
                self.focus_index = self.fields.index("color")
            self._refresh()

        self.app.push_screen(
            TextPrompt(
                "ICC profile",
                placeholder="/usr/share/color/icc/display.icc",
                value=value,
            ),
            done,
        )

    def action_apply(self) -> None:
        self.dismiss(True)

    def action_cancel(self) -> None:
        (
            self.output.transform,
            self.output.adaptive_sync,
            self.output.enabled,
            self.output.hdr,
            self.output.bit_depth,
            self.output.color_profile,
        ) = self._snapshot
        self.dismiss(False)


class ProfileScreen(ModalScreen[str | None]):
    """Browse, apply, save, rename, reorder and delete named profiles."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape,q", "cancel", "Cancel", show=False),
        Binding("s", "save", "Save", show=False),
        Binding("d", "delete", "Delete", show=False),
        Binding("r", "rename", "Rename", show=False),
        Binding("l", "launch", show=False),
        Binding("shift+up", "move_up", show=False),
        Binding("shift+down", "move_down", show=False),
        Binding("k", "cursor_up", show=False),
        Binding("j", "cursor_down", show=False),
    ]

    def __init__(
        self,
        store: ProfileStore,
        *,
        current_states: list[OutputState] | None = None,
        initial_highlight: str | None = None,
        standalone: bool = False,
        allow_save: bool = True,
    ) -> None:
        super().__init__()
        self.store = store
        self.current_states = current_states or []
        self.initial_highlight = initial_highlight
        self.standalone = standalone
        self.allow_save = allow_save
        self._profiles = {}
        self._confirm_delete: str | None = None
        # The last failed action, shown above the file's own problems until
        # the next action succeeds (reloading the list must not hide it).
        self._action_error: str | None = None
        self._problems: list[str] = []

    def compose(self) -> ComposeResult:
        with _dialog("Profiles", "profile-dialog"):
            yield Static("", id="profile-error", classes="dialog-error")
            yield OptionList(id="profile-list")
            yield Static("", id="profile-help", classes="dialog-help")

    def on_mount(self) -> None:
        self._reload()
        self.query_one(OptionList).focus()

    def _reload(self) -> None:
        help_text = self.query_one("#profile-help", Static)
        option_list = self.query_one(OptionList)
        option_list.clear_options()
        self._profiles = {}
        active_name = None
        names: list[str] = []
        try:
            self._profiles = self.store.load()
            active_name = self.store.find_active(self.current_states)
            names = self.store.names()
            # Broken profiles are skipped, but say why they are missing.
            self._problems = self.store.problems()
        except ProfileError as exc:
            self._problems = [str(exc)]
        self._show_messages()

        for name in names:
            profile = self._profiles.get(name)
            if profile is None:
                continue
            count = len(profile.outputs)
            suffix = "output" if count == 1 else "outputs"
            marker = " *" if name == active_name else ""
            option_list.add_option(
                Option(f"{name}{marker}  ({count} {suffix})", id=name)
            )
        if self.allow_save:
            option_list.add_option(Option("Save current layout…", id="__save__"))
        if self.standalone:
            option_list.add_option(Option("[ Open monitor editor ]", id="__launch__"))

        if self._confirm_delete:
            help_text.update(
                f"Delete profile [b]{self._confirm_delete}[/b]?  "
                "[b]y[/b] Yes | [b]n[/b] No"
            )
        elif self.standalone:
            help_text.update(
                "[b]↵[/b] Apply & exit | [b]r[/b] Rename | [b]d[/b] Delete | "
                "[b]⇧↑↓[/b] Reorder | [b]l[/b] Editor | [b]Esc[/b] Quit"
            )
        else:
            help_text.update(
                "[b]↵[/b] Apply | [b]s[/b] Save | [b]r[/b] Rename | [b]d[/b] Delete | "
                "[b]⇧↑↓[/b] Reorder | [b]Esc[/b] Close"
            )

        highlight = 0
        if self.initial_highlight:
            for index, name in enumerate(names):
                if name == self.initial_highlight:
                    highlight = index
                    break
            self.initial_highlight = None
        if self._confirm_delete:
            for index in range(option_list.option_count):
                option = option_list.get_option_at_index(index)
                if option.id == self._confirm_delete:
                    highlight = index
                    break
        option_list.highlighted = highlight

    @on(OptionList.OptionSelected)
    def option_selected(self, event: OptionList.OptionSelected) -> None:
        self.dismiss(event.option_id)

    def _selected_name(self) -> str | None:
        option_list = self.query_one(OptionList)
        index = option_list.highlighted
        if index is None:
            return None
        option = option_list.get_option_at_index(index)
        name = option.id
        if not name or name in ("__save__", "__launch__"):
            return None
        return name

    def _show_messages(self) -> None:
        messages = [self._action_error] if self._action_error else []
        messages.extend(self._problems)
        error = self.query_one("#profile-error", Static)
        error.update("\n".join(messages))
        error.display = bool(messages)

    def _store_call(self, call: Callable[[], object]) -> object | None:
        """Run a store change; on failure keep the error visible."""
        try:
            result = call()
        except ProfileError as exc:
            self._action_error = str(exc)
            self._show_messages()
            return None
        self._action_error = None
        return result

    def on_key(self, event) -> None:  # type: ignore[no-untyped-def]
        if self._confirm_delete is None:
            return
        key = event.key
        if key in ("y", "Y"):
            name = self._confirm_delete
            self._confirm_delete = None
            if self._store_call(lambda: self.store.delete(name)):
                self.notify(
                    f"Deleted profile {name!r}.",
                    title="Info",
                    severity="information",
                )
            self._reload()
            self.query_one(OptionList).focus()
        elif key in ("n", "N", "escape"):
            self._confirm_delete = None
            self._reload()
            self.query_one(OptionList).focus()
        event.stop()

    def action_save(self) -> None:
        if not self.allow_save:
            return
        self.dismiss("__save__")

    def action_launch(self) -> None:
        if self.standalone:
            self.dismiss("__launch__")

    def action_delete(self) -> None:
        name = self._selected_name()
        if name is None:
            return
        self._confirm_delete = name
        self._reload()

    def action_rename(self) -> None:
        name = self._selected_name()
        if name is None:
            return

        def done(value: str | None) -> None:
            if value is None or value == name:
                return
            if self._store_call(lambda: self.store.rename(name, value)):
                self.initial_highlight = value
                self._reload()
            self.query_one(OptionList).focus()

        self.app.push_screen(
            TextPrompt("Rename profile", placeholder="new name", value=name),
            done,
        )

    def _move(self, offset: int) -> None:
        name = self._selected_name()
        if name is None:
            return
        if self._store_call(lambda: self.store.move(name, offset)):
            self.initial_highlight = name
            self._reload()
            self.query_one(OptionList).focus()

    def action_move_up(self) -> None:
        self._move(-1)

    def action_move_down(self) -> None:
        self._move(1)

    def action_cursor_up(self) -> None:
        option_list = self.query_one(OptionList)
        if option_list.highlighted is not None and option_list.highlighted > 0:
            option_list.highlighted -= 1

    def action_cursor_down(self) -> None:
        option_list = self.query_one(OptionList)
        if (
            option_list.highlighted is not None
            and option_list.highlighted < option_list.option_count - 1
        ):
            option_list.highlighted += 1

    def action_cancel(self) -> None:
        if self._confirm_delete is not None:
            self._confirm_delete = None
            self._reload()
            return
        self.dismiss(None)


class HelpScreen(ModalScreen[None]):
    """Scrollable keyboard reference."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape,q,question_mark", "close", "Close", show=False),
        Binding("up,k", "scroll_up", show=False),
        Binding("down,j", "scroll_down", show=False),
        Binding("pageup", "page_up", show=False),
        Binding("pagedown", "page_down", show=False),
        Binding("home", "scroll_home", show=False),
        Binding("end", "scroll_end", show=False),
    ]

    def __init__(self, extra: str = "") -> None:
        super().__init__()
        self.extra = extra

    def compose(self) -> ComposeResult:
        scroll = VerticalScroll(classes="dialog help-dialog")
        scroll.border_title = "Keys"
        with scroll:
            yield Static(HELP_TEXT, markup=True)
            if self.extra:
                yield Static(self.extra, markup=False, classes="help-extra")
            yield Static(
                "[b]↑↓[/b] Scroll | [b]PgUp/PgDn[/b] Page | [b]Esc[/b] Close",
                classes="dialog-help",
            )

    def on_mount(self) -> None:
        self.query_one(VerticalScroll).focus()

    def action_close(self) -> None:
        self.dismiss()

    def action_scroll_up(self) -> None:
        self.query_one(VerticalScroll).scroll_up()

    def action_scroll_down(self) -> None:
        self.query_one(VerticalScroll).scroll_down()

    def action_page_up(self) -> None:
        self.query_one(VerticalScroll).scroll_page_up()

    def action_page_down(self) -> None:
        self.query_one(VerticalScroll).scroll_page_down()

    def action_scroll_home(self) -> None:
        self.query_one(VerticalScroll).scroll_home()

    def action_scroll_end(self) -> None:
        self.query_one(VerticalScroll).scroll_end()
