"""Modal screens used by the TUI."""

from __future__ import annotations

from typing import ClassVar

from rich.text import Text
from textual import on
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical, VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Input, OptionList, Static
from textual.widgets.option_list import Option

from .model import TRANSFORMS, Output, OutputState
from .profiles import ProfileError, ProfileStore

HELP_TEXT = """\
[b]Move[/b]
  arrows / hjkl          move by the grid size
  Shift + arrows         move by 10× the grid size
  g                      cycle grid size (1, 8, 16, 32, 64 px)
  L                      cycle snap mode (Off, Edges, Centers, Both)

[b]Display[/b]
  Tab / Shift+Tab        select next / previous output
  r                      pick scale (0.50x – 3.00x, custom)
  [ / ]                  decrease / increase scale by 0.05
  f                      pick resolution and refresh rate
  t                      pick transform / rotation
  v                      toggle adaptive sync (VRR)
  c / d                  advanced display settings
  m                      mirroring (informational; not supported)
  space / Enter          enable / disable the selected output

[b]Layout[/b]
  a                      apply pending changes with wlr-randr
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

[b]Not available through wlr-randr[/b]
  Mirroring, HDR, colour depth, colour management and SDR
  brightness/saturation are compositor features. wlr-randr does
  not expose them, so this tool cannot change them. Use wlr-randr
  for everything in the wlr-output-management protocol.
"""


class DialogStatic(Static):
    """A focusable Static used to trap keys inside a modal dialog."""

    can_focus = True


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
        with Vertical(classes="dialog"):
            yield Static(self.dialog_title, classes="dialog-title")
            if self.subtitle:
                yield Static(self.subtitle, classes="dialog-subtitle")
            yield OptionList(
                *[Option(label, id=option_id) for option_id, label in self.choices],
                id="choice-list",
            )
            yield Static("Enter select  ·  Esc cancel", classes="dialog-help")

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
        with Vertical(classes="dialog prompt-dialog"):
            yield Static(self.dialog_title, classes="dialog-title")
            yield Input(
                value=self.value, placeholder=self.placeholder, id="prompt-input"
            )
            yield Static("Enter confirm  ·  Esc cancel", classes="dialog-help")

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
        with Vertical(classes="dialog prompt-dialog"):
            yield Static(self.dialog_title, classes="dialog-title")
            yield DialogStatic(self.message, markup=False)
            yield Static("Y confirm  ·  N / Esc cancel", classes="dialog-help")

    def on_mount(self) -> None:
        self.query_one(DialogStatic).focus()

    def action_yes(self) -> None:
        self.dismiss(True)

    def action_no(self) -> None:
        self.dismiss(False)


class AdvancedSettingsScreen(ModalScreen[bool]):
    """Transform / VRR / on-off settings available through wlr-randr.

    hyprmon also exposes colour depth, colour management, HDR and SDR
    controls. Those are Hyprland-only features; wlr-randr has no equivalent,
    so they are shown here as unavailable instead of silently ignored.
    """

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape", "cancel", "Cancel", show=False),
        Binding("enter", "apply", "Apply", show=False),
        Binding("up,k,shift+tab", "focus_previous", show=False),
        Binding("down,j,tab", "focus_next", show=False),
        Binding("left", "adjust(-1)", show=False),
        Binding("right", "adjust(1)", show=False),
        Binding("space", "toggle", show=False),
        Binding("t", "cycle_transform", show=False),
        Binding("v", "toggle_vrr", show=False),
    ]

    FIELDS = ("transform", "vrr", "enabled")

    def __init__(self, output: Output) -> None:
        super().__init__()
        self.output = output
        self.focus_index = 0
        self._snapshot = (
            output.transform,
            output.adaptive_sync,
            output.enabled,
        )

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog advanced-dialog"):
            yield Static("Advanced display settings", classes="dialog-title")
            yield DialogStatic("", id="advanced-body")
            yield Static(
                "Tab/↑↓ navigate  ·  ←→ adjust  ·  Space toggle  ·  Enter apply  ·  Esc cancel",
                classes="dialog-help",
            )

    def on_mount(self) -> None:
        body = self.query_one("#advanced-body", DialogStatic)
        body.focus()
        self._refresh()

    def _refresh(self) -> None:
        text = Text()
        text.append(f"{self.output.name}\n", style="bold")
        if self.output.title and self.output.title != self.output.name:
            text.append(f"{self.output.title}\n", style="dim")
        text.append("\n")

        for index, field in enumerate(self.FIELDS):
            focused = index == self.focus_index
            label_style = "bold yellow" if focused else "dim"
            value_style = "yellow" if focused else "white"
            if field == "transform":
                label = "Transform"
                value = self.output.transform
            elif field == "vrr":
                label = "Adaptive sync"
                value = "On" if self.output.adaptive_sync else "Off"
            else:
                label = "Output"
                value = "On" if self.output.enabled else "Off"
            text.append(f"  {label:<16}", style=label_style)
            text.append(f"{value}\n", style=value_style)

        text.append("\n")
        text.append("  Colour depth       ", style="dim")
        text.append("not exposed by wlr-randr\n", style="dim italic")
        text.append("  Colour mode / HDR  ", style="dim")
        text.append("not exposed by wlr-randr\n", style="dim italic")
        text.append("  SDR brightness     ", style="dim")
        text.append("not exposed by wlr-randr\n", style="dim italic")
        text.append("  SDR saturation     ", style="dim")
        text.append("not exposed by wlr-randr\n", style="dim italic")
        text.append("\n")
        text.append(
            "  Changes stay pending until you press a in the main view.\n",
            style="dim",
        )
        self.query_one("#advanced-body", DialogStatic).update(text)

    def action_focus_next(self) -> None:
        self.focus_index = (self.focus_index + 1) % len(self.FIELDS)
        self._refresh()

    def action_focus_previous(self) -> None:
        self.focus_index = (self.focus_index - 1) % len(self.FIELDS)
        self._refresh()

    def action_adjust(self, delta: int) -> None:
        field = self.FIELDS[self.focus_index]
        if field == "transform":
            index = (
                TRANSFORMS.index(self.output.transform)
                if self.output.transform in TRANSFORMS
                else 0
            )
            self.output.transform = TRANSFORMS[(index + delta) % len(TRANSFORMS)]
        elif field == "vrr":
            self.output.adaptive_sync = not self.output.adaptive_sync
        else:
            self.output.enabled = not self.output.enabled
        self._refresh()

    def action_toggle(self) -> None:
        field = self.FIELDS[self.focus_index]
        if field == "transform":
            self.action_adjust(1)
        elif field == "vrr":
            self.output.adaptive_sync = not self.output.adaptive_sync
            self._refresh()
        else:
            self.output.enabled = not self.output.enabled
            self._refresh()

    def action_cycle_transform(self) -> None:
        self.action_adjust(1)

    def action_toggle_vrr(self) -> None:
        self.output.adaptive_sync = not self.output.adaptive_sync
        self._refresh()

    def action_apply(self) -> None:
        self.dismiss(True)

    def action_cancel(self) -> None:
        transform, adaptive_sync, enabled = self._snapshot
        self.output.transform = transform
        self.output.adaptive_sync = adaptive_sync
        self.output.enabled = enabled
        self.dismiss(False)


