"""Named monitor-layout profiles stored as JSON."""

from __future__ import annotations

import json
import os
import tempfile
import time
from collections.abc import Iterable
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path

from .model import OutputState

SCHEMA_VERSION = 2


class ProfileError(RuntimeError):
    """Raised when a profile file cannot be read or written."""


def _now() -> str:
    return datetime.now(UTC).isoformat(timespec="seconds")


@dataclass
class Profile:
    name: str
    outputs: dict[str, OutputState] = field(default_factory=dict)
    created_at: str = ""
    updated_at: str = ""

    def to_dict(self) -> dict[str, object]:
        data: dict[str, object] = {
            "outputs": {name: state.to_dict() for name, state in self.outputs.items()}
        }
        if self.created_at:
            data["created_at"] = self.created_at
        if self.updated_at:
            data["updated_at"] = self.updated_at
        return data

    @classmethod
    def from_dict(cls, name: str, data: dict[str, object]) -> Profile:
        raw_outputs = data.get("outputs")
        if not isinstance(raw_outputs, dict):
            raise ProfileError(f"Profile {name!r} has no outputs object.")
        outputs: dict[str, OutputState] = {}
        for output_name, raw_state in raw_outputs.items():
            if not isinstance(raw_state, dict):
                raise ProfileError(f"Profile {name!r} has an invalid output entry.")
            outputs[str(output_name)] = OutputState.from_dict(
                str(output_name), raw_state
            )
        return cls(
            name=name,
            outputs=outputs,
            created_at=str(data.get("created_at") or ""),
            updated_at=str(data.get("updated_at") or ""),
        )


