from __future__ import annotations

import json
import os
import subprocess
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

import pytest

from wlr_randr_tui.compositor import (
    Backend,
    Compositor,
    CompositorError,
    Mango,
    Niri,
    StateFileError,
    Sway,
    detect_compositor,
)
from wlr_randr_tui.model import Output
from wlr_randr_tui.wlr import ApplyResult

FIXTURES = Path(__file__).parent / "fixtures"


def load_outputs() -> list[Output]:
    data = json.loads((FIXTURES / "outputs.json").read_text())
    return [Output.from_json(item) for item in data]


def config_dir(tmp_path: Path) -> Path:
    return tmp_path / "home" / ".config"


def make_env(tmp_path: Path, *tools: str) -> dict[str, str]:
    """An environment with fake IPC tools on PATH."""
    bindir = tmp_path / "bin"
    bindir.mkdir(exist_ok=True)
    for tool in tools:
        path = bindir / tool
        path.write_text("#!/bin/sh\nexit 0\n")
        path.chmod(0o755)
    return {
        "HOME": str(tmp_path / "home"),
        "XDG_CONFIG_HOME": str(config_dir(tmp_path)),
        "XDG_STATE_HOME": str(tmp_path / "state"),
        "PATH": str(bindir),
    }


class FakeWlr:
    def __init__(self, outputs: list[Output] | None = None) -> None:
        self.outputs = outputs if outputs is not None else load_outputs()
        self.applied: list[str] = []

    def read_outputs(self) -> list[Output]:
        return [Output.from_json(_raw(output)) for output in self.outputs]

    def apply_states(self, states):  # type: ignore[no-untyped-def]
        self.applied.extend(state.name for state in states)
        return [ApplyResult(name=state.name, command=[], ok=True) for state in states]


def _raw(output: Output) -> dict[str, object]:
    data = json.loads((FIXTURES / "outputs.json").read_text())
    raw = next(item for item in data if item["name"] == output.name)
    raw["position"] = {"x": output.x, "y": output.y}
    return raw


class Recorder:
    """Stand-in for subprocess.run that records commands."""

    def __init__(self, responses: dict[str, str] | None = None) -> None:
        self.commands: list[list[str]] = []
        self.responses = responses or {}

    def __call__(self, command, **kwargs):  # type: ignore[no-untyped-def]
        self.commands.append(list(command))
        joined = " ".join(command)
        stdout = next(
            (value for key, value in self.responses.items() if key in joined), ""
        )
        return subprocess.CompletedProcess(command, 0, stdout=stdout, stderr="")


# ----------------------------------------------------------------------
# Detection
# ----------------------------------------------------------------------


@pytest.mark.parametrize(
    ("env", "expected"),
    [
        ({"MANGO_INSTANCE_SIGNATURE": "/run/mango.sock"}, Mango),
        ({"SWAYSOCK": "/run/sway.sock"}, Sway),
        ({"NIRI_SOCKET": "/run/niri.sock"}, Niri),
        ({"XDG_CURRENT_DESKTOP": "sway:wlroots"}, Sway),
        ({"XDG_CURRENT_DESKTOP": "mango"}, Mango),
        ({}, Compositor),
    ],
)
def test_detect_compositor_from_environment(env, expected) -> None:  # type: ignore[no-untyped-def]
    assert type(detect_compositor(env=env)) is expected


def test_detect_compositor_honours_explicit_name() -> None:
    assert type(detect_compositor("niri", env={"SWAYSOCK": "x"})) is Niri
    with pytest.raises(ValueError):
        detect_compositor("kwin", env={})


def test_generic_wlroots_has_no_include_or_extras(tmp_path: Path) -> None:
    compositor = Compositor(env=make_env(tmp_path))
    assert compositor.include_path() is None
    assert compositor.include_hint() == ""
    outputs = load_outputs()
    outputs[0].hdr = True
    compositor.enrich(outputs)
    assert outputs[0].hdr is None


# ----------------------------------------------------------------------
# Rendering
# ----------------------------------------------------------------------


