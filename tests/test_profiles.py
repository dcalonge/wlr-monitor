from __future__ import annotations

import json
from pathlib import Path

import pytest

from wlr_randr_tui.model import Mode, OutputState
from wlr_randr_tui.profiles import ProfileError, ProfileStore

FIXTURES = Path(__file__).parent / "fixtures"


def sample_state(
    name: str = "HDMI-A-1",
    *,
    x: int = 0,
    scale: float = 1.0,
    hardware_id: str = "",
) -> OutputState:
    return OutputState(
        name=name,
        enabled=True,
        mode=Mode(width=1920, height=1080, refresh=100.0),
        x=x,
        y=0,
        scale=scale,
        transform="normal",
        adaptive_sync=False,
        hardware_id=hardware_id,
        description=f"{name} description",
    )


def test_missing_file_has_no_profiles(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    assert store.load() == {}
    assert store.names() == []


def test_profile_round_trip(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    store = ProfileStore(path)
    state = sample_state(hardware_id="HW/1")
    store.save("work", [state])

    assert path.exists()
    assert store.names() == ["work"]
    profile = store.get("work")
    assert profile is not None
    assert profile.outputs["HDMI-A-1"].to_dict() == state.to_dict()
    assert profile.created_at
    assert profile.updated_at

    raw = json.loads(path.read_text())
    assert raw["version"] == 2
    assert raw["order"] == ["work"]
    assert raw["profiles"]["work"]["outputs"]["HDMI-A-1"]["mode"]["width"] == 1920
    assert raw["profiles"]["work"]["outputs"]["HDMI-A-1"]["hardware_id"] == "HW/1"


def test_update_existing_profile_preserves_created_at(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    store.save("work", [sample_state()])
    first = store.get("work")
    assert first is not None
    changed = sample_state(x=1920, scale=1.5)
    store.save("work", [changed])
    profile = store.get("work")
    assert profile is not None
    assert profile.outputs["HDMI-A-1"].x == 1920
    assert profile.outputs["HDMI-A-1"].scale == 1.5
    assert profile.created_at == first.created_at


def test_each_write_keeps_a_timestamped_backup(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    store = ProfileStore(path)
    store.save("work", [sample_state()])
    store.save("home", [sample_state(x=100)])
    assert list(tmp_path.glob("profiles.json.bak.*"))


def test_delete_profile(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    store.save("work", [sample_state()])
    assert store.delete("work")
    assert not store.delete("work")
    assert store.names() == []


def test_empty_name_is_rejected(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    with pytest.raises(ProfileError):
        store.save("   ", [sample_state()])


def test_reserved_name_is_rejected(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    with pytest.raises(ProfileError):
        store.save("__save__", [sample_state()])


def test_invalid_json_reports_error(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    path.write_text("{ nope")
    store = ProfileStore(path)
    with pytest.raises(ProfileError):
        store.load()


def test_profile_file_is_private(tmp_path: Path) -> None:
    path = tmp_path / "profiles.json"
    ProfileStore(path).save("work", [sample_state()])
    assert oct(path.stat().st_mode & 0o777) == "0o600"


def test_names_keep_insertion_order_and_move(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    for name in ("work", "home", "docked"):
        store.save(name, [sample_state()])
    assert store.names() == ["work", "home", "docked"]

    assert store.move("docked", -1)
    assert store.names() == ["work", "docked", "home"]
    assert store.move("docked", -1)
    assert store.names() == ["docked", "work", "home"]
    assert not store.move("docked", -1)


def test_rename_profile(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    store.save("work", [sample_state()])
    profile = store.get("work")
    assert profile is not None
    created_at = profile.created_at

    assert store.rename("work", "office")
    assert store.names() == ["office"]
    renamed = store.get("office")
    assert renamed is not None
    assert renamed.created_at == created_at
    assert store.get("work") is None


def test_rename_rejects_duplicate_and_invalid_names(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    store.save("work", [sample_state()])
    store.save("home", [sample_state()])
    with pytest.raises(ProfileError):
        store.rename("work", "home")
    with pytest.raises(ProfileError):
        store.rename("work", "  ")


def test_find_active_by_connector_name(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    state = sample_state()
    store.save("work", [state])
    assert store.find_active([state]) == "work"

    changed = sample_state(x=1)
    assert store.find_active([changed]) is None


def test_find_active_by_hardware_id_fallback(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    saved = sample_state(name="DP-1", hardware_id="Microstep/MSI MP275/ABC")
    store.save("work", [saved])

    replugged = sample_state(name="DP-4", hardware_id="Microstep/MSI MP275/ABC")
    assert store.find_active([replugged]) == "work"


def test_find_active_requires_the_same_output_count(tmp_path: Path) -> None:
    store = ProfileStore(tmp_path / "profiles.json")
    state = sample_state()
    store.save("work", [state])
    second = sample_state(name="eDP-1")
    assert store.find_active([state, second]) is None
