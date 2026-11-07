"""The wlr-randr-tui Textual application."""

from __future__ import annotations

from typing import Any, ClassVar

from textual import on
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Footer, Header, Static

from .layout import Guide, SnapMode, snap_position
from .model import (
    SCALE_MAX,
    SCALE_MIN,
    TRANSFORMS,
    Output,
    OutputState,
    clamp_scale,
    common_scales,
    step_scale,
)
from .profiles import ProfileError, ProfileStore
from .screens import (
    AdvancedSettingsScreen,
    ChoiceScreen,
    ConfirmScreen,
    HelpScreen,
    ProfileScreen,
    TextPrompt,
    UnsupportedScreen,
)
from .widgets import (
    DetailsPanel,
    DragFinished,
    LayoutMap,
    OutputList,
    OutputMoved,
    OutputScaleStep,
    OutputSelected,
    OutputToggled,
)
from .wlr import ApplyResult, WlrRandr

GRID_SIZES = (1, 8, 16, 32, 64)
DEFAULT_GRID = 32
DEFAULT_SNAP = SnapMode.EDGES
DEFAULT_SNAP_THRESHOLD = 10
STARTUP_PROFILE = "default"

MIRROR_MESSAGE = """\
Mirroring is a compositor feature that the wlr-output-management
protocol does not expose. wlr-randr can only position, scale,
rotate, enable and disable outputs, so this tool cannot mirror
one monitor onto another.

Arrange the outputs side by side instead, or configure mirroring
in your compositor's own configuration.
"""


class MainScreen(Screen[None]):
    """The default screen.

    Tab is Focus Next in Textual's default screen, so override it here to keep
    output cycling working without stealing Tab from modal dialogs.
    """

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("tab", "app.next_output", "Next"),
        Binding("shift+tab", "app.previous_output", "Prev", show=False),
    ]