def test_mango_rules_match_edid_and_fall_back_to_name(tmp_path: Path) -> None:
    hdmi, edp = load_outputs()
    hdmi.hdr = True
    hdmi.color_profile = "icc:/icc/display.icc"
    edp.transform = "flipped-90"
    mango = Mango(env=make_env(tmp_path))
    assert mango.render_output(hdmi) == [
        (
            "monitor_rule=make:Microstep,model:MSI MP275,serial:PC3M394200857,"
            "width:1920,height:1080,refresh:100,x:0,y:0,scale:1,rr:0,vrr:0,"
            "hdr:1,icc:/icc/display.icc"
        )
    ]
    # eDP-1 has no serial, so it is matched by an anchored connector regex.
    assert mango.render_output(edp) == [
        (
            "monitor_rule=name:^eDP-1$,width:1920,height:1080,refresh:60,"
            "x:1920,y:0,scale:1.2,rr:5,vrr:1"
        )
    ]
    edp.enabled = False
    assert mango.render_output(edp) == ["monitor_rule=name:^eDP-1$,disable:1"]


def test_mango_rejects_values_it_cannot_parse(tmp_path: Path) -> None:
    hdmi = load_outputs()[0]
    hdmi.model = "Model, with comma"
    mango = Mango(env=make_env(tmp_path))
    assert mango.render_output(hdmi)[0].startswith("monitor_rule=name:^HDMI-A-1$,")
    assert not mango.color_profile_supported("icc:/a,b.icc")
    hdmi.color_profile = "icc:/" + "x" * 600
    with pytest.raises(CompositorError, match="line limit"):
        mango.render_output(hdmi)


def test_sway_output_lines(tmp_path: Path) -> None:
    hdmi, edp = load_outputs()
    hdmi.hdr = False
    hdmi.bit_depth = 10
    hdmi.color_profile = 'icc:/my "icc".icc'
    sway = Sway(env=make_env(tmp_path))
    assert sway.render_output(hdmi) == [
        (
            'output "Microstep MSI MP275 PC3M394200857" enable '
            "mode 1920x1080@100.000Hz pos 0 0 scale 1 transform normal "
            "adaptive_sync off render_bit_depth 10 hdr off "
            'color_profile icc "/my \\"icc\\".icc"'
        )
    ]
    edp.enabled = False
    assert sway.render_output(edp) == ['output "eDP-1" disable']


def test_niri_output_blocks(tmp_path: Path) -> None:
    hdmi, edp = load_outputs()
    niri = Niri(env=make_env(tmp_path))
    assert niri.render_output(edp) == [
        'output "eDP-1" {',
        '    mode "1920x1080@60.000"',
        "    scale 1.2",
        '    transform "normal"',
        "    position x=1920 y=0",
        "    variable-refresh-rate",
        "}",
    ]
    hdmi.enabled = False
    assert niri.render_output(hdmi) == [
        'output "Microstep MSI MP275 PC3M394200857" {',
        "    off",
        "}",
    ]


def test_unprintable_edid_strings_never_reach_the_config(tmp_path: Path) -> None:
    hdmi = load_outputs()[0]
    hdmi.model = "Evil\ninclude /tmp/x"
    sway = Sway(env=make_env(tmp_path))
    assert sway.render_output(hdmi)[0].startswith('output "HDMI-A-1" ')
    text = sway.render_include([hdmi], profile="work\nexec rm -rf ~")
    assert "\nexec" not in text
    assert "# Profile: workexec rm -rf ~" in text


# ----------------------------------------------------------------------
# Include file handling
# ----------------------------------------------------------------------


def test_write_include_and_state_sidecar(tmp_path: Path) -> None:
    env = make_env(tmp_path)
    sway = Sway(env=env)
    outputs = load_outputs()
    outputs[0].color_profile = "srgb"
    outputs[0].bit_depth = 10
    path = sway.write_include(outputs, profile="work")
    assert path == config_dir(tmp_path) / "sway" / "wlr-randr-tui.conf"
    text = path.read_text()
    assert text.startswith("# Generated by wlr-randr-tui")
    assert "# Profile: work" in text
    assert "color_profile srgb" in text

    # Settings sway cannot report come back from the sidecar.
    fresh = load_outputs()
    sway.enrich(fresh)
    assert fresh[0].color_profile == "srgb"
    assert fresh[0].bit_depth == 10
    assert fresh[1].color_profile is None


