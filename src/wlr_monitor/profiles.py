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

from .model import OutputState, match_state

SCHEMA_VERSION = 2


class ProfileError(RuntimeError):
    """Raised when a profile file cannot be read or written."""


def _now() -> str:
    return datetime.now(UTC).isoformat(timespec="seconds")


RESERVED_NAMES = ("__save__", "__launch__")


def _validate_name(name: str) -> str:
    clean_name = name.strip()
    if not clean_name:
        raise ProfileError("Profile name must not be empty.")
    if clean_name in RESERVED_NAMES:
        raise ProfileError("That profile name is reserved.")
    # Names are written into compositor config comments, so no newlines.
    if any(
        character in "/\\" or not character.isprintable() for character in clean_name
    ):
        raise ProfileError(
            "Profile names cannot contain slashes or control characters."
        )
    return clean_name


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
            try:
                outputs[str(output_name)] = OutputState.from_dict(
                    str(output_name), raw_state
                )
            except (KeyError, TypeError, ValueError) as exc:
                raise ProfileError(
                    f"Profile {name!r} has an invalid entry for {output_name}: {exc}"
                ) from exc
        return cls(
            name=name,
            outputs=outputs,
            created_at=str(data.get("created_at") or ""),
            updated_at=str(data.get("updated_at") or ""),
        )


@dataclass
class _Snapshot:
    """The parsed profile file: valid profiles, order and broken entries."""

    profiles: dict[str, Profile] = field(default_factory=dict)
    order: list[str] = field(default_factory=list)
    # name -> (raw JSON value, error message); written back untouched.
    broken: dict[str, tuple[object, str]] = field(default_factory=dict)


class ProfileStore:
    """A small JSON file containing named profiles.

    The file also keeps a display order so the profile browser can be
    re-ordered, like hyprmon's profile menu.

    A profile with invalid content (for example an unknown rotation value)
    does not make the whole file unusable: it is left out of ``load()`` and
    ``names()``, reported by ``problems()``, raises from ``get()``, and is
    written back unchanged so editing other profiles never loses it.
    """

    def __init__(self, path: Path | str | None = None) -> None:
        self.path = Path(path) if path is not None else self.default_path()

    @staticmethod
    def default_path() -> Path:
        config_home = os.environ.get("XDG_CONFIG_HOME")
        base = Path(config_home) if config_home else Path.home() / ".config"
        return base / "wlr-monitor" / "profiles.json"

    def load(self) -> dict[str, Profile]:
        return self._read().profiles

    def problems(self) -> list[str]:
        """One message per profile that could not be read."""
        return [message for _raw, message in self._read().broken.values()]

    def exists(self, name: str) -> bool:
        """True for any stored profile, including one that cannot be read."""
        snapshot = self._read()
        return name in snapshot.profiles or name in snapshot.broken

    def names(self) -> list[str]:
        snapshot = self._read()
        result = [name for name in snapshot.order if name in snapshot.profiles]
        result.extend(sorted(name for name in snapshot.profiles if name not in result))
        return result

    def get(self, name: str) -> Profile | None:
        snapshot = self._read()
        if name in snapshot.broken:
            raise ProfileError(snapshot.broken[name][1])
        return snapshot.profiles.get(name)

    def save(self, name: str, states: Iterable[OutputState]) -> Profile:
        clean_name = _validate_name(name)

        snapshot = self._read()
        existing = snapshot.profiles.get(clean_name)
        now = _now()
        profile = Profile(
            name=clean_name,
            outputs={state.name: state for state in states},
            created_at=existing.created_at if existing else now,
            updated_at=now,
        )
        # Saving over a broken profile replaces it.
        snapshot.broken.pop(clean_name, None)
        snapshot.profiles[clean_name] = profile
        if clean_name not in snapshot.order:
            snapshot.order.append(clean_name)
        self._write(snapshot)
        return profile

    def delete(self, name: str) -> bool:
        snapshot = self._read()
        if name not in snapshot.profiles and name not in snapshot.broken:
            return False
        snapshot.profiles.pop(name, None)
        snapshot.broken.pop(name, None)
        snapshot.order = [item for item in snapshot.order if item != name]
        self._write(snapshot)
        return True

    def rename(self, old_name: str, new_name: str) -> bool:
        clean_name = _validate_name(new_name)

        snapshot = self._read()
        if old_name in snapshot.broken:
            raise ProfileError(snapshot.broken[old_name][1])
        if old_name not in snapshot.profiles:
            return False
        if clean_name == old_name:
            return True
        if clean_name in snapshot.profiles or clean_name in snapshot.broken:
            raise ProfileError(f"A profile named {clean_name!r} already exists.")

        profile = snapshot.profiles.pop(old_name)
        profile.name = clean_name
        profile.updated_at = _now()
        snapshot.profiles[clean_name] = profile
        snapshot.order = [
            clean_name if item == old_name else item for item in snapshot.order
        ]
        self._write(snapshot)
        return True

    def move(self, name: str, offset: int) -> bool:
        snapshot = self._read()
        if name not in snapshot.profiles:
            return False
        order = [item for item in snapshot.order if item in snapshot.profiles]
        order.extend(sorted(item for item in snapshot.profiles if item not in order))
        index = order.index(name)
        target = max(0, min(len(order) - 1, index + offset))
        if target == index:
            return False
        order.insert(target, order.pop(index))
        # Broken profiles keep their place at the end of the order.
        snapshot.order = order + [
            item for item in snapshot.order if item in snapshot.broken
        ]
        self._write(snapshot)
        return True

    def find_active(self, states: Iterable[OutputState]) -> str | None:
        """Return the name of the profile that matches these output states.

        Profiles are matched by connector name first and by EDID hardware id
        as a fallback, which keeps them usable when the kernel reassigns
        connector names after a replug.
        """
        current = list(states)
        connected = [state.hardware_id for state in current]
        for name, profile in self.load().items():
            if len(profile.outputs) != len(current):
                continue
            matched = True
            for current_state in current:
                saved = match_state(
                    profile.outputs,
                    current_state.name,
                    current_state.hardware_id,
                    connected,
                )
                if saved is None or not saved.matches_config(current_state):
                    matched = False
                    break
            if matched:
                return name
        return None

    def _read(self) -> _Snapshot:
        if not self.path.exists():
            return _Snapshot()
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

        snapshot = _Snapshot()
        for name, raw_profile in raw_profiles.items():
            key = str(name)
            try:
                if not isinstance(raw_profile, dict):
                    raise ProfileError(f"Profile {key!r} is not an object.")
                snapshot.profiles[key] = Profile.from_dict(key, raw_profile)
            except ProfileError as exc:
                snapshot.broken[key] = (raw_profile, str(exc))

        raw_order = data.get("order", [])
        if isinstance(raw_order, list):
            snapshot.order = [str(item) for item in raw_order]
        return snapshot

    def _write(self, snapshot: _Snapshot) -> None:
        profiles: dict[str, object] = {
            name: raw for name, (raw, _message) in snapshot.broken.items()
        }
        profiles.update(
            {name: profile.to_dict() for name, profile in snapshot.profiles.items()}
        )
        payload = {
            "version": SCHEMA_VERSION,
            "order": [name for name in snapshot.order if name in profiles],
            "profiles": profiles,
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