class UnsupportedScreen(ModalScreen[None]):
    """Explains a hyprmon feature that wlr-randr cannot provide."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape,q,question_mark,enter,space", "close", "Close", show=False)
    ]

    def __init__(self, title: str, message: str) -> None:
        super().__init__()
        self.dialog_title = title
        self.message = message

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog unsupported-dialog"):
            yield Static(self.dialog_title, classes="dialog-title")
            yield DialogStatic(self.message, markup=False)
            yield Static("Esc / Enter to close", classes="dialog-help")

    def on_mount(self) -> None:
        self.query_one(DialogStatic).focus()

    def action_close(self) -> None:
        self.dismiss()


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

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog"):
            yield Static("Profiles", classes="dialog-title")
            yield Static("", id="profile-error", classes="dialog-error")
            yield OptionList(id="profile-list")
            yield Static("", id="profile-help", classes="dialog-help")

    def on_mount(self) -> None:
        self._reload()
        self.query_one(OptionList).focus()

    def _reload(self) -> None:
        error = self.query_one("#profile-error", Static)
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
        except ProfileError as exc:
            error.update(str(exc))
        else:
            error.update("")

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
                f"Delete profile {self._confirm_delete!r}?  y confirm  ·  n cancel"
            )
        elif self.standalone:
            help_text.update(
                "Enter apply & exit  ·  r rename  ·  d delete  ·  "
                "Shift+↑/↓ reorder  ·  l open editor  ·  Esc quit"
            )
        else:
            help_text.update(
                "Enter apply  ·  s save  ·  r rename  ·  d delete  ·  "
                "Shift+↑/↓ reorder  ·  Esc cancel"
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

    def on_key(self, event) -> None:  # type: ignore[no-untyped-def]
        if self._confirm_delete is None:
            return
        key = event.key
        if key in ("y", "Y"):
            name = self._confirm_delete
            self._confirm_delete = None
            try:
                deleted = self.store.delete(name)
            except ProfileError as exc:
                self.query_one("#profile-error", Static).update(str(exc))
            else:
                if deleted:
                    self.notify(f"Deleted profile {name!r}.", severity="information")
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
            try:
                renamed = self.store.rename(name, value)
            except ProfileError as exc:
                self.query_one("#profile-error", Static).update(str(exc))
                return
            if renamed:
                self.initial_highlight = value
                self._reload()
            self.query_one(OptionList).focus()

        self.app.push_screen(
            TextPrompt("Rename profile", placeholder="new name", value=name),
            done,
        )

    def action_move_up(self) -> None:
        name = self._selected_name()
        if name is None:
            return
        if self.store.move(name, -1):
            self.initial_highlight = name
            self._reload()
            self.query_one(OptionList).focus()

    def action_move_down(self) -> None:
        name = self._selected_name()
        if name is None:
            return
        if self.store.move(name, 1):
            self.initial_highlight = name
            self._reload()
            self.query_one(OptionList).focus()

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

    def compose(self) -> ComposeResult:
        with VerticalScroll(classes="dialog help-dialog"):
            yield Static("wlr-randr-tui keys", classes="dialog-title")
            yield Static(HELP_TEXT, markup=True)
            yield Static(
                "↑↓/jk scroll  ·  PgUp/PgDn page  ·  Home/End  ·  Esc close",
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