def test_write_include_refuses_symlinks(tmp_path: Path) -> None:
    env = make_env(tmp_path)
    mango = Mango(env=env)
    path = mango.include_path()
    assert path is not None
    path.parent.mkdir(parents=True)
    store_file = tmp_path / "store-file"
    store_file.write_text("")
    path.symlink_to(store_file)
    with pytest.raises(CompositorError, match="symlink"):
        mango.write_include(load_outputs())


def test_write_include_can_be_disabled(tmp_path: Path) -> None:
    niri = Niri(env=make_env(tmp_path), write_include=False)
    assert niri.write_include(load_outputs()) is None
    assert not (config_dir(tmp_path) / "niri" / "wlr-randr-tui.kdl").exists()


def test_include_path_override(tmp_path: Path) -> None:
    custom = tmp_path / "custom.conf"
    sway = Sway(env=make_env(tmp_path), include_path=custom)
    assert sway.include_path() == custom
    sway.write_include(load_outputs())
    assert custom.exists()


def test_include_detection_for_each_compositor(tmp_path: Path) -> None:
    env = make_env(tmp_path)
    config = config_dir(tmp_path)

    mango = Mango(env=env)
    (config / "mango").mkdir(parents=True)
    main = config / "mango" / "config.conf"
    main.write_text("source=./binds.conf\n")
    assert not mango.include_is_sourced()
    assert "source_optional=~/.config/mango/wlr-randr-tui.conf" in mango.include_hint()
    # A nested include one level deep is followed, with ~ expanded.
    (config / "mango" / "binds.conf").write_text(
        "source_optional=~/.config/mango/wlr-randr-tui.conf\n"
    )
    assert mango.include_is_sourced()
    assert mango.include_hint() == ""
    (config / "mango" / "binds.conf").write_text("# source_optional=x\n")
    assert not mango.include_is_sourced()

    sway = Sway(env=env)
    (config / "sway").mkdir()
    (config / "sway" / "config").write_text(f"include {config}/sway/*.conf\n")
    assert sway.include_is_sourced() is False  # the glob matches nothing yet
    sway.write_include(load_outputs())
    assert sway.include_is_sourced()

    niri = Niri(env=env)
    (config / "niri").mkdir()
    (config / "niri" / "config.kdl").write_text(
        '// include "wlr-randr-tui.kdl"\ninput {}\n'
    )
    assert not niri.include_is_sourced()
    assert niri.include_line() == 'include "wlr-randr-tui.kdl"'
    (config / "niri" / "config.kdl").write_text(
        'include optional=true "wlr-randr-tui.kdl"\n'
    )
    assert niri.include_is_sourced()


# ----------------------------------------------------------------------
# Applying
# ----------------------------------------------------------------------


def test_sway_applies_with_swaymsg_and_persists(tmp_path: Path) -> None:
    env = make_env(tmp_path, "swaymsg")
    outputs = load_outputs()
    backend = Backend(FakeWlr(outputs), Sway(env=env))
    states = [output.state() for output in outputs]
    states[0].enabled = False
    states[1].adaptive_sync = True
    recorder = Recorder()
    with patch("subprocess.run", side_effect=recorder):
        results = backend.apply_states(states, profile="desk")

    assert all(result.ok for result in results)
    applies = [cmd for cmd in recorder.commands if "--" in cmd]
    # Enabled outputs are configured before others are disabled.
    assert applies[0][-1].startswith('output "eDP-1" enable')
    assert "adaptive_sync on" in applies[0][-1]
    assert applies[1][-1] == 'output "HDMI-A-1" disable'
    include = (config_dir(tmp_path) / "sway" / "wlr-randr-tui.conf").read_text()
    assert "# Profile: desk" in include
    assert 'output "Microstep MSI MP275 PC3M394200857" disable' in include


