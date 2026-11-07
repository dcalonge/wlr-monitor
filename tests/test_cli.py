from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from unittest.mock import patch

from wlr_monitor import cli
from wlr_monitor.model import Output, OutputState
from wlr_monitor.profiles import ProfileStore
from wlr_monitor.wlr import ApplyResult, WlrRandrError

FIXTURES = Path(__file__).parent / "fixtures"


def load_outputs() -> list[Output]:
    data = json.loads((FIXTURES / "outputs.json").read_text())
    return [Output.from_json(item) for item in data]


def make_store(path: Path) -> ProfileStore:
    store = ProfileStore(path)
    outputs = load_outputs()
    store.save("work", [output.state() for output in outputs])
    moved = [output.state() for output in outputs]
    moved[0].x = 128
    store.save("home", moved)
    return store


def test_parser_accepts_hyprmon_compatible_flags(tmp_path: Path) -> None:
    parser = cli.build_parser()
    args = parser.parse_args(
        [
            "--cfg",
            str(tmp_path / "cfg"),
            "--list-profiles",
            "--json",
        ]
    )
    assert args.config == tmp_path / "cfg"
    assert args.list_profiles and args.json

    args = parser.parse_args(["profiles"])
    assert args.command == "profiles"

    args = parser.parse_args(["--active-profile"])
    assert args.active_profile


def test_config_path_accepts_files_and_directories(tmp_path: Path) -> None:
    assert cli._config_path(None) is None
    assert cli._config_path(tmp_path / "profiles.json") == (tmp_path / "profiles.json")
    assert cli._config_path(tmp_path) == tmp_path / "profiles.json"

    directory = tmp_path / "cfg"
    directory.mkdir()
    assert cli._config_path(directory) == directory / "profiles.json"

    explicit = tmp_path / "custom"
    explicit.write_text("{}")
    assert cli._config_path(explicit) == explicit


def test_list_profiles_text_marks_active(tmp_path: Path, capsys) -> None:
    path = tmp_path / "profiles.json"
    make_store(path)

    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()):
        assert cli.main(["--config", str(path), "--list-profiles"]) == 0

    assert capsys.readouterr().out == "work *\nhome\n"


def test_list_profiles_json_matches_hyprmon_shape(tmp_path: Path, capsys) -> None:
    path = tmp_path / "profiles.json"
    make_store(path)

    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()):
        assert cli.main(["--config", str(path), "--list-profiles", "--json"]) == 0

    payload = json.loads(capsys.readouterr().out)
    assert payload == [
        {"name": "work", "is_active": True},
        {"name": "home", "is_active": False},
    ]


def test_list_profiles_still_works_without_wlr_randr(tmp_path: Path, capsys) -> None:
    path = tmp_path / "profiles.json"
    make_store(path)

    with patch.object(
        cli.WlrRandr, "read_outputs", side_effect=WlrRandrError("no backend")
    ):
        assert cli.main(["--config", str(path), "--list-profiles"]) == 0

    assert capsys.readouterr().out == "work\nhome\n"


def test_active_profile_output(tmp_path: Path, capsys) -> None:
    path = tmp_path / "profiles.json"
    make_store(path)

    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()):
        assert cli.main(["--config", str(path), "--active-profile"]) == 0
    assert capsys.readouterr().out == "work\n"

    moved = load_outputs()
    moved[0].x = 500
    with patch.object(cli.WlrRandr, "read_outputs", return_value=moved):
        assert cli.main(["--config", str(path), "--active-profile"]) == 0
    assert capsys.readouterr().out == "No active profile found\n"


def test_profiles_command_launches_browser(tmp_path: Path) -> None:
    captured: dict[str, object] = {}

    class DummyApp:
        def __init__(self, **kwargs: object) -> None:
            captured.update(kwargs)

        def run(self) -> None:
            captured["ran"] = True

    with (
        patch("wlr_monitor.app.WlrMonitorApp", DummyApp),
        patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()),
    ):
        assert cli.main(["profiles", "--config", str(tmp_path / "p.json")]) == 0

    assert captured["profiles_only"] is True
    assert captured["ran"] is True


