from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import patch

from wlr_randr_tui import cli
from wlr_randr_tui.model import Output
from wlr_randr_tui.profiles import ProfileStore
from wlr_randr_tui.wlr import ApplyResult, WlrRandrError

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

    with patch.object(cli, "WlrRandrApp", DummyApp):
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

    with patch.object(cli, "WlrRandrApp", DummyApp):
        assert cli.main(["--profiles", "--config", str(tmp_path / "p.json")]) == 0

    assert captured["profiles_only"] is True


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
