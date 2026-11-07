"""Thin wrapper around the wlr-randr command line tool."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from .model import Output, OutputState


class WlrRandrError(RuntimeError):
    """Raised when wlr-randr cannot be run or reports an error."""


@dataclass
class ApplyResult:
    name: str
    command: list[str]
    ok: bool
    stdout: str = ""
    stderr: str = ""
    error: str = ""
    warning: str = ""

    @property
    def detail(self) -> str:
        if self.error:
            return self.error
        if self.stderr.strip():
            return self.stderr.strip()
        return self.stdout.strip()


class WlrRandr:
    """Read and apply output state through wlr-randr."""

    def __init__(self, binary: str | None = None) -> None:
        self.binary = (
            binary
            or os.environ.get("WLR_RANDR")
            or shutil.which("wlr-randr")
            or "wlr-randr"
        )

    def ensure_available(self) -> None:
        path = shutil.which(self.binary)
        if path is None and not Path(self.binary).exists():
            raise WlrRandrError(
                "wlr-randr was not found. Install wlr-randr and make sure it is on PATH."
            )

    def read_outputs(self) -> list[Output]:
        self.ensure_available()
        stdout = self._run([self.binary, "--json"])
        try:
            data = json.loads(stdout)
        except json.JSONDecodeError as exc:
            raise WlrRandrError(f"wlr-randr returned invalid JSON: {exc}") from exc
        if not isinstance(data, list):
            raise WlrRandrError("wlr-randr returned an unexpected JSON document.")
        return [Output.from_json(item) for item in data]

    def command_for(self, state: OutputState) -> list[str]:
        command = [self.binary, "--output", state.name]
        if not state.enabled:
            command.append("--off")
            return command

        command.append("--on")
        if state.mode is not None:
            command.extend(["--mode", state.mode.cli_value()])
        command.extend(["--pos", f"{state.x},{state.y}"])
        command.extend(["--scale", format_number(state.scale)])
        command.extend(["--transform", state.transform])
        command.extend(
            [
                "--adaptive-sync",
                "enabled" if state.adaptive_sync else "disabled",
            ]
        )
        return command

    def apply_states(self, states: Iterable[OutputState]) -> list[ApplyResult]:
        self.ensure_available()
        return [
            run_apply_command(state.name, self.command_for(state))
            for state in apply_order(states)
        ]

    def _run(self, command: Sequence[str]) -> str:
        try:
            completed = subprocess.run(
                list(command),
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
        except FileNotFoundError as exc:
            raise WlrRandrError(
                "wlr-randr was not found. Install wlr-randr and make sure it is on PATH."
            ) from exc
        except subprocess.TimeoutExpired as exc:
            raise WlrRandrError("wlr-randr timed out.") from exc
        if completed.returncode != 0:
            detail = (
                completed.stderr.strip() or completed.stdout.strip() or "unknown error"
            )
            raise WlrRandrError(f"wlr-randr failed: {detail}")
        return completed.stdout


def format_number(value: float) -> str:
    """Format a float without trailing zeros (``1.0`` → ``1``)."""
    text = f"{value:.6f}".rstrip("0").rstrip(".")
    return text or "0"


def apply_order(states: Iterable[OutputState]) -> list[OutputState]:
    """Enable outputs before disabling others, so a session never has none."""
    items = list(states)
    return [state for state in items if state.enabled] + [
        state for state in items if not state.enabled
    ]


def run_apply_command(
    name: str,
    command: list[str],
    *,
    timeout: float = 15,
    error_markers: tuple[str, ...] = (),
    env: Mapping[str, str] | None = None,
) -> ApplyResult:
    """Run one configuration command and wrap the outcome in an ApplyResult.

    ``error_markers`` catches tools that report failures on stdout while still
    exiting with status 0 (mango's ``mmsg`` does this). ``env`` defaults to
    the current process environment.
    """
    tool = Path(command[0]).name if command else "command"
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
            env=dict(env) if env is not None else None,
        )
    except FileNotFoundError as exc:
        return ApplyResult(name=name, command=command, ok=False, error=str(exc))
    except subprocess.TimeoutExpired:
        return ApplyResult(
            name=name, command=command, ok=False, error=f"{tool} timed out"
        )
    output = f"{completed.stdout}\n{completed.stderr}"
    ok = completed.returncode == 0 and not any(
        marker in output for marker in error_markers
    )
    return ApplyResult(
        name=name,
        command=command,
        ok=ok,
        stdout=completed.stdout,
        stderr=completed.stderr,
    )