def test_profiles_flag_launches_browser(tmp_path: Path) -> None:
    captured: dict[str, object] = {}

    class DummyApp:
        def __init__(self, **kwargs: object) -> None:
            captured.update(kwargs)

        def run(self) -> None:
            captured["ran"] = True

    with (
        patch("wlr_monitor.app.WlrMonitorApp", DummyApp),
        patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()),
    ):
        assert cli.main(["--profiles", "--config", str(tmp_path / "p.json")]) == 0

    assert captured["profiles_only"] is True


def test_tui_reads_outputs_while_the_app_starts(tmp_path: Path) -> None:
    """Outputs are read in the background and handed to the app."""
    captured: dict[str, object] = {}

    class DummyApp:
        def __init__(self, **kwargs: object) -> None:
            captured.update(kwargs)

        def run(self) -> None:
            pass

    with (
        patch("wlr_monitor.app.WlrMonitorApp", DummyApp),
        patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()),
    ):
        assert cli.main(["--config", str(tmp_path / "p.json")]) == 0
        pending = captured["pending_outputs"]
        outputs = pending.result()  # type: ignore[attr-defined]

    assert [output.name for output in outputs] == ["HDMI-A-1", "eDP-1"]


def test_one_shot_commands_do_not_import_textual(tmp_path: Path) -> None:
    code = (
        "import sys; from wlr_monitor import cli; "
        f"cli.main(['--config', {str(tmp_path / 'p.json')!r}, '--list-profiles']); "
        "sys.exit('textual' in sys.modules)"
    )
    src = Path(cli.__file__).resolve().parents[1]
    completed = subprocess.run(
        [sys.executable, "-c", code],
        env={"PYTHONPATH": str(src), "PATH": ""},
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr


def test_apply_profile_uses_hardware_id_fallback(tmp_path: Path, capsys) -> None:
    path = tmp_path / "profiles.json"
    store = ProfileStore(path)
    saved = load_outputs()[0].state()
    saved.name = "DP-9"
    saved.hardware_id = "Microstep/MSI MP275/PC3M394200857"
    store.save("replugged", [saved])

    outputs = load_outputs()[:1]
    applied: list[str] = []

    def fake_apply(states):  # type: ignore[no-untyped-def]
        applied.extend(state.name for state in states)
        return [ApplyResult(name=state.name, command=[], ok=True) for state in states]

    with (
        patch.object(cli.WlrRandr, "read_outputs", return_value=outputs),
        patch.object(cli.WlrRandr, "apply_states", side_effect=fake_apply),
    ):
        assert cli.main(["--config", str(path), "--profile", "replugged"]) == 0

    assert applied == ["HDMI-A-1"]
    assert "Applied HDMI-A-1." in capsys.readouterr().out


def test_delete_profile_command(tmp_path: Path, capsys) -> None:
    path = tmp_path / "profiles.json"
    make_store(path)
    assert cli.main(["--config", str(path), "--delete-profile", "work"]) == 0
    assert ProfileStore(path).names() == ["home"]
    assert "Deleted profile" in capsys.readouterr().out


def test_outputs_json_includes_state(tmp_path: Path, capsys) -> None:
    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()):
        assert cli.main(["--outputs", "--json"]) == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload[0]["name"] == "HDMI-A-1"
    assert payload[0]["position"] == {"x": 0, "y": 0}
    assert payload[0]["hardware_id"]


def test_print_config_renders_profile_for_compositor(tmp_path: Path, capsys) -> None:
    path = tmp_path / "profiles.json"
    make_store(path)

    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()):
        assert (
            cli.main(
                [
                    "--config",
                    str(path),
                    "--compositor",
                    "mango",
                    "--print-config",
                    "home",
                ]
            )
            == 0
        )
    out = capsys.readouterr().out
    assert "# Profile: home" in out
    assert "monitor_rule=make:Microstep,model:MSI MP275,serial:PC3M394200857," in out
    assert "x:128," in out


def test_print_config_needs_a_compositor(capsys) -> None:
    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()):
        assert cli.main(["--print-config"]) == 1
    assert "--compositor" in capsys.readouterr().err