def test_sway_reads_hdr_and_vrr_from_ipc(tmp_path: Path) -> None:
    env = make_env(tmp_path, "swaymsg")
    payload = json.dumps(
        [{"name": "HDMI-A-1", "hdr": True, "adaptive_sync_status": "enabled"}]
    )
    recorder = Recorder({"get_outputs": payload})
    outputs = load_outputs()
    with patch("subprocess.run", side_effect=recorder):
        Sway(env=env).enrich(outputs)
    assert outputs[0].hdr is True
    assert outputs[0].adaptive_sync is True
    assert outputs[1].hdr is None


def test_niri_applies_each_setting_with_niri_msg(tmp_path: Path) -> None:
    env = make_env(tmp_path, "niri")
    outputs = load_outputs()
    outputs[1].x = -1600
    backend = Backend(FakeWlr(outputs), Niri(env=env))
    recorder = Recorder()
    with patch("subprocess.run", side_effect=recorder):
        results = backend.apply_states([outputs[1].state()])
    assert [result.ok for result in results] == [True]
    tail = [cmd[2:] for cmd in recorder.commands if cmd[1:3] == ["msg", "output"]]
    assert tail == [
        ["output", "eDP-1", "on"],
        ["output", "eDP-1", "mode", "1920x1080@60.000"],
        ["output", "eDP-1", "scale", "1.2"],
        ["output", "eDP-1", "transform", "normal"],
        ["output", "eDP-1", "position", "set", "--", "-1600", "0"],
        ["output", "eDP-1", "vrr", "on"],
    ]
    assert (config_dir(tmp_path) / "niri" / "wlr-randr-tui.kdl").exists()


def test_mango_reloads_config_when_the_include_is_sourced(tmp_path: Path) -> None:
    env = make_env(tmp_path, "mmsg")
    (config_dir(tmp_path) / "mango").mkdir(parents=True)
    (config_dir(tmp_path) / "mango" / "config.conf").write_text(
        "source_optional=./wlr-randr-tui.conf\n"
    )
    outputs = load_outputs()
    wlr = FakeWlr(outputs)
    backend = Backend(wlr, Mango(env=env))
    state = outputs[0].state()
    state.adaptive_sync = True
    recorder = Recorder()
    with patch("subprocess.run", side_effect=recorder):
        results = backend.apply_states([state], profile="gaming")

    assert [result.ok for result in results] == [True]
    assert wlr.applied == []  # the rule reload applies everything, VRR included
    assert [cmd[1:] for cmd in recorder.commands if cmd[1:2] == ["dispatch"]] == [
        ["dispatch", "reload_config"]
    ]
    include = (config_dir(tmp_path) / "mango" / "wlr-randr-tui.conf").read_text()
    assert "serial:PC3M394200857," in include
    assert ",vrr:1" in include
    assert "name:^eDP-1$" in include  # untouched outputs keep their rule


def test_mango_reload_errors_are_reported(tmp_path: Path) -> None:
    env = make_env(tmp_path, "mmsg")
    (config_dir(tmp_path) / "mango").mkdir(parents=True)
    (config_dir(tmp_path) / "mango" / "config.conf").write_text(
        f"source={config_dir(tmp_path)}/mango/wlr-randr-tui.conf\n"
    )
    mango = Mango(env=env)
    assert mango.include_is_sourced()
    outputs = load_outputs()
    recorder = Recorder({"reload_config": '{"error":"bad"}'})
    with patch("subprocess.run", side_effect=recorder):
        results = Backend(FakeWlr(outputs), mango).apply_states([outputs[0].state()])
    assert [result.ok for result in results] == [False]


def test_mango_without_include_uses_wlr_randr_and_warns_about_vrr(
    tmp_path: Path,
) -> None:
    env = make_env(tmp_path, "mmsg")
    outputs = load_outputs()
    wlr = FakeWlr(outputs)
    backend = Backend(wlr, Mango(env=env))
    state = outputs[0].state()
    state.adaptive_sync = True
    state.hdr = True
    recorder = Recorder()
    with patch("subprocess.run", side_effect=recorder):
        results = backend.apply_states([state])

    assert wlr.applied == ["HDMI-A-1"]
    assert ["dispatch", "togglehdr,on,HDMI-A-1"] in [
        cmd[1:] for cmd in recorder.commands
    ]
    warnings = [result.warning for result in results if result.warning]
    assert (
        warnings and "source_optional=~/.config/mango/wlr-randr-tui.conf" in warnings[0]
    )
    # The include is still written so it can be sourced later.
    assert (config_dir(tmp_path) / "mango" / "wlr-randr-tui.conf").exists()