class ProfileStore:
    """A small JSON file containing named profiles.

    The file also keeps a display order so the profile browser can be
    re-ordered, mirroring hyprmon's profile menu.
    """

    def __init__(self, path: Path | str | None = None) -> None:
        self.path = Path(path) if path is not None else self.default_path()

    @staticmethod
    def default_path() -> Path:
        config_home = os.environ.get("XDG_CONFIG_HOME")
        base = Path(config_home) if config_home else Path.home() / ".config"
        return base / "wlr-randr-tui" / "profiles.json"

    def load(self) -> dict[str, Profile]:
        return self._read()[0]

    def names(self) -> list[str]:
        profiles, order = self._read()
        result = [name for name in order if name in profiles]
        result.extend(sorted(name for name in profiles if name not in result))
        return result

    def get(self, name: str) -> Profile | None:
        return self.load().get(name)

    def save(self, name: str, states: Iterable[OutputState]) -> Profile:
        clean_name = name.strip()
        if not clean_name:
            raise ProfileError("Profile name must not be empty.")
        if clean_name == "__save__":
            raise ProfileError("That profile name is reserved.")
        if any(character in clean_name for character in "/\\\0"):
            raise ProfileError(
                "Profile names cannot contain slashes or control characters."
            )

        profiles, order = self._read()
        existing = profiles.get(clean_name)
        now = _now()
        profile = Profile(
            name=clean_name,
            outputs={state.name: state for state in states},
            created_at=existing.created_at if existing else now,
            updated_at=now,
        )
        profiles[clean_name] = profile
        if clean_name not in order:
            order.append(clean_name)
        self._write(profiles, order)
        return profile

    def delete(self, name: str) -> bool:
        profiles, order = self._read()
        if name not in profiles:
            return False
        del profiles[name]
        self._write(profiles, [item for item in order if item != name])
        return True

    def rename(self, old_name: str, new_name: str) -> bool:
        clean_name = new_name.strip()
        if not clean_name:
            raise ProfileError("Profile name must not be empty.")
        if clean_name == "__save__":
            raise ProfileError("That profile name is reserved.")
        if any(character in clean_name for character in "/\\\0"):
            raise ProfileError(
                "Profile names cannot contain slashes or control characters."
            )

        profiles, order = self._read()
        if old_name not in profiles:
            return False
        if clean_name != old_name and clean_name in profiles:
            raise ProfileError(f"A profile named {clean_name!r} already exists.")
        if clean_name == old_name:
            return True

        profile = profiles.pop(old_name)
        profile.name = clean_name
        profile.updated_at = _now()
        profiles[clean_name] = profile
        order = [clean_name if item == old_name else item for item in order]
        self._write(profiles, order)
        return True

    def move(self, name: str, offset: int) -> bool:
        profiles, order = self._read()
        if name not in profiles:
            return False
        order = [item for item in order if item in profiles]
        order.extend(sorted(item for item in profiles if item not in order))
        try:
            index = order.index(name)
        except ValueError:
            return False
        target = max(0, min(len(order) - 1, index + offset))
        if target == index:
            return False
        order.insert(target, order.pop(index))
        self._write(profiles, order)
        return True

    def find_active(self, states: Iterable[OutputState]) -> str | None:
        """Return the name of the profile that matches these output states.

        Profiles are matched by connector name first and by EDID hardware id
        as a fallback, which keeps them usable when the kernel reassigns
        connector names after a replug.
        """
        current = list(states)
        for name, profile in self.load().items():
            if len(profile.outputs) != len(current):
                continue
            matched = True
            for current_state in current:
                saved = profile.outputs.get(current_state.name)
                if saved is None and current_state.hardware_id:
                    saved = next(
                        (
                            candidate
                            for candidate in profile.outputs.values()
                            if candidate.hardware_id
                            and candidate.hardware_id == current_state.hardware_id
                        ),
                        None,
                    )
                if saved is None or not saved.matches_config(current_state):
                    matched = False
                    break
            if matched:
                return name
        return None

    def _read(self) -> tuple[dict[str, Profile], list[str]]:
        if not self.path.exists():
            return {}, []
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except OSError as exc:
            raise ProfileError(f"Could not read {self.path}: {exc}") from exc
        except json.JSONDecodeError as exc:
            raise ProfileError(f"Could not parse {self.path}: {exc}") from exc

        if not isinstance(data, dict):
            raise ProfileError(f"{self.path} does not contain a JSON object.")
        raw_profiles = data.get("profiles", {})
        if not isinstance(raw_profiles, dict):
            raise ProfileError(f"{self.path} has an invalid profiles object.")

        profiles: dict[str, Profile] = {}
        for name, raw_profile in raw_profiles.items():
            if not isinstance(raw_profile, dict):
                raise ProfileError(f"Profile {name!r} is not an object.")
            profiles[str(name)] = Profile.from_dict(str(name), raw_profile)

        raw_order = data.get("order", [])
        order = [str(item) for item in raw_order] if isinstance(raw_order, list) else []
        return profiles, order

    def _write(self, profiles: dict[str, Profile], order: list[str]) -> None:
        payload = {
            "version": SCHEMA_VERSION,
            "order": [name for name in order if name in profiles],
            "profiles": {name: profile.to_dict() for name, profile in profiles.items()},
        }
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            try:
                os.chmod(self.path.parent, 0o700)
            except OSError:
                pass
            self._backup_existing()
            fd, temp_name = tempfile.mkstemp(
                prefix=f".{self.path.name}.",
                suffix=".tmp",
                dir=self.path.parent,
                text=True,
            )
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as handle:
                    json.dump(payload, handle, indent=2, sort_keys=True)
                    handle.write("\n")
                os.chmod(temp_name, 0o600)
                os.replace(temp_name, self.path)
            except BaseException:
                try:
                    os.unlink(temp_name)
                except OSError:
                    pass
                raise
        except OSError as exc:
            raise ProfileError(f"Could not write {self.path}: {exc}") from exc

    def _backup_existing(self) -> None:
        """Keep a timestamped copy before modifying an existing profile file."""
        if not self.path.exists():
            return
        stamp = int(time.time())
        backup = self.path.with_name(f"{self.path.name}.bak.{stamp}")
        counter = 1
        while backup.exists():
            backup = self.path.with_name(f"{self.path.name}.bak.{stamp}.{counter}")
            counter += 1
        data = self.path.read_bytes()
        backup.write_bytes(data)
        try:
            os.chmod(backup, 0o600)
        except OSError:
            pass
