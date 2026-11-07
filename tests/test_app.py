from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from unittest.mock import patch

from textual.widgets import Input, OptionList, Static

from wlr_monitor.app import GRID_SIZES, STARTUP_PROFILE, WlrMonitorApp
from wlr_monitor.compositor import Backend, Background, Mango, Niri, Sway
from wlr_monitor.layout import SnapMode
from wlr_monitor.model import Output, OutputState
from wlr_monitor.profiles import ProfileError, ProfileStore
from wlr_monitor.screens import (
    AdvancedSettingsScreen,
    ChoiceScreen,
    ConfirmScreen,
    HelpScreen,
    ProfileScreen,
    TextPrompt,
)
from wlr_monitor.widgets import (
    DetailsPanel,
    DragFinished,
    HelpItem,
    LayoutMap,
    OutputList,
    OutputMoved,
    OutputScaleStep,
    OutputToggled,
    layout_help,
)
from wlr_monitor.wlr import ApplyResult, WlrRandrError

FIXTURES = Path(__file__).parent / "fixtures"


def load_outputs() -> list[Output]:
    data = json.loads((FIXTURES / "outputs.json").read_text())
    return [Output.from_json(item) for item in data]


def test_app_loads_outputs_moves_and_uses_hyprmon_defaults(tmp_path: Path) -> None:
    outputs = load_outputs()
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=outputs,
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            assert [output.name for output in app.outputs] == ["HDMI-A-1", "eDP-1"]
            assert app.current is not None and app.current.name == "HDMI-A-1"
            assert app.grid == 32
            assert app.snap_mode is SnapMode.EDGES
            assert app.snap_threshold == 10

            map_text = app.query_one(LayoutMap).render().plain
            assert "HDMI-A-1" in map_text
            assert "eDP-1" in map_text

            app.action_move_right()
            assert app.outputs[0].x == 32
            assert "HDMI-A-1" in app.dirty_names
            assert app.guides

            app.action_reset()
            assert app.outputs[0].x == 0
            assert app.dirty_names == set()
            assert app.guides == []

            app.action_next_output()
            assert app.current is not None and app.current.name == "eDP-1"

            # App-level bindings must still receive keys with no focusable widget.
            await pilot.press("tab")
            assert app.current is not None and app.current.name == "HDMI-A-1"
            await pilot.press("right")
            assert app.outputs[0].x == 32

    asyncio.run(run())


