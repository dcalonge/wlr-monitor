"""Command line entry point."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from . import __version__
from .app import WlrRandrApp
from .model import Output, OutputState, format_refresh
from .profiles import Profile, ProfileError, ProfileStore
from .wlr import WlrRandr, WlrRandrError


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="wlr-randr-tui",
        description="A terminal monitor-layout editor for wlroots compositors.",
    )
    parser.add_argument(
        "-v",
        "--version",
        action="version",
        version=f"%(prog)s {__version__}",
    )
    parser.add_argument(
        "--wlr-randr",
        metavar="PATH",
        help="path to the wlr-randr executable (default: $WLR_RANDR or PATH)",
    )
    parser.add_argument(
        "--config",
        "--cfg",
        dest="config",
        metavar="PATH",
        type=Path,
        help=(
            "profile file or directory "
            "(default: $XDG_CONFIG_HOME/wlr-randr-tui/profiles.json)"
        ),
    )
    action = parser.add_mutually_exclusive_group()
    action.add_argument(
        "--profile",
        metavar="NAME",
        help="apply a saved profile and exit",
    )
    action.add_argument(
        "--profiles",
        action="store_true",
        help="open the profile browser",
    )
    action.add_argument(
        "--list-profiles",
        action="store_true",
        help="list saved profile names and exit",
    )
    action.add_argument(
        "--active-profile",
        action="store_true",
        help="print the name of the active profile and exit",
    )
    action.add_argument(
        "--delete-profile",
        metavar="NAME",
        help="delete a saved profile and exit",
    )
    action.add_argument(
        "--outputs",
        action="store_true",
        help="print the current outputs and exit",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="with --list-profiles or --outputs, print JSON",
    )
    parser.add_argument(
        "command",
        nargs="?",
        choices=("profiles",),
        help="'profiles' is an alias for --profiles",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    store = ProfileStore(_config_path(args.config))
    backend = WlrRandr(args.wlr_randr)

    try:
        if args.list_profiles:
            return _list_profiles(store, backend, as_json=args.json)
        if args.active_profile:
            return _print_active_profile(store, backend)
        if args.delete_profile:
            return _delete_profile(store, args.delete_profile)
        if args.outputs:
            return _print_outputs(backend, as_json=args.json)
        if args.profile:
            return _apply_profile(backend, store, args.profile)
    except (WlrRandrError, ProfileError) as exc:
        print(f"wlr-randr-tui: {exc}", file=sys.stderr)
        return 1

    app = WlrRandrApp(
        backend=backend,
        store=store,
        profiles_only=args.profiles or args.command == "profiles",
    )
    app.run()
    return 0


def _config_path(raw: Path | None) -> Path | None:
    """Accept either a profiles.json file or a directory, like hyprmon's --cfg."""
    if raw is None:
        return None
    path = raw.expanduser()
    if path.exists() and path.is_dir():
        return path / "profiles.json"
    if path.suffix == ".json" or (path.exists() and path.is_file()):
        return path
    return path / "profiles.json"


def _active_profile(store: ProfileStore, backend: WlrRandr) -> str | None:
    outputs = backend.read_outputs()
    return store.find_active([output.state() for output in outputs])


def _list_profiles(store: ProfileStore, backend: WlrRandr, *, as_json: bool) -> int:
    names = store.names()
    try:
        active = _active_profile(store, backend)
    except (WlrRandrError, ProfileError):
        # Listing must still work when wlr-randr is unavailable.
        active = None

    if as_json:
        payload = [{"name": name, "is_active": name == active} for name in names]
        print(json.dumps(payload, indent=2))
        return 0

    # One name per line, with an asterisk for the active profile.
    for name in names:
        print(f"{name} *" if name == active else name)
    return 0


def _print_active_profile(store: ProfileStore, backend: WlrRandr) -> int:
    active = _active_profile(store, backend)
    print(active if active else "No active profile found")
    return 0


def _delete_profile(store: ProfileStore, name: str) -> int:
    if store.delete(name):
        print(f"Deleted profile {name!r}.")
        return 0
    print(f"wlr-randr-tui: no profile named {name!r}", file=sys.stderr)
    return 1


def _print_outputs(backend: WlrRandr, *, as_json: bool) -> int:
    outputs = backend.read_outputs()
    if as_json:
        print(json.dumps([_output_dict(output) for output in outputs], indent=2))
        return 0
    if not outputs:
        print("No outputs found.")
        return 0
    header = (
        f"{'NAME':<14} {'ON':<4} {'MODE':<20} {'POSITION':<12} "
        f"{'SCALE':<7} {'TRANSFORM':<12} VRR"
    )
    print(header)
    for output in outputs:
        mode = (
            f"{output.mode.size_text}@{format_refresh(output.mode.refresh)}"
            if output.mode is not None
            else "-"
        )
        print(
            f"{output.name:<14} "
            f"{'yes' if output.enabled else 'no':<4} "
            f"{mode:<20} "
            f"{f'{output.x},{output.y}':<12} "
            f"{output.scale:<7g} "
            f"{output.transform:<12} "
            f"{'on' if output.adaptive_sync else 'off'}"
        )
    return 0


def _output_dict(output: Output) -> dict[str, object]:
    state = output.state()
    result = state.to_dict()
    result["name"] = state.name
    result["description"] = output.description
    return result


def _matching_state(profile: Profile, output: Output) -> OutputState | None:
    state = profile.outputs.get(output.name)
    if state is not None:
        return state
    if not output.hardware_id:
        return None
    return next(
        (
            candidate
            for candidate in profile.outputs.values()
            if candidate.hardware_id and candidate.hardware_id == output.hardware_id
        ),
        None,
    )


def _apply_profile(backend: WlrRandr, store: ProfileStore, name: str) -> int:
    profile = store.get(name)
    if profile is None:
        available = ", ".join(store.names()) or "none"
        print(
            f"wlr-randr-tui: no profile named {name!r} (available: {available})",
            file=sys.stderr,
        )
        return 1

    outputs = backend.read_outputs()
    states = []
    not_in_profile = []
    current_names = {output.name for output in outputs}
    for output in outputs:
        state = _matching_state(profile, output)
        if state is None:
            not_in_profile.append(output.name)
            continue
        state.apply_to(output)
        states.append(output.state())

    if not states:
        print(
            f"wlr-randr-tui: profile {name!r} does not match any connected output.",
            file=sys.stderr,
        )
        return 1

    results = backend.apply_states(states)
    failures = [result for result in results if not result.ok]
    for result in results:
        if result.ok:
            print(f"Applied {result.name}.")
        else:
            print(f"Failed {result.name}: {result.detail}", file=sys.stderr)
    if not_in_profile:
        print(
            f"Left unchanged (not in profile): {', '.join(not_in_profile)}",
            file=sys.stderr,
        )
    absent = sorted(set(profile.outputs) - current_names)
    if absent:
        print(f"Skipped (not connected): {', '.join(absent)}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