def test_missing_ipc_tool_falls_back_to_wlr_randr(tmp_path: Path) -> None:
    env = make_env(tmp_path)
    outputs = load_outputs()
    wlr = FakeWlr(outputs)
    results = Backend(wlr, Sway(env=env)).apply_states([outputs[0].state()])
    assert all(result.ok for result in results)
    assert wlr.applied == ["HDMI-A-1"]
    assert any("swaymsg was not found" in result.warning for result in results)
    assert (config_dir(tmp_path) / "sway" / "wlr-randr-tui.conf").exists()


def test_include_write_failure_is_a_warning_after_live_apply(tmp_path: Path) -> None:
    env = make_env(tmp_path, "swaymsg")
    outputs = load_outputs()
    sway = Sway(env=env)
    path = sway.include_path()
    assert path is not None
    path.parent.mkdir(parents=True)
    path.symlink_to(tmp_path / "elsewhere")
    with patch("subprocess.run", side_effect=Recorder()):
        results = Backend(FakeWlr(outputs), sway).apply_states([outputs[0].state()])
    assert all(result.ok for result in results)
    assert any("symlink" in result.warning for result in results)


def test_backend_write_include_uses_live_layout(tmp_path: Path) -> None:
    env = make_env(tmp_path)
    backend = Backend(FakeWlr(), Niri(env=env))
    path = backend.write_include(profile="init")
    assert path is not None and path.exists()
    assert os.access(path, os.W_OK)
    assert "// Profile: init" in path.read_text()


# ----------------------------------------------------------------------
# Review follow-ups: retention, identities, preserved options, validation
# ----------------------------------------------------------------------


def test_disconnected_outputs_keep_their_rules(tmp_path: Path) -> None:
    env = make_env(tmp_path)
    mango = Mango(env=env)
    docked = load_outputs()
    docked[0].x = 1600  # external monitor right of the laptop
    mango.write_include(docked)

    # Undocked: only eDP-1 is connected, the user moves it and applies.
    laptop = [load_outputs()[1]]
    laptop[0].x = 0
    mango.write_include(laptop)
    text = (config_dir(tmp_path) / "mango" / "wlr-randr-tui.conf").read_text()
    assert "name:^eDP-1$,width:1920,height:1080,refresh:60,x:0," in text
    assert "serial:PC3M394200857,width:1920,height:1080,refresh:100,x:1600," in text

    # The retained monitor is reconnected on another connector: no duplicate.
    replugged = load_outputs()
    replugged[0].name = "DP-7"
    mango.write_include(replugged)
    text = (config_dir(tmp_path) / "mango" / "wlr-randr-tui.conf").read_text()
    assert text.count("serial:PC3M394200857") == 1


def test_identical_monitors_fall_back_to_connector_names(tmp_path: Path) -> None:
    first, second = load_outputs()
    second.make, second.model, second.serial = first.make, first.model, first.serial
    for compositor in (Sway, Niri, Mango):
        text = compositor(env=make_env(tmp_path)).render_include([first, second])
        assert "PC3M394200857" not in text, compositor.name
        assert "HDMI-A-1" in text and "eDP-1" in text


def test_mango_keeps_hand_added_rule_options(tmp_path: Path) -> None:
    mango = Mango(env=make_env(tmp_path))
    outputs = load_outputs()
    path = mango.write_include(outputs)
    assert path is not None
    lines = [
        line + ",hdr_max_lum:616,primary:1" if "PC3M394200857" in line else line
        for line in path.read_text().splitlines()
    ]
    path.write_text("\n".join(lines) + "\n")
    outputs[0].x = 64
    mango.write_include(outputs)
    line = next(
        line for line in path.read_text().splitlines() if "PC3M394200857" in line
    )
    assert line.endswith(",hdr_max_lum:616,primary:1")
    assert ",x:64," in line
    assert line.count("vrr:") == 1