def test_grid_and_snap_keys_cycle_like_hyprmon(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            assert app.grid == 32
            await pilot.press("g")
            assert app.grid == 64
            await pilot.press("g")
            assert app.grid == GRID_SIZES[0]
            await pilot.press("g")
            assert app.grid == GRID_SIZES[1]

            assert app.snap_mode is SnapMode.EDGES
            await pilot.press("L")
            assert app.snap_mode is SnapMode.CENTERS
            await pilot.press("L")
            assert app.snap_mode is SnapMode.BOTH
            await pilot.press("L")
            assert app.snap_mode is SnapMode.OFF

    asyncio.run(run())


def test_scale_stepping_clamps_to_hyprmon_range(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            output = app.outputs[0]
            output.scale = 2.95
            app.action_scale_up()
            assert output.scale == 3.0
            app.action_scale_up()
            assert output.scale == 3.0
            app.action_scale_down()
            assert output.scale == 2.95

            output.scale = 0.52
            app.action_scale_down()
            assert output.scale == 0.5
            app.action_scale_down()
            assert output.scale == 0.5

    asyncio.run(run())


def test_last_active_monitor_cannot_be_disabled(tmp_path: Path) -> None:
    outputs = load_outputs()
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=outputs,
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_toggle_enabled()
            assert app.outputs[0].enabled is False

            app.selected = 1
            app.action_toggle_enabled()
            assert app.outputs[1].enabled is True
            assert "last active monitor" in app.status.lower()

            app.action_toggle_enabled()
            assert app.outputs[1].enabled is True

    asyncio.run(run())


def test_single_output_cannot_be_disabled(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=[load_outputs()[0]],
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_toggle_enabled()
            assert app.outputs[0].enabled is True
            assert "last active monitor" in app.status.lower()

    asyncio.run(run())


def test_advanced_dialog_applies_and_cancels(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_advanced()
            await pilot.pause()
            assert isinstance(app.screen, AdvancedSettingsScreen)

            await pilot.press("t")
            assert app.outputs[0].transform == "90"
            await pilot.press("down", "space")
            assert app.outputs[0].adaptive_sync is True
            await pilot.press("enter")
            await pilot.pause()
            assert not isinstance(app.screen, AdvancedSettingsScreen)
            assert app.outputs[0].transform == "90"
            assert app.outputs[0].adaptive_sync is True
            assert "HDMI-A-1" in app.dirty_names

            app.action_reset()
            app.action_advanced()
            await pilot.pause()
            await pilot.press("t")
            assert app.outputs[0].transform == "90"
            await pilot.press("escape")
            await pilot.pause()
            assert app.outputs[0].transform == "normal"
            assert app.dirty_names == set()

    asyncio.run(run())


def test_help_blocks_shortcuts(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(80, 24)) as pilot:
            await pilot.pause()
            app.action_help()
            await pilot.pause()
            assert isinstance(app.screen, HelpScreen)
            await pilot.press("g")
            assert app.grid == 32
            await pilot.press("pagedown")
            await pilot.press("end")
            await pilot.press("escape")
            await pilot.pause()
            assert not isinstance(app.screen, HelpScreen)

    asyncio.run(run())


def sway_backend(tmp_path: Path) -> Backend:
    env = {"HOME": str(tmp_path / "home"), "PATH": str(tmp_path / "bin")}
    return Backend(FakeBackend(load_outputs()), Sway(env=env))  # type: ignore[arg-type]


def test_advanced_dialog_offers_compositor_color_settings(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        backend=sway_backend(tmp_path),
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_advanced()
            await pilot.pause()
            screen = app.screen
            assert isinstance(screen, AdvancedSettingsScreen)
            assert screen.fields == (
                "transform",
                "vrr",
                "enabled",
                "hdr",
                "bit_depth",
                "color",
            )
            await pilot.press("down", "down", "down", "space")
            assert app.outputs[0].hdr is True
            await pilot.press("down", "right", "right", "right")
            assert app.outputs[0].bit_depth == 10
            await pilot.press("down", "right")
            assert app.outputs[0].color_profile == "gamma22"
            await pilot.press("enter")
            await pilot.pause()
            assert "HDMI-A-1" in app.dirty_names
            details = app.query_one(DetailsPanel).render()
            assert "HDR" in str(details)

            # Reset must drop the new settings again, not keep them.
            app.action_reset()
            assert app.outputs[0].hdr is None
            assert app.outputs[0].bit_depth is None
            assert app.outputs[0].color_profile is None
            assert app.dirty_names == set()

            app.action_advanced()
            await pilot.pause()
            await pilot.press("down", "down", "down", "space", "escape")
            await pilot.pause()
            assert app.outputs[0].hdr is None

    asyncio.run(run())


def test_generic_backend_marks_color_settings_unsupported(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_advanced()
            await pilot.pause()
            screen = app.screen
            assert isinstance(screen, AdvancedSettingsScreen)
            assert screen.fields == ("transform", "vrr", "enabled")
            body = str(screen.query_one("#advanced-body").render())
            assert "Not supported by wlroots" in body

    asyncio.run(run())


def test_apply_warnings_are_notified(tmp_path: Path) -> None:
    class WarningBackend(FakeBackend):
        def apply_states(self, states, *, profile=None):  # type: ignore[no-untyped-def]
            results = super().apply_states(states)
            self.profile = profile
            return [*results, ApplyResult("config", [], True, warning="heads up")]

    backend = WarningBackend(load_outputs())
    path = tmp_path / "profiles.json"
    store = ProfileStore(path)
    store.save("desk", [output.state() for output in load_outputs()])
    app = WlrMonitorApp(backend=backend, store=store)

    async def run() -> None:
        async with app.run_test(size=(110, 34), notifications=True) as pilot:
            await pilot.pause()
            app.outputs[0].x = 64
            app.action_apply()
            await pilot.pause()
            messages = [notification.message for notification in app._notifications]
            assert "heads up" in messages
            assert app._apply_profile("desk")
            assert backend.profile == "desk"

    asyncio.run(run())


def test_small_terminals_hide_secondary_sections(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(40, 14)) as pilot:
            await pilot.pause()
            assert app.screen.has_class("-compact")
            assert app.screen.has_class("-tiny")
            assert app.query_one(DetailsPanel).display is False
            assert app.query_one(OutputList).display is False
            assert "HDMI-A-1" in app.query_one(LayoutMap).render().plain
            await pilot.resize_terminal(120, 40)
            await pilot.pause()
            assert not app.screen.has_class("-compact")
            assert app.query_one(DetailsPanel).display is True
            assert app.query_one(OutputList).display is True

    asyncio.run(run())


def test_help_bar_wraps_and_drops_low_priority_items() -> None:
    items = [
        HelpItem("a", "Apply", 0),
        HelpItem("z", "Reset", 6),
        HelpItem("q", "Quit", 1),
    ]
    assert [[item.key for item in line] for line in layout_help(items, 80)] == [
        ["a", "z", "q"]
    ]
    assert [[item.key for item in line] for line in layout_help(items, 10)] == [
        ["a"],
        ["q"],
    ]


def test_theme_uses_terminal_ansi_colors(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            assert app.theme == "terminal"
            assert app.native_ansi_color is True
            colors = set()
            for strip in app.screen._compositor.render_strips():
                for segment in strip:
                    style = segment.style
                    if style is None:
                        continue
                    for color in (style.color, style.bgcolor):
                        if color is not None:
                            colors.add(color.type.name)
            assert colors <= {"DEFAULT", "STANDARD"}

    asyncio.run(run())


def test_app_saves_profiles_and_marks_active(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    app = WlrMonitorApp(
        store=ProfileStore(path),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_save_profile()
            await pilot.pause()
            await pilot.press("w", "o", "r", "k", "enter")
            await pilot.pause()
            assert app.active_profile == "work"

            app.action_open_profiles()
            await pilot.pause()
            screen = app.screen
            assert isinstance(screen, ProfileScreen)
            options = screen.query_one(OptionList)
            prompts = [
                str(options.get_option_at_index(index).prompt)
                for index in range(options.option_count)
            ]
            assert any("work *" in prompt for prompt in prompts)

            # Rename through the browser.
            screen.action_rename()
            await pilot.pause()
            assert isinstance(app.screen, TextPrompt)
            prompt = app.screen.query_one(Input)
            prompt.value = "office"
            await pilot.press("enter")
            await pilot.pause()
            assert ProfileStore(path).names() == ["office"]

            # Delete through the browser, with confirmation.
            screen = app.screen
            assert isinstance(screen, ProfileScreen)
            screen.action_delete()
            await pilot.pause()
            await pilot.press("y")
            await pilot.pause()
            assert ProfileStore(path).names() == []

    asyncio.run(run())


def test_startup_profile_overwrite_confirmation(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    app = WlrMonitorApp(
        store=ProfileStore(path),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_save_startup()
            await pilot.pause()
            assert ProfileStore(path).names() == [STARTUP_PROFILE]

            app.outputs[0].x = 64
            app.action_save_startup()
            await pilot.pause()
            assert isinstance(app.screen, ConfirmScreen)
            await pilot.press("y")
            await pilot.pause()

            profiles = ProfileStore(path).load()
            assert profiles[STARTUP_PROFILE].outputs["HDMI-A-1"].x == 64

    asyncio.run(run())


def test_profiles_only_launches_browser_and_editor(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
        profiles_only=True,
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            await pilot.pause()
            assert isinstance(app.screen, ProfileScreen)
            await pilot.press("l")
            await pilot.pause()
            assert not isinstance(app.screen, ProfileScreen)
            assert app._profiles_only is False

    asyncio.run(run())


def test_output_mouse_messages_are_wired(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.on_output_toggled(OutputToggled("HDMI-A-1"))
            assert app.outputs[0].enabled is False

            app.on_output_scale_step(OutputScaleStep("HDMI-A-1", 1))
            assert app.outputs[0].scale == 1.05

            app._drag_raw["HDMI-A-1"] = (app.outputs[0].x, app.outputs[0].y)
            start_x = app.outputs[0].x
            app.on_output_moved(OutputMoved("HDMI-A-1", 40, 0))
            assert app.outputs[0].x == start_x + 32

    asyncio.run(run())


class FakeBackend:
    def __init__(self, outputs: list[Output]) -> None:
        self.outputs = outputs
        self.applied: list[str] = []

    def read_outputs(self) -> list[Output]:
        return self.outputs

    def apply_states(
        self, states: list[OutputState], *, profile: str | None = None
    ) -> list[ApplyResult]:
        results: list[ApplyResult] = []
        for state in states:
            output = next(item for item in self.outputs if item.name == state.name)
            state.apply_to(output)
            self.applied.append(state.name)
            results.append(ApplyResult(name=state.name, command=[], ok=True))
        return results


def test_apply_uses_backend_and_updates_baseline(tmp_path: Path) -> None:
    backend = FakeBackend(load_outputs())
    app = WlrMonitorApp(
        backend=backend,
        store=ProfileStore(tmp_path / "profiles.json"),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_revert_live()
            assert "no previous layout" in app.status.lower()

            app.outputs[0].x = 96
            app.action_apply()
            await pilot.pause()
            assert backend.applied == ["HDMI-A-1"]
            assert app.outputs[0].x == 96
            assert app.dirty_names == set()

            app.action_revert_live()
            await pilot.pause()
            assert backend.applied == ["HDMI-A-1", "HDMI-A-1", "eDP-1"]
            assert app.outputs[0].x == 0
            assert app.dirty_names == set()

    asyncio.run(run())


def test_mount_uses_outputs_read_in_the_background(tmp_path: Path) -> None:
    class CountingBackend(FakeBackend):
        reads = 0

        def read_outputs(self) -> list[Output]:
            self.reads += 1
            return super().read_outputs()

    backend = CountingBackend(load_outputs())
    app = WlrMonitorApp(
        backend=backend,
        store=ProfileStore(tmp_path / "profiles.json"),
        pending_outputs=Background(lambda: load_outputs()[:1]),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            assert [output.name for output in app.outputs] == ["HDMI-A-1"]
            assert backend.reads == 0
            # Later reloads read fresh state from the backend.
            app.action_reload()
            assert backend.reads == 1
            assert [output.name for output in app.outputs] == ["HDMI-A-1", "eDP-1"]

    asyncio.run(run())


def test_background_read_errors_are_shown(tmp_path: Path) -> None:
    def fail() -> list[Output]:
        raise WlrRandrError("wlr-randr was not found.")

    app = WlrMonitorApp(
        backend=FakeBackend(load_outputs()),
        store=ProfileStore(tmp_path / "profiles.json"),
        pending_outputs=Background(fail),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            assert app.outputs == []
            assert app.status == "wlr-randr was not found."

    asyncio.run(run())


def test_mode_and_scale_pickers(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_pick_mode()
            await pilot.pause()
            assert isinstance(app.screen, ChoiceScreen)
            await pilot.press("g")
            assert app.grid == 32
            await pilot.press("down", "enter")
            await pilot.pause()
            assert app.outputs[0].mode is not None
            assert app.outputs[0].mode.refresh == 60.0

            app.action_pick_scale()
            await pilot.pause()
            assert isinstance(app.screen, ChoiceScreen)
            await pilot.press("down", "enter")
            await pilot.pause()
            assert app.outputs[0].scale == 1.1

    asyncio.run(run())


def test_corrupt_profile_file_does_not_crash_browser(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    path.write_text("{ nope")
    app = WlrMonitorApp(store=ProfileStore(path), initial_outputs=load_outputs())

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_open_profiles()
            await pilot.pause()
            assert isinstance(app.screen, ProfileScreen)
            error = app.screen.query_one("#profile-error")
            assert "Could not parse" in str(error.render())
            await pilot.press("escape")
            await pilot.pause()

    asyncio.run(run())


def test_mouse_click_and_right_click_on_layout_map(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            map_widget = app.query_one(LayoutMap)
            map_widget.render()
            geometry = map_widget._geometry
            assert geometry is not None
            offset = map_widget.content_offset

            rect = geometry.boxes["eDP-1"]
            click_x = rect.x + offset.x + 1
            click_y = rect.y + offset.y + 1
            await pilot.click(LayoutMap, offset=(click_x, click_y))
            await pilot.pause()
            assert app.current is not None and app.current.name == "eDP-1"

            rect = geometry.boxes["HDMI-A-1"]
            click_x = rect.x + offset.x + 1
            click_y = rect.y + offset.y + 1
            await pilot.click(LayoutMap, offset=(click_x, click_y), button=3)
            await pilot.pause()
            assert app.outputs[0].enabled is False

    asyncio.run(run())


def test_layout_map_receives_guides_and_draws_disabled_outputs(
    tmp_path: Path,
) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.outputs[1].enabled = False
            app._sync()
            map_widget = app.query_one(LayoutMap)
            assert "[OFF]" in map_widget.render().plain

            app.action_move_right()
            assert app.guides
            assert map_widget.guides == app.guides
            app._drag_raw["HDMI-A-1"] = (app.outputs[0].x, app.outputs[0].y)
            app.on_drag_finished(DragFinished("HDMI-A-1"))
            assert app.guides == []
            assert app.outputs[0].x == 32

    asyncio.run(run())


def test_profile_browser_reorders_profiles(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    outputs = load_outputs()
    store = ProfileStore(path)
    store.save("work", [output.state() for output in outputs])
    moved = [output.state() for output in outputs]
    moved[0].x = 1920
    store.save("home", moved)

    app = WlrMonitorApp(store=ProfileStore(path), initial_outputs=outputs)

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_open_profiles()
            await pilot.pause()
            screen = app.screen
            assert isinstance(screen, ProfileScreen)
            screen.action_move_down()
            await pilot.pause()
            assert ProfileStore(path).names() == ["home", "work"]
            screen.action_move_up()
            await pilot.pause()
            assert ProfileStore(path).names() == ["work", "home"]

    asyncio.run(run())


def test_startup_creates_missing_include_and_shows_hint(tmp_path: Path) -> None:
    home = tmp_path / "home"
    env = {"HOME": str(home), "PATH": str(tmp_path / "bin")}
    backend = Backend(FakeBackend(load_outputs()), Niri(env=env))  # type: ignore[arg-type]
    app = WlrMonitorApp(backend=backend, store=ProfileStore(tmp_path / "p.json"))

    async def run() -> None:
        async with app.run_test(size=(110, 34), notifications=True) as pilot:
            await pilot.pause()
            include = home / ".config" / "niri" / "wlr-monitor.kdl"
            # niri rejects a missing include, so the file must exist first.
            assert include.exists()
            assert 'output "Microstep MSI MP275 PC3M394200857" {' in include.read_text()
            messages = [item.message for item in app._notifications]
            assert any("wlr-monitor.kdl" in message for message in messages)

    asyncio.run(run())


def test_startup_never_writes_the_mango_include(tmp_path: Path) -> None:
    # mango ignores a missing source_optional file, and writing one could
    # trigger a full config reload just by opening the TUI.
    home = tmp_path / "home"
    env = {"HOME": str(home), "PATH": str(tmp_path / "bin")}
    backend = Backend(FakeBackend(load_outputs()), Mango(env=env))  # type: ignore[arg-type]
    app = WlrMonitorApp(backend=backend, store=ProfileStore(tmp_path / "p.json"))

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            assert not (home / ".config" / "mango" / "wlr-monitor.conf").exists()

    asyncio.run(run())


def test_profiles_from_other_compositors_show_no_phantom_changes(
    tmp_path: Path,
) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    states = [output.state() for output in load_outputs()]
    states[0].hdr = True
    states[0].bit_depth = 10
    store.save("sway-hdr", states)
    backend = FakeBackend(load_outputs())
    app = WlrMonitorApp(backend=backend, store=store)

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            # The generic wlroots backend cannot do HDR or bit depth.
            assert app._apply_profile("sway-hdr")
            assert app.outputs[0].hdr is None
            assert app.outputs[0].bit_depth is None
            assert app.dirty_names == set()

    asyncio.run(run())


def test_rotation_is_named_consistently(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            assert "Rotation" in app.query_one(OutputList).render().plain
            app.action_pick_transform()
            await pilot.pause()
            assert isinstance(app.screen, ChoiceScreen)
            assert app.screen.dialog_title == "Rotation"
            await pilot.press("down", "enter")
            await pilot.pause()
            assert app.status == "HDMI-A-1: rotation 90"
            details = str(app.query_one(DetailsPanel).render())
            assert "Rotation" in details and "rotation normal → 90" in details

    asyncio.run(run())


def test_advanced_dialog_cannot_disable_the_last_monitor(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=[load_outputs()[0]],
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_advanced()
            await pilot.pause()
            await pilot.press("down", "down", "space", "enter")
            await pilot.pause()
            assert app.outputs[0].enabled is True
            assert app.dirty_names == set()

    asyncio.run(run())


def test_apply_refuses_to_turn_off_every_monitor(tmp_path: Path) -> None:
    backend = FakeBackend(load_outputs())
    store = ProfileStore(tmp_path / "profiles.json")
    dark = [output.state() for output in load_outputs()]
    for state in dark:
        state.enabled = False
    store.save("dark", dark)
    app = WlrMonitorApp(backend=backend, store=store)

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            assert not app._apply_profile("dark")
            assert backend.applied == []
            assert "every monitor" in app.status

    asyncio.run(run())


def test_profile_browser_survives_write_errors(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    store = ProfileStore(path)
    store.save("work", [output.state() for output in load_outputs()])
    store.save("home", [output.state() for output in load_outputs()])
    app = WlrMonitorApp(store=store, initial_outputs=load_outputs())
    failure = ProfileError("Could not write profiles.json: disk full")

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_open_profiles()
            await pilot.pause()
            screen = app.screen
            assert isinstance(screen, ProfileScreen)
            error = screen.query_one("#profile-error", Static)
            screen.query_one(OptionList).highlighted = 0  # "work", can move down
            with patch.object(ProfileStore, "_write", side_effect=failure):
                screen.action_move_down()
                await pilot.pause()
                assert app.is_running
                assert "disk full" in str(error.render()) and error.display

            # The delete error must survive the list reload.
            readonly = ProfileError("Could not write profiles.json: read-only")
            with patch.object(ProfileStore, "_write", side_effect=readonly):
                screen.query_one(OptionList).highlighted = 0
                screen.action_delete()
                await pilot.press("y")
                await pilot.pause()
                assert "read-only" in str(error.render()) and error.display
            assert ProfileStore(path).names() == ["work", "home"]

            # The next successful action clears it.
            screen.query_one(OptionList).highlighted = 0
            screen.action_move_down()
            await pilot.pause()
            assert not error.display
            assert ProfileStore(path).names() == ["home", "work"]

    asyncio.run(run())


def test_startup_profile_overwrite_asks_even_if_broken(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    store = ProfileStore(path)
    store.save(STARTUP_PROFILE, [output.state() for output in load_outputs()])
    raw = json.loads(path.read_text())
    raw["profiles"][STARTUP_PROFILE]["outputs"]["HDMI-A-1"]["transform"] = "bogus"
    path.write_text(json.dumps(raw))
    app = WlrMonitorApp(store=store, initial_outputs=load_outputs())

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_save_startup()
            await pilot.pause()
            assert isinstance(app.screen, ConfirmScreen)

    asyncio.run(run())


def test_icc_prompt_expands_home_and_rejects_relative_paths(tmp_path: Path) -> None:
    app = WlrMonitorApp(
        backend=sway_backend(tmp_path),
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_advanced()
            await pilot.pause()
            await pilot.press("i")
            await pilot.pause()
            prompt = app.screen.query_one(Input)
            prompt.value = "relative.icc"
            await pilot.press("enter")
            await pilot.pause()
            assert app.outputs[0].color_profile is None

            await pilot.press("i")
            await pilot.pause()
            app.screen.query_one(Input).value = "~/display.icc"
            await pilot.press("enter")
            await pilot.pause()
            home = os.path.expanduser("~")
            assert app.outputs[0].color_profile == f"icc:{home}/display.icc"

    asyncio.run(run())