def test_init_include_writes_file_and_prints_line(tmp_path: Path, capsys) -> None:
    include = tmp_path / "out" / "outputs.conf"
    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()):
        assert (
            cli.main(
                ["--compositor", "sway", "--include", str(include), "--init-include"]
            )
            == 0
        )
    assert 'output "eDP-1" enable' in include.read_text()
    out = capsys.readouterr().out
    assert f"include {include}" in out


def test_apply_profile_writes_compositor_include(tmp_path: Path, capsys) -> None:
    path = tmp_path / "profiles.json"
    make_store(path)
    include = tmp_path / "niri.kdl"
    with (
        patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()),
        patch.object(
            cli.WlrRandr,
            "apply_states",
            side_effect=lambda states: [
                ApplyResult(name=state.name, command=[], ok=True) for state in states
            ],
        ),
    ):
        # The generic backend applies through wlr-randr and writes nothing.
        assert cli.main(["--config", str(path), "--profile", "home"]) == 0
    assert not include.exists()

    # niri without `niri` on PATH falls back to wlr-randr and still persists.
    with (
        patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()),
        patch.object(
            cli.WlrRandr,
            "apply_states",
            side_effect=lambda states: [
                ApplyResult(name=state.name, command=[], ok=True) for state in states
            ],
        ),
        patch("shutil.which", return_value=None),
    ):
        code = cli.main(
            [
                "--config",
                str(path),
                "--compositor",
                "niri",
                "--include",
                str(include),
                "--profile",
                "home",
            ]
        )
    assert code == 0
    assert "niri was not found" in capsys.readouterr().err
    assert "position x=128 y=0" in include.read_text()


def test_print_config_matches_what_would_be_written(tmp_path: Path, capsys) -> None:
    include = tmp_path / "mango.conf"
    args = ["--compositor", "mango", "--include", str(include)]
    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()):
        assert cli.main([*args, "--init-include"]) == 0
    capsys.readouterr()

    # Undocked: only eDP-1 is connected, but HDMI-A-1 keeps its rule.
    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()[1:]):
        assert cli.main([*args, "--print-config"]) == 0
    printed = capsys.readouterr().out
    assert "serial:PC3M394200857" in printed
    assert "name:^eDP-1$" in printed


def test_profile_settings_the_compositor_lacks_are_dropped(
    tmp_path: Path, capsys
) -> None:
    path = tmp_path / "profiles.json"
    store = ProfileStore(path)
    state = load_outputs()[0].state()
    state.hdr = True
    state.color_profile = "srgb"
    store.save("hdr", [state])
    applied: list[OutputState] = []

    def fake_apply(states):  # type: ignore[no-untyped-def]
        applied.extend(states)
        return [ApplyResult(name=item.name, command=[], ok=True) for item in states]

    with (
        patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()[:1]),
        patch.object(cli.WlrRandr, "apply_states", side_effect=fake_apply),
    ):
        assert cli.main(["--config", str(path), "--profile", "hdr"]) == 0
    assert applied[0].hdr is None
    assert applied[0].color_profile is None


def test_list_profiles_reports_broken_ones(tmp_path: Path, capsys) -> None:
    path = tmp_path / "profiles.json"
    make_store(path)
    raw = json.loads(path.read_text())
    raw["profiles"]["home"]["outputs"]["HDMI-A-1"]["transform"] = "bogus"
    path.write_text(json.dumps(raw))
    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()):
        assert cli.main(["--config", str(path), "--list-profiles"]) == 0
    captured = capsys.readouterr()
    assert captured.out == "work *\n"
    assert "invalid transform" in captured.err
    with patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()):
        assert cli.main(["--config", str(path), "--profile", "home"]) == 1
    assert "invalid transform" in capsys.readouterr().err


def test_cli_refuses_profiles_that_turn_everything_off(tmp_path: Path, capsys) -> None:
    path = tmp_path / "profiles.json"
    store = ProfileStore(path)
    dark = [output.state() for output in load_outputs()]
    for state in dark:
        state.enabled = False
    store.save("dark", dark)
    with (
        patch.object(cli.WlrRandr, "read_outputs", return_value=load_outputs()),
        patch.object(cli.WlrRandr, "apply_states") as apply,
    ):
        assert cli.main(["--config", str(path), "--profile", "dark"]) == 1
    apply.assert_not_called()
    assert "every connected monitor" in capsys.readouterr().err