def test_niri_keeps_hand_added_nodes_and_on_demand_vrr(tmp_path: Path) -> None:
    env = make_env(tmp_path, "niri")
    niri = Niri(env=env)
    outputs = load_outputs()
    path = niri.write_include(outputs)
    assert path is not None
    text = path.read_text().replace(
        "    position x=1920 y=0\n    variable-refresh-rate\n",
        "    position x=1920 y=0\n"
        "    variable-refresh-rate on-demand=true\n"
        "    focus-at-startup\n"
        "    layout {\n"
        "        gaps 4\n"
        "    }\n",
    )
    path.write_text(text)

    preserved = niri.preserved_options()
    assert preserved["eDP-1"] == (
        "    variable-refresh-rate on-demand=true",
        "    focus-at-startup",
        "    layout {",
        "        gaps 4",
        "    }",
    )

    # niri reports vrr_enabled=false while no VRR window is focused.
    payload = json.dumps({"eDP-1": {"name": "eDP-1", "vrr_enabled": False}})
    fresh = load_outputs()
    with patch("subprocess.run", side_effect=Recorder({"--json": payload})):
        niri.enrich(fresh)
    assert fresh[1].adaptive_sync is True

    recorder = Recorder({"--json": payload})
    with patch("subprocess.run", side_effect=recorder):
        Backend(FakeWlr(fresh), niri).apply_states([fresh[1].state()])
    assert ["output", "eDP-1", "vrr", "on", "--on-demand"] in [
        cmd[2:] for cmd in recorder.commands
    ]
    block = path.read_text().split('output "eDP-1" {', 1)[1]
    assert "variable-refresh-rate on-demand=true" in block
    assert "\n    variable-refresh-rate\n" not in block
    assert "gaps 4" in block

    # Turning VRR off in the TUI drops the on-demand line too.
    fresh[1].adaptive_sync = False
    niri.write_include(fresh)
    assert "variable-refresh-rate" not in path.read_text().split('"eDP-1"', 1)[1]


def test_invalid_profile_values_never_reach_commands(tmp_path: Path) -> None:
    state = load_outputs()[0].state()
    state.transform = "normal; exec rm -rf ~"
    for compositor in (Sway(env=make_env(tmp_path)), Niri(env=make_env(tmp_path))):
        with pytest.raises(CompositorError, match="invalid transform"):
            compositor.check_state(state)
    with pytest.raises(CompositorError):
        Sway(env=make_env(tmp_path)).output_args(state)

    good = load_outputs()[0].state()
    good.color_profile = "srgb; exec evil"
    assert "exec" not in Sway(env=make_env(tmp_path)).output_args(good)


def test_capabilities_follow_the_compositor_version(tmp_path: Path) -> None:
    env = make_env(tmp_path, "swaymsg", "niri")
    old_sway = json.dumps([{"name": "HDMI-A-1", "adaptive_sync_status": "disabled"}])
    sway = Sway(env=env)
    with patch("subprocess.run", side_effect=Recorder({"get_outputs": old_sway})):
        sway.enrich(load_outputs())
    assert sway.capabilities.hdr is False
    assert sway.capabilities.color_presets == ()
    assert sway.capabilities.icc is True
    assert Sway.default_capabilities.hdr is True

    old_niri = json.dumps({"eDP-1": {"name": "eDP-1", "vrr_enabled": True}})
    niri = Niri(env=env)
    with patch("subprocess.run", side_effect=Recorder({"--json": old_niri})):
        niri.enrich(load_outputs())
    assert niri.capabilities.bit_depths == ()


def test_mango_paths_follow_mango_not_xdg(tmp_path: Path) -> None:
    env = make_env(tmp_path)
    env["XDG_CONFIG_HOME"] = str(tmp_path / "elsewhere")
    mango = Mango(env=env)
    mango_dir = tmp_path / "home" / ".config" / "mango"
    assert mango.include_path() == mango_dir / "wlr-randr-tui.conf"
    (mango_dir / "conf.d").mkdir(parents=True)
    (mango_dir / "config.conf").write_text("source=./conf.d/monitors.conf\n")
    # "./" is relative to ~/.config/mango, not to the including file.
    (mango_dir / "conf.d" / "monitors.conf").write_text(
        "source_optional=./wlr-randr-tui.conf\n"
    )
    assert mango.include_is_sourced()


