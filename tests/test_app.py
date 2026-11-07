from __future__ import annotations

import asyncio
import json
from pathlib import Path

from textual.widgets import Input, OptionList

from wlr_randr_tui.app import GRID_SIZES, STARTUP_PROFILE, WlrRandrApp
from wlr_randr_tui.layout import SnapMode
from wlr_randr_tui.model import Output, OutputState
from wlr_randr_tui.profiles import ProfileStore
from wlr_randr_tui.screens import (
    AdvancedSettingsScreen,
    ChoiceScreen,
    ConfirmScreen,
    HelpScreen,
    ProfileScreen,
    TextPrompt,
    UnsupportedScreen,
)
from wlr_randr_tui.widgets import (
    DragFinished,
    LayoutMap,
    OutputMoved,
    OutputScaleStep,
    OutputToggled,
)
from wlr_randr_tui.wlr import ApplyResult

FIXTURES = Path(__file__).parent / "fixtures"


def load_outputs() -> list[Output]:
    data = json.loads((FIXTURES / "outputs.json").read_text())
    return [Output.from_json(item) for item in data]


def test_app_loads_outputs_moves_and_uses_hyprmon_defaults(tmp_path: Path) -> None:
    outputs = load_outputs()
    app = WlrRandrApp(
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
    app = WlrRandrApp(
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
    app = WlrRandrApp(
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
    app = WlrRandrApp(
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
    app = WlrRandrApp(
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
    app = WlrRandrApp(
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
    app = WlrRandrApp(
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


def test_mirror_shows_unsupported_dialog(tmp_path: Path) -> None:
    app = WlrRandrApp(
        store=ProfileStore(tmp_path / "profiles.json"),
        initial_outputs=load_outputs(),
    )

    async def run() -> None:
        async with app.run_test(size=(110, 34)) as pilot:
            await pilot.pause()
            app.action_mirror()
            await pilot.pause()
            assert isinstance(app.screen, UnsupportedScreen)
            await pilot.press("escape")
            await pilot.pause()
            assert not isinstance(app.screen, UnsupportedScreen)

    asyncio.run(run())


def test_app_saves_profiles_and_marks_active(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    app = WlrRandrApp(
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
    app = WlrRandrApp(
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
    app = WlrRandrApp(
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
    app = WlrRandrApp(
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

    def apply_states(self, states: list[OutputState]) -> list[ApplyResult]:
        results: list[ApplyResult] = []
        for state in states:
            output = next(item for item in self.outputs if item.name == state.name)
            state.apply_to(output)
            self.applied.append(state.name)
            results.append(ApplyResult(name=state.name, command=[], ok=True))
        return results


def test_apply_uses_backend_and_updates_baseline(tmp_path: Path) -> None:
    backend = FakeBackend(load_outputs())
    app = WlrRandrApp(
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


def test_mode_and_scale_pickers(tmp_path: Path) -> None:
    app = WlrRandrApp(
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
    app = WlrRandrApp(store=ProfileStore(path), initial_outputs=load_outputs())

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
    app = WlrRandrApp(
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
    app = WlrRandrApp(
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

    app = WlrRandrApp(store=ProfileStore(path), initial_outputs=outputs)

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
