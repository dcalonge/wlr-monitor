from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

from wlr_monitor.model import Mode, Output, OutputState
from wlr_monitor.wlr import WlrRandr

FIXTURES = Path(__file__).parent / "fixtures"


def load_outputs() -> list[Output]:
    data = json.loads((FIXTURES / "outputs.json").read_text())
    return [Output.from_json(item) for item in data]


def test_command_for_enabled_output() -> None:
    backend = WlrRandr(sys.executable)
    output = load_outputs()[0]
    state = output.state()
    command = backend.command_for(state)
    assert command[:3] == [sys.executable, "--output", "HDMI-A-1"]
    assert "--on" in command
    assert command[command.index("--mode") + 1] == "1920x1080@100"
    assert command[command.index("--pos") + 1] == "0,0"
    assert command[command.index("--scale") + 1] == "1"
    assert command[command.index("--transform") + 1] == "normal"
    assert command[command.index("--adaptive-sync") + 1] == "disabled"


def test_command_for_disabled_output_only_turns_it_off() -> None:
    backend = WlrRandr(sys.executable)
    output = load_outputs()[0]
    state = output.state()
    state.enabled = False
    assert backend.command_for(state) == [
        sys.executable,
        "--output",
        "HDMI-A-1",
        "--off",
    ]


def test_read_outputs() -> None:
    payload = (FIXTURES / "outputs.json").read_text()
    completed = subprocess.CompletedProcess([], 0, stdout=payload, stderr="")
    with patch("subprocess.run", return_value=completed):
        outputs = WlrRandr(sys.executable).read_outputs()
    assert [output.name for output in outputs] == ["HDMI-A-1", "eDP-1"]


def test_read_outputs_reports_bad_json() -> None:
    completed = subprocess.CompletedProcess([], 0, stdout="not json", stderr="")
    with (
        patch("subprocess.run", return_value=completed),
        pytest.raises(Exception, match="invalid JSON"),
    ):
        WlrRandr(sys.executable).read_outputs()


def test_apply_states_collects_failures() -> None:
    def fake_run(
        command: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        if "eDP-1" in command:
            return subprocess.CompletedProcess(command, 1, stdout="", stderr="bad mode")
        return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

    outputs = load_outputs()
    with patch("subprocess.run", side_effect=fake_run):
        results = WlrRandr(sys.executable).apply_states(
            [output.state() for output in outputs]
        )

    assert [result.ok for result in results] == [True, False]
    assert results[1].detail == "bad mode"


def test_state_with_no_mode_is_valid() -> None:
    state = OutputState(
        name="DP-1",
        enabled=False,
        mode=None,
        x=0,
        y=0,
        scale=1.0,
        transform="normal",
        adaptive_sync=False,
    )
    assert state.to_dict()["mode"] is None
    restored = OutputState.from_dict("DP-1", state.to_dict())
    assert restored.mode is None


def test_mode_cli_value_keeps_precision() -> None:
    state = OutputState(
        name="DP-1",
        enabled=True,
        mode=Mode(width=2560, height=1440, refresh=59.950001),
        x=0,
        y=0,
        scale=1.0,
        transform="normal",
        adaptive_sync=False,
    )
    command = WlrRandr(sys.executable).command_for(state)
    assert command[command.index("--mode") + 1] == "2560x1440@59.950001"