# ----------------------------------------------------------------------
# Consistency fixes
# ----------------------------------------------------------------------


def test_niri_config_never_gets_max_bpc_the_compositor_lacks(tmp_path: Path) -> None:
    niri = Niri(env=make_env(tmp_path))
    hdmi = load_outputs()[0]
    hdmi.bit_depth = 10
    assert "    max-bpc 10" in niri.render_output(hdmi)
    # niri <= 25.11: the live command and the config agree.
    niri.capabilities = replace(niri.default_capabilities, bit_depths=())
    assert not any("max-bpc" in line for line in niri.render_output(hdmi))
    assert not any(cmd[0] == "max-bpc" for cmd in niri.output_commands(hdmi.state()))


def test_deleting_the_include_forgets_remembered_state(tmp_path: Path) -> None:
    mango = Mango(env=make_env(tmp_path))
    outputs = load_outputs()
    outputs[0].color_profile = "icc:/x.icc"
    path = mango.write_include(outputs)
    assert path is not None
    path.unlink()
    assert mango.read_include_state() == {}
    mango.write_include([load_outputs()[1]])
    text = path.read_text()
    assert "PC3M394200857" not in text  # the disconnected monitor is gone
    fresh = load_outputs()
    mango.enrich(fresh)
    assert fresh[0].color_profile is None


def test_custom_include_paths_get_their_own_state(tmp_path: Path) -> None:
    env = make_env(tmp_path)
    default = Sway(env=env)
    custom = Sway(env=env, include_path=tmp_path / "other.conf")
    assert default.state_path() != custom.state_path()
    default.write_include(load_outputs())
    # Nothing from the default include leaks into the custom one.
    custom.write_include([load_outputs()[1]])
    assert "PC3M394200857" not in (tmp_path / "other.conf").read_text()
    assert [output.name for output in custom.retained_outputs([])] == ["eDP-1"]


def test_niri_validates_every_output_before_running_anything(tmp_path: Path) -> None:
    env = make_env(tmp_path, "niri")
    outputs = load_outputs()
    good = outputs[0].state()
    bad = outputs[1].state()
    bad.transform = "sideways"
    recorder = Recorder()
    with (
        patch("subprocess.run", side_effect=recorder),
        pytest.raises(CompositorError, match="invalid transform"),
    ):
        Niri(env=env).apply(FakeWlr(outputs), [good, bad], outputs)
    assert recorder.commands == []


def test_commands_run_with_the_compositor_environment(tmp_path: Path) -> None:
    env = make_env(tmp_path, "swaymsg")
    env["SWAYSOCK"] = "/run/user/1000/sway-test.sock"
    outputs = load_outputs()
    seen: list[dict[str, str] | None] = []

    def fake_run(command, **kwargs):  # type: ignore[no-untyped-def]
        seen.append(kwargs.get("env"))
        return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

    with patch("subprocess.run", side_effect=fake_run):
        Sway(env=env).apply(FakeWlr(outputs), [outputs[0].state()], [])
    assert seen and all(
        item is not None and item["SWAYSOCK"] == env["SWAYSOCK"] for item in seen
    )


def test_mango_without_include_explains_no_include(tmp_path: Path) -> None:
    env = make_env(tmp_path, "mmsg")
    outputs = load_outputs()
    state = outputs[0].state()
    state.adaptive_sync = True
    with patch("subprocess.run", side_effect=Recorder()):
        results = Backend(
            FakeWlr(outputs), Mango(env=env, write_include=False)
        ).apply_states([state])
    warnings = [result.warning for result in results if result.warning]
    assert any("--no-include" in warning for warning in warnings)
    assert not any("source_optional" in warning for warning in warnings)