class WlrRandrApp(App[None]):
    """Interactive monitor layout editor for wlroots compositors."""

    TITLE = "wlr-randr-tui"
    SUB_TITLE = "wlroots monitor layout"

    CSS = """
    Screen {
        layout: vertical;
    }

    #main {
        height: 1fr;
    }

    #sidebar {
        width: 40;
        height: 1fr;
        border: round $primary;
        padding: 0 1;
    }

    #right {
        width: 1fr;
        height: 1fr;
    }

    #map {
        height: 1fr;
        min-height: 6;
        border: round $primary;
    }

    #details {
        height: 13;
        border: round $primary;
        padding: 0 1;
        overflow-y: auto;
    }

    #status {
        height: 1;
        padding: 0 1;
        background: $panel;
        color: $text-muted;
    }

    OutputList {
        height: 1fr;
    }

    LayoutMap {
        height: 1fr;
    }

    ModalScreen {
        align: center middle;
    }

    .dialog {
        width: 72;
        height: 75%;
        border: round $primary;
        background: $surface;
        padding: 1 2;
    }

    .prompt-dialog {
        height: auto;
        min-height: 9;
    }

    .advanced-dialog {
        width: 68;
        height: auto;
        max-height: 90%;
    }

    .unsupported-dialog {
        width: 72;
        height: auto;
        max-height: 80%;
    }

    .help-dialog {
        height: auto;
        max-height: 90%;
        overflow-y: auto;
    }

    .dialog-title {
        text-style: bold;
        color: $accent;
        padding-bottom: 1;
    }

    .dialog-subtitle {
        color: $text-muted;
        padding-bottom: 1;
    }

    .dialog-help {
        color: $text-muted;
        padding-top: 1;
    }

    .dialog-error {
        color: $error;
    }

    #choice-list,
    #profile-list {
        height: 1fr;
    }
    """

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("q,Q,ctrl+c", "quit", "Quit"),
        Binding("question_mark", "help", "Help"),
        Binding("up,k", "move_up", "Move"),
        Binding("down,j", "move_down", show=False),
        Binding("left,h", "move_left", show=False),
        Binding("right,l", "move_right", show=False),
        Binding("shift+up,K", "move_up_fast", show=False),
        Binding("shift+down,J", "move_down_fast", show=False),
        Binding("shift+left,H", "move_left_fast", show=False),
        Binding("shift+right", "move_right_fast", show=False),
        Binding("g,G", "cycle_grid", "Grid"),
        Binding("L", "cycle_snap", "Snap"),
        Binding("r,R", "pick_scale", "Scale"),
        Binding("f,F", "pick_mode", "Mode"),
        Binding("t", "pick_transform", "Transform"),
        Binding("v", "toggle_adaptive", "VRR"),
        Binding("left_square_bracket", "scale_down", show=False),
        Binding("right_square_bracket", "scale_up", show=False),
        Binding("c,C,d,D", "advanced", "Advanced"),
        Binding("m,M", "mirror", "Mirror"),
        Binding("space,enter", "toggle_enabled", "On/off"),
        Binding("a,A", "apply", "Apply"),
        Binding("ctrl+r", "reload", "Reload"),
        Binding("p,P", "save_profile", "Save profile"),
        Binding("S", "save_startup", show=False),
        Binding("o,O", "open_profiles", "Profiles"),
        Binding("z", "reset", show=False),
        Binding("Z", "revert_live", show=False),
    ]

    def __init__(
        self,
        backend: WlrRandr | Any | None = None,
        store: ProfileStore | None = None,
        *,
        initial_outputs: list[Output] | None = None,
        profiles_only: bool = False,
    ) -> None:
        super().__init__()
        self.backend = backend if backend is not None else WlrRandr()
        self.store = store if store is not None else ProfileStore()
        self._initial_outputs = initial_outputs
        self._profiles_only = profiles_only
        self.outputs: list[Output] = []
        self.baseline: dict[str, dict[str, Any]] = {}
        self.selected = 0
        self.grid = DEFAULT_GRID
        self.snap_mode = DEFAULT_SNAP
        self.snap_threshold = DEFAULT_SNAP_THRESHOLD
        self.guides: list[Guide] = []
        self.active_profile: str | None = None
        self.status = "Ready."
        self._error: str | None = None
        self._ui_ready = False
        self._drag_raw: dict[str, tuple[float, float]] = {}
        self._rollback: list[OutputState] | None = None

    def compose(self) -> ComposeResult:
        yield Header()
        with Horizontal(id="main"):
            with Vertical(id="sidebar"):
                yield OutputList(id="outputs")
            with Vertical(id="right"):
                yield LayoutMap(id="map")
                yield DetailsPanel(id="details")
        yield Static("", id="status")
        yield Footer()

    def get_default_screen(self) -> Screen[None]:
        return MainScreen()

    def check_action(self, action: str, parameters: tuple[object, ...]) -> bool | None:
        """Keep global shortcuts from firing while a modal is open."""
        if action == "quit":
            return True
        return isinstance(self.screen, MainScreen)

    def on_mount(self) -> None:
        self._ui_ready = True
        if self._initial_outputs is not None:
            self._set_outputs(self._initial_outputs)
        else:
            self._reload_from_backend()
        if self._profiles_only:
            self.call_after_refresh(self.action_open_profiles)

    @property
    def current(self) -> Output | None:
        if 0 <= self.selected < len(self.outputs):
            return self.outputs[self.selected]
        return None

    @property
    def dirty_names(self) -> set[str]:
        return {
            output.name
            for output in self.outputs
            if self.baseline.get(output.name) != output.state().to_dict()
        }

    def _find(self, name: str) -> Output | None:
        return next((output for output in self.outputs if output.name == name), None)

    def baseline_states(self) -> list[OutputState]:
        """The last applied state of every output, as profile-ready objects."""
        states: list[OutputState] = []
        for output in self.outputs:
            raw = self.baseline.get(output.name)
            if raw is None:
                continue
            try:
                states.append(OutputState.from_dict(output.name, raw))
            except (KeyError, TypeError, ValueError):
                continue
        return states

    def _detect_active_profile(self) -> str | None:
        try:
            return self.store.find_active(self.baseline_states())
        except ProfileError:
            return None

    def _set_outputs(
        self, outputs: list[Output], *, keep_name: str | None = None
    ) -> None:
        self.outputs = outputs
        self.baseline = {output.name: output.state().to_dict() for output in outputs}
        if keep_name is not None:
            for index, output in enumerate(outputs):
                if output.name == keep_name:
                    self.selected = index
                    break
            else:
                self.selected = min(self.selected, max(0, len(outputs) - 1))
        elif self.selected >= len(outputs):
            self.selected = max(0, len(outputs) - 1)
        self.guides = []
        self.active_profile = self._detect_active_profile()
        self._sync()

    def _sync(self) -> None:
        if not self._ui_ready:
            return
        current = self.current
        current_name = current.name if current is not None else None
        dirty = self.dirty_names
        self.query_one(OutputList).set_outputs(self.outputs, self.selected, dirty)
        self.query_one(LayoutMap).set_outputs(
            self.outputs,
            current_name,
            dirty,
            self.guides,
        )
        self.query_one(DetailsPanel).show_output(
            current,
            self.baseline.get(current_name) if current_name is not None else None,
            error=self._error,
            profile_name=self.active_profile,
        )
        self._update_status()

    def _update_status(self) -> None:
        parts = [
            f"grid {self.grid}px",
            f"snap {self.snap_mode.label}",
        ]
        if self.active_profile:
            parts.append(f"profile {self.active_profile}")
        dirty = self.dirty_names
        if dirty:
            parts.append(f"{len(dirty)} pending")
        parts.append(self.status)
        self.query_one("#status", Static).update("  ·  ".join(parts))

    def _select_name(self, name: str) -> None:
        for index, output in enumerate(self.outputs):
            if output.name == name:
                self.selected = index
                self._sync()
                return

    def _clear_guides(self) -> None:
        self.guides = []

    def _notify(self, message: str, severity: str = "information") -> None:
        self.status = message
        self._update_status()
        self.notify(message, severity=severity, timeout=4)

    def _reload_from_backend(self) -> bool:
        current = self.current
        current_name = current.name if current is not None else None
        try:
            outputs = self.backend.read_outputs()
        except Exception as exc:  # noqa: BLE001 - wlr-randr errors are user-facing
            self._error = str(exc)
            self._sync()
            self._notify(str(exc), severity="error")
            return False
        self._error = None
        self._set_outputs(outputs, keep_name=current_name)
        return True

    def _apply_states(
        self,
        states: list[OutputState],
        success_message: str,
        *,
        remember_previous: bool = True,
    ) -> bool:
        if not states:
            self._notify("Nothing to apply.", severity="warning")
            return False
        previous = self.baseline_states() if remember_previous else None
        try:
            results = self.backend.apply_states(states)
        except Exception as exc:  # noqa: BLE001
            self._error = str(exc)
            self._sync()
            self._notify(str(exc), severity="error")
            return False

        failures = [result for result in results if not result.ok]
        if failures:
            self._error = _format_failures(failures)
            self._sync()
            self._notify(self._error, severity="error")
            return False

        self._error = None
        if previous is not None:
            self._rollback = previous
        applied = {state.name for state in states}
        for output in self.outputs:
            if output.name in applied:
                self.baseline[output.name] = output.state().to_dict()
        self.status = success_message
        current = self.current
        current_name = current.name if current is not None else None
        try:
            outputs = self.backend.read_outputs()
        except Exception:  # noqa: BLE001 - keep local state if the refresh fails
            self.active_profile = self._detect_active_profile()
            self._sync()
            self._notify(success_message)
            return True
        self._set_outputs(outputs, keep_name=current_name)
        self._notify(success_message)
        return True

    # ------------------------------------------------------------------
    # Keyboard actions
    # ------------------------------------------------------------------

    def action_quit(self) -> None:
        self.exit()

    def action_help(self) -> None:
        self.push_screen(HelpScreen())

    def action_next_output(self) -> None:
        if not self.outputs:
            return
        self.selected = (self.selected + 1) % len(self.outputs)
        self._clear_guides()
        self._sync()

    def action_previous_output(self) -> None:
        if not self.outputs:
            return
        self.selected = (self.selected - 1) % len(self.outputs)
        self._clear_guides()
        self._sync()

    def _move(self, dx: int, dy: int, multiplier: int = 1) -> None:
        output = self.current
        if output is None:
            return
        new_x = output.x + dx * self.grid * multiplier
        new_y = output.y + dy * self.grid * multiplier
        new_x, new_y, guides = snap_position(
            output,
            self.outputs,
            new_x,
            new_y,
            mode=self.snap_mode,
            threshold=self.snap_threshold,
        )
        self.guides = guides
        if (new_x, new_y) != (output.x, output.y):
            output.x, output.y = new_x, new_y
        self.status = f"{output.name}: {output.x},{output.y}"
        self._sync()

    def action_move_up(self) -> None:
        self._move(0, -1)

    def action_move_down(self) -> None:
        self._move(0, 1)

    def action_move_left(self) -> None:
        self._move(-1, 0)

    def action_move_right(self) -> None:
        self._move(1, 0)

    def action_move_up_fast(self) -> None:
        self._move(0, -1, 10)

    def action_move_down_fast(self) -> None:
        self._move(0, 1, 10)

    def action_move_left_fast(self) -> None:
        self._move(-1, 0, 10)

    def action_move_right_fast(self) -> None:
        self._move(1, 0, 10)

    def action_cycle_grid(self) -> None:
        index = GRID_SIZES.index(self.grid) if self.grid in GRID_SIZES else 0
        self.grid = GRID_SIZES[(index + 1) % len(GRID_SIZES)]
        self.status = f"Grid: {self.grid}px"
        self._update_status()

    def action_cycle_snap(self) -> None:
        self.snap_mode = self.snap_mode.cycled()
        self._clear_guides()
        self.status = (
            f"Snap: {self.snap_mode.label} ({self.snap_threshold}px)"
            if self.snap_mode is not SnapMode.OFF
            else "Snap: Off"
        )
        self._sync()

    def action_pick_mode(self) -> None:
        output = self.current
        if output is None:
            return
        modes = output.mode_options()
        if not modes:
            self._notify("This output reports no modes.", severity="warning")
            return

        current_key = output.mode.key() if output.mode is not None else None
        choices: list[tuple[str, str]] = []
        selected: int | None = None
        for index, mode in enumerate(modes):
            marker = "●" if mode.key() == current_key else " "
            suffix = "  preferred" if mode.preferred else ""
            choices.append((str(index), f"{marker} {mode.label}{suffix}"))
            if mode.key() == current_key:
                selected = index

        def done(choice: str | None) -> None:
            if choice is None:
                return
            output.mode = modes[int(choice)]
            self._clear_guides()
            self.status = f"{output.name}: {output.mode.label}"
            self._sync()

        self.push_screen(
            ChoiceScreen("Resolution and refresh rate", choices, selected=selected),
            done,
        )

    def action_pick_scale(self) -> None:
        output = self.current
        if output is None:
            return
        scales = common_scales(output.scale)
        choices = [(f"{scale:g}", f"{scale:g}x") for scale in scales]
        choices.append(("custom", "Custom value…"))
        selected = next(
            (
                index
                for index, scale in enumerate(scales)
                if abs(scale - output.scale) < 0.0001
            ),
            len(scales),
        )

        def done(choice: str | None) -> None:
            if choice is None:
                return
            if choice == "custom":
                self._prompt_scale(output)
                return
            self._set_scale(output, float(choice))

        self.push_screen(ChoiceScreen("Scale", choices, selected=selected), done)

    def _prompt_scale(self, output: Output) -> None:
        def done(value: str | None) -> None:
            if value is None:
                return
            try:
                scale = float(value.replace(",", "."))
            except ValueError:
                self._notify(f"Invalid scale: {value!r}", severity="error")
                return
            if not SCALE_MIN <= scale <= SCALE_MAX:
                self._notify(
                    f"Scale must be between {SCALE_MIN:g} and {SCALE_MAX:g}.",
                    severity="error",
                )
                return
            self._set_scale(output, clamp_scale(scale))

        self.push_screen(
            TextPrompt(
                f"Scale for {output.name}",
                placeholder="1.25",
                value=f"{output.scale:g}",
            ),
            done,
        )

    def _set_scale(self, output: Output, scale: float) -> None:
        output.scale = clamp_scale(scale)
        self._clear_guides()
        self.status = f"{output.name}: scale {output.scale:g}"
        self._sync()

    def _step_scale(self, steps: int) -> None:
        output = self.current
        if output is None:
            return
        output.scale = step_scale(output.scale, steps)
        self._clear_guides()
        self.status = f"{output.name}: scale {output.scale:g}"
        self._sync()

    def action_scale_down(self) -> None:
        self._step_scale(-1)

    def action_scale_up(self) -> None:
        self._step_scale(1)

    def action_pick_transform(self) -> None:
        output = self.current
        if output is None:
            return
        choices = [(transform, transform) for transform in TRANSFORMS]
        selected = (
            TRANSFORMS.index(output.transform) if output.transform in TRANSFORMS else 0
        )

        def done(choice: str | None) -> None:
            if choice is None:
                return
            output.transform = choice
            self._clear_guides()
            self.status = f"{output.name}: transform {choice}"
            self._sync()

        self.push_screen(ChoiceScreen("Transform", choices, selected=selected), done)

    def action_toggle_adaptive(self) -> None:
        output = self.current
        if output is None:
            return
        output.adaptive_sync = not output.adaptive_sync
        self._clear_guides()
        self.status = (
            f"{output.name}: adaptive sync {'on' if output.adaptive_sync else 'off'}"
        )
        self._sync()

    def _toggle_output(self, name: str) -> None:
        output = self._find(name)
        if output is None:
            return
        if output.enabled and sum(1 for item in self.outputs if item.enabled) <= 1:
            self._notify("Cannot disable the last active monitor.", severity="warning")
            return
        output.enabled = not output.enabled
        self._clear_guides()
        self.status = f"{output.name}: {'enabled' if output.enabled else 'disabled'}"
        self._sync()

    def action_toggle_enabled(self) -> None:
        output = self.current
        if output is None:
            return
        self._toggle_output(output.name)

    def action_advanced(self) -> None:
        output = self.current
        if output is None:
            return

        def done(applied: bool | None) -> None:
            self._clear_guides()
            if applied:
                self.status = f"{output.name}: advanced settings updated"
            else:
                self.status = f"{output.name}: advanced settings cancelled"
            self._sync()

        self.push_screen(AdvancedSettingsScreen(output), done)

    def action_mirror(self) -> None:
        self.push_screen(UnsupportedScreen("Mirroring", MIRROR_MESSAGE))

    def action_apply(self) -> None:
        dirty = [output for output in self.outputs if output.name in self.dirty_names]
        self._apply_states(
            [output.state() for output in dirty],
            f"Applied {len(dirty)} output(s).",
        )

    def action_reload(self) -> None:
        if self._reload_from_backend():
            self.status = "Reloaded outputs."
            self._notify("Reloaded outputs from wlr-randr.")

    def action_reset(self) -> None:
        self._error = None
        for output in self.outputs:
            raw = self.baseline.get(output.name)
            if raw is None:
                continue
            try:
                OutputState.from_dict(output.name, raw).apply_to(output)
            except (KeyError, TypeError, ValueError):
                continue
        self._clear_guides()
        self.status = "Reset pending changes."
        self._sync()

    def action_revert_live(self) -> None:
        if not self._rollback:
            self._notify("No previous layout to revert to.", severity="warning")
            return
        previous = self.baseline_states()
        self._clear_guides()
        if self._apply_states(
            list(self._rollback),
            "Reverted to the previous layout.",
            remember_previous=False,
        ):
            self._rollback = previous

    def action_save_profile(self) -> None:
        if not self.outputs:
            self._notify("There are no outputs to save.", severity="warning")
            return

        def done(name: str | None) -> None:
            if name is None:
                return
            try:
                profile = self.store.save(
                    name, [output.state() for output in self.outputs]
                )
            except ProfileError as exc:
                self._notify(str(exc), severity="error")
                return
            self.active_profile = self._detect_active_profile()
            self._sync()
            self._notify(
                f"Saved profile {profile.name!r} ({len(profile.outputs)} outputs)."
            )

        self.push_screen(
            TextPrompt("Save layout as profile", placeholder="home, work, docked…"),
            done,
        )

    def _save_startup_profile(self) -> None:
        if not self.outputs:
            self._notify("There are no outputs to save.", severity="warning")
            return
        try:
            profile = self.store.save(
                STARTUP_PROFILE, [output.state() for output in self.outputs]
            )
        except ProfileError as exc:
            self._notify(str(exc), severity="error")
            return
        self.active_profile = self._detect_active_profile()
        self._sync()
        self._notify(
            f"Saved startup profile {profile.name!r} ({len(profile.outputs)} outputs)."
        )

    def action_save_startup(self) -> None:
        if not self.outputs:
            self._notify("There are no outputs to save.", severity="warning")
            return
        try:
            exists = STARTUP_PROFILE in self.store.names()
        except ProfileError as exc:
            self._notify(str(exc), severity="error")
            return
        if not exists:
            self._save_startup_profile()
            return

        def done(confirmed: bool | None) -> None:
            if confirmed:
                self._save_startup_profile()

        self.push_screen(
            ConfirmScreen(
                "Overwrite startup profile?",
                f"A profile named {STARTUP_PROFILE!r} already exists. "
                "Overwrite it with the current layout?",
            ),
            done,
        )

    def action_open_profiles(self) -> None:
        standalone = self._profiles_only

        def done(choice: str | None) -> None:
            if choice is None:
                if standalone:
                    self.exit()
                return
            if choice == "__launch__":
                self._profiles_only = False
                self.status = "Monitor editor ready."
                self._update_status()
                return
            if choice == "__save__":
                self.action_save_profile()
                return
            applied = self._apply_profile(choice)
            if standalone:
                if applied:
                    self.exit()
                else:
                    self._profiles_only = False

        self.push_screen(
            ProfileScreen(
                self.store,
                current_states=self.baseline_states(),
                initial_highlight=self.active_profile,
                standalone=standalone,
                allow_save=not standalone,
            ),
            done,
        )

    def _apply_profile(self, name: str) -> bool:
        try:
            profile = self.store.get(name)
        except ProfileError as exc:
            self._notify(str(exc), severity="error")
            return False
        if profile is None:
            self._notify(f"No profile named {name!r}.", severity="error")
            return False

        states: list[OutputState] = []
        for output in self.outputs:
            state = profile.outputs.get(output.name)
            if state is None and output.hardware_id:
                state = next(
                    (
                        candidate
                        for candidate in profile.outputs.values()
                        if candidate.hardware_id
                        and candidate.hardware_id == output.hardware_id
                    ),
                    None,
                )
            if state is None:
                continue
            state.apply_to(output)
            states.append(output.state())
        if not states:
            self._notify(
                f"Profile {name!r} does not match any connected output.",
                severity="warning",
            )
            return False
        return self._apply_states(states, f"Applied profile {name!r}.")

    # ------------------------------------------------------------------
    # Mouse events
    # ------------------------------------------------------------------

    @on(OutputSelected)
    def on_output_selected(self, event: OutputSelected) -> None:
        if event.name is None:
            return
        output = self._find(event.name)
        if output is not None:
            self._drag_raw[event.name] = (output.x, output.y)
        self._clear_guides()
        self._select_name(event.name)

    @on(OutputMoved)
    def on_output_moved(self, event: OutputMoved) -> None:
        output = self._find(event.name)
        if output is None:
            return
        base_x, base_y = self._drag_raw.get(event.name, (output.x, output.y))
        raw_x = base_x + event.dx
        raw_y = base_y + event.dy
        self._drag_raw[event.name] = (raw_x, raw_y)

        new_x = _quantize(raw_x, self.grid)
        new_y = _quantize(raw_y, self.grid)
        new_x, new_y, guides = snap_position(
            output,
            self.outputs,
            new_x,
            new_y,
            mode=self.snap_mode,
            threshold=self.snap_threshold,
        )
        self.guides = guides
        output.x, output.y = new_x, new_y
        self.status = f"{output.name}: {output.x},{output.y}"
        self._sync()

    @on(DragFinished)
    def on_drag_finished(self, event: DragFinished) -> None:
        output = self._find(event.name)
        if output is None:
            return
        self._drag_raw.pop(event.name, None)
        self._clear_guides()
        self.status = f"{output.name}: {output.x},{output.y}"
        self._sync()

    @on(OutputToggled)
    def on_output_toggled(self, event: OutputToggled) -> None:
        self._toggle_output(event.name)

    @on(OutputScaleStep)
    def on_output_scale_step(self, event: OutputScaleStep) -> None:
        output = self._find(event.name)
        if output is None:
            return
        output.scale = step_scale(output.scale, event.steps)
        self._clear_guides()
        self.status = f"{output.name}: scale {output.scale:g}"
        self._sync()


def _quantize(value: float, grid: int) -> int:
    """Quantise a position to a grid, truncating like hyprmon does."""
    if grid <= 1:
        return round(value)
    return int(value / grid) * grid


def _format_failures(failures: list[ApplyResult]) -> str:
    parts = []
    for failure in failures[:3]:
        detail = failure.detail or "unknown error"
        parts.append(f"{failure.name}: {detail}")
    if len(failures) > 3:
        parts.append(f"and {len(failures) - 3} more")
    return "wlr-randr failed — " + "; ".join(parts)