def test_mango_reloads_even_if_only_the_state_file_fails(tmp_path: Path) -> None:
    env = make_env(tmp_path, "mmsg")
    (config_dir(tmp_path) / "mango").mkdir(parents=True)
    (config_dir(tmp_path) / "mango" / "config.conf").write_text(
        "source_optional=./wlr-randr-tui.conf\n"
    )
    # A file where the state directory should be makes the sidecar unwritable.
    (tmp_path / "state").write_text("")
    outputs = load_outputs()
    recorder = Recorder()
    with patch("subprocess.run", side_effect=recorder):
        results = Backend(FakeWlr(outputs), Mango(env=env)).apply_states(
            [outputs[0].state()]
        )
    assert all(result.ok for result in results)
    assert any("will not be remembered" in result.warning for result in results)
    assert ["dispatch", "reload_config"] in [cmd[1:] for cmd in recorder.commands]
    assert (config_dir(tmp_path) / "mango" / "wlr-randr-tui.conf").exists()


def test_drop_unsupported_clears_foreign_profile_settings(tmp_path: Path) -> None:
    output = load_outputs()[0]
    output.hdr = True
    output.bit_depth = 10
    output.color_profile = "srgb"
    Compositor(env=make_env(tmp_path)).drop_unsupported([output])
    assert (output.hdr, output.bit_depth, output.color_profile) == (None, None, None)


# ----------------------------------------------------------------------
# Second review: twins, --no-include, ICC paths, relative --include
# ----------------------------------------------------------------------


def twins() -> list[Output]:
    """Two identical monitors that report the same EDID serial."""
    first, second = load_outputs()
    second.make, second.model, second.serial = first.make, first.model, first.serial
    return [first, second]


def test_unplugged_twin_keeps_its_own_rule(tmp_path: Path) -> None:
    sway = Sway(env=make_env(tmp_path))
    sway.write_include(twins())
    sway.write_include(twins()[:1])  # unplug the second monitor and apply
    targets = [
        line.split(" enable")[0]
        for line in sway.include_path().read_text().splitlines()  # type: ignore[union-attr]
        if line.startswith("output")
    ]
    # Both rules stay, matched by connector, so the twin is not merged in.
    assert targets == ['output "HDMI-A-1"', 'output "eDP-1"']


def test_moved_unique_monitor_is_not_duplicated(tmp_path: Path) -> None:
    sway = Sway(env=make_env(tmp_path))
    sway.write_include(load_outputs())
    moved = load_outputs()
    moved[0].name = "DP-7"  # same monitor, new connector
    sway.write_include(moved)
    text = sway.include_path().read_text()  # type: ignore[union-attr]
    assert text.count("PC3M394200857") == 1


def test_sidecar_lookup_skips_ambiguous_twins(tmp_path: Path) -> None:
    sway = Sway(env=make_env(tmp_path))
    outputs = twins()
    outputs[0].color_profile = "srgb"
    sway.write_include(outputs)
    renamed = twins()
    renamed[0].name, renamed[1].name = "DP-1", "DP-2"
    sway.enrich(renamed)
    # Neither twin can be identified by EDID, so neither gets srgb.
    assert [output.color_profile for output in renamed] == [None, None]


def test_no_include_ignores_old_state(tmp_path: Path) -> None:
    env = make_env(tmp_path)
    outputs = load_outputs()
    outputs[0].color_profile = "srgb"
    Sway(env=env).write_include(outputs)
    fresh = load_outputs()
    Sway(env=env, write_include=False).enrich(fresh)
    assert fresh[0].color_profile is None


def test_icc_profiles_must_be_absolute(tmp_path: Path) -> None:
    for compositor in (Sway(env=make_env(tmp_path)), Mango(env=make_env(tmp_path))):
        assert compositor.color_profile_supported("icc:/usr/share/x.icc")
        assert not compositor.color_profile_supported("icc:~/x.icc")
        assert not compositor.color_profile_supported("icc:x.icc")


def test_relative_include_paths_are_made_absolute(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    sway = Sway(env=make_env(tmp_path), include_path="outputs.conf")
    assert sway.include_path() == tmp_path / "outputs.conf"
    assert sway.include_line() == f"include {tmp_path / 'outputs.conf'}"


def test_state_file_warning_does_not_claim_an_apply(tmp_path: Path) -> None:
    env = make_env(tmp_path)
    (tmp_path / "state").write_text("")  # makes the state dir unwritable
    with pytest.raises(StateFileError) as excinfo:
        Niri(env=env).write_include(load_outputs())
    assert "include was written" in str(excinfo.value)
    assert "applied" not in str(excinfo.value)
