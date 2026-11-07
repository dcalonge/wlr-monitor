# wlr-randr-tui

A terminal monitor-layout editor for **wlroots compositors** (Sway, Mango,
Hyprland's wlroots roots, River, Labwc, Wayfire, …) built on
[`wlr-randr`](https://gitlab.freedesktop.org/emersion/wlr-randr).

It is a TUI in the spirit of [hyprmon](https://github.com/erans/hyprmon), but
instead of talking to Hyprland it uses the standard
`wlr-output-management-unstable-v1` protocol through `wlr-randr`, so it works
anywhere `wlr-randr` works.

```text
┌ wlr-randr-tui ────────────────────────────────────────────────────────────┐
│ Outputs                       │ Layout                                    │
│ ▸ HDMI-A-1  1920x1080@100 1x  │        ╔══════════════════════╗           │
│   eDP-1     1920x1080@60 1.2x │        ║      HDMI-A-1  [ON]  ║           │
│                               │        ║   1920x1080 @ 100Hz  ║           │
│                               │        ╚══════════════════════╝           │
│                               │ Selected output                           │
│                               │ HDMI-A-1  Microstep MSI MP275             │
│                               │   State         on                        │
│                               │   Mode          1920x1080 @ 100 Hz        │
│                               │   Position      0,0  (logical 1920x1080)  │
│                               │   Scale         1                         │
│                               │   Transform     normal                    │
├───────────────────────────────┴───────────────────────────────────────────┤
│ grid 32px · snap Edges · profile work · Ready.      q Quit  ? Help        │
└───────────────────────────────────────────────────────────────────────────┘
```

## Features

- Proportional “desk map” of all outputs, including disabled ones.
- Keyboard and mouse movement, with grid sizes `1, 8, 16, 32, 64` and
  `Off / Edges / Centers / Both` snapping with visual alignment guides.
- Resolution and refresh-rate picker built from the modes `wlr-randr` reports.
- Scale picker with common DPI presets plus custom values; `[`/`]` and the
  mouse wheel step by `0.05`, clamped to `0.5–3.0`.
- Advanced dialog for transform/rotation, adaptive sync (VRR) and on/off.
- Visual badges for VRR, rotation and non-default scale, plus `[ON]`/`[OFF]`.
- Live apply through `wlr-randr`, with pending changes highlighted.
- Ordered named profiles with active-profile detection, rename, reorder,
  delete confirmation and timestamped backups.
- Scrollable help screen and an explicit informational dialog for features the
  wlr-output-management protocol cannot express.
- Small, dependency-light Python codebase using
  [Textual](https://textual.textualize.io/).

## Requirements

- Linux with a wlroots-based Wayland compositor.
- `wlr-randr` on `PATH`.
- Python 3.11+ and Textual (installed automatically by pip/pipx/uv).
- A terminal at least ~80×24. Mouse support is optional but enables dragging,
  right-click toggling and wheel scaling.

## Install

### pipx / uv

```sh
pipx install .
# or
uv tool install .
```

### Nix

The repository has a flake with the package and a development shell:

```sh
nix run .
nix develop
```

Or add it to a NixOS/Home Manager configuration:

```nix
inputs.wlr-randr-tui.url = "github:dcalonge/wlr-randr-tui";

# Home Manager
home.packages = [ inputs.wlr-randr-tui.packages.${pkgs.system}.default ];
```

## Usage

```sh
wlr-randr-tui                       # open the monitor editor
wlr-randr-tui profiles              # open the profile browser
wlr-randr-tui --profiles            # same as above
wlr-randr-tui --active-profile      # print the active profile name
wlr-randr-tui --list-profiles       # one name per line, * marks the active one
wlr-randr-tui --list-profiles --json
wlr-randr-tui --profile work        # apply a saved profile and exit
wlr-randr-tui --delete-profile old
wlr-randr-tui --outputs             # print current outputs
wlr-randr-tui --outputs --json
wlr-randr-tui --cfg ~/.config/wlr-randr-tui
```

`--cfg` accepts either a directory (then `profiles.json` is read inside it) or
a JSON file directly. `-v` / `--version` prints the version.

Changes are **not applied** until you press `a`. This makes it safe to
experiment with a layout before touching the live session. `Ctrl+R` reloads
from `wlr-randr` and discards pending edits.

## Keys

| Key | Action |
| --- | --- |
| `↑` `↓` `←` `→` / `hjkl` | Move the selected output by the grid size |
| `Shift` + arrows / `K` `J` `H` | Move by 10× the grid size |
| `Tab` / `Shift+Tab` | Select next / previous output |
| `g` / `G` | Cycle grid size (1, 8, 16, 32, 64 px) |
| `L` | Cycle snap mode (Off, Edges, Centers, Both) |
| `f` / `F` | Pick resolution and refresh rate |
| `r` / `R` | Open the scale picker |
| `[` / `]` | Scale down / up by 0.05 |
| `t` | Pick transform / rotation |
| `v` | Toggle adaptive sync (VRR) |
| `c` / `d` | Open the advanced display-settings dialog |
| `m` | Explain why mirroring is unavailable |
| `Space` / `Enter` | Enable / disable the selected output |
| `a` | Apply pending changes |
| `Ctrl+R` | Reload outputs, discarding pending changes |
| `z` | Reset pending edits to the last applied state |
| `Z` | Revert to the layout before the last apply |
| `S` | Save the current layout as the startup profile (`default`) |
| `p` | Save the current layout as a named profile |
| `o` | Open the profile browser |
| `Enter` (in browser) | Apply the highlighted profile |
| `r` (in browser) | Rename the highlighted profile |
| `d` (in browser) | Delete the highlighted profile (with confirmation) |
| `Shift+↑` / `Shift+↓` (in browser) | Reorder profiles |
| `?` | Show the scrollable help screen |
| `q` / `Ctrl+C` | Quit |

The last enabled monitor cannot be disabled.

Mouse support: left-click a monitor to select it and drag it to move it;
right-click to toggle it on/off; scroll over a monitor to change its scale by
`0.05`. Dragging requires a terminal with mouse support (foot, Kitty, WezTerm,
Alacritty, etc.).

## Profiles

Profiles live in:

```text
$XDG_CONFIG_HOME/wlr-randr-tui/profiles.json
```

which defaults to `~/.config/wlr-randr-tui/profiles.json`. The file keeps an
explicit display order and timestamps:

```json
{
  "version": 2,
  "order": ["work"],
  "profiles": {
    "work": {
      "created_at": "2026-01-01T09:00:00+00:00",
      "updated_at": "2026-01-01T09:00:00+00:00",
      "outputs": {
        "HDMI-A-1": {
          "enabled": true,
          "mode": { "height": 1080, "refresh": 100.0, "width": 1920 },
          "position": { "x": 0, "y": 0 },
          "scale": 1.0,
          "transform": "normal",
          "adaptive_sync": false,
          "hardware_id": "Microstep/MSI MP275/PC3M394200857",
          "description": "Microstep MSI MP275 PC3M394200857 (HDMI-A-1)"
        }
      }
    }
  }
}
```

Profiles match outputs by connector name first and by EDID hardware id as a
fallback, so they keep working when the kernel renames a connector after a
replug. Outputs in the profile that are not connected are skipped; connected
outputs missing from the profile are left alone.

Before modifying an existing profile file, the store writes a timestamped
backup next to it (`profiles.json.bak.<epoch>`). The file and its directory are
created with user-only permissions.

To restore a layout at login, run `wlr-randr-tui --profile work` from your
compositor's startup file or a systemd user service. You can also bind it to a
key, for example in Sway:

```text
bindsym $mod+Shift+p exec wlr-randr-tui --profile work
```

## Feature coverage

The goal is parity wherever the wlr-output-management protocol exposes the
necessary controls.

| Feature | Here |
| --- | --- |
| Visual monitor layout | Yes, including disabled outputs |
| Keyboard / mouse movement | Yes |
| Grid sizes 1/8/16/32/64 | Yes |
| Edge/center snapping with guides | Yes, Off/Edges/Centers/Both |
| Scale presets + custom scale | Yes, with 0.05 stepping |
| Resolution / refresh picker | Yes |
| VRR, transform, on/off | Yes, in the advanced dialog |
| Visual status indicators | Yes (`[ON]`, `[OFF]`, VRR, transform, scale) |
| Live apply | Yes, explicitly with `a` |
| Profiles: apply/save/rename/reorder/delete/active marker | Yes |
| CLI profile menu, list, active profile, JSON | Yes (`profiles`, `--profiles`, `--list-profiles`, `--active-profile`) |
| Scrollable help | Yes |
| Timestamped profile backups | Yes |
| Mirroring | **Not available** — not in `wlr-randr` |
| HDR, SDR brightness/saturation, colour depth/mode | **Not available** — compositor-specific |
| Rewriting the compositor config file | **Not done** — use profiles instead |
| Workspace migration on monitor removal | **Not available** — compositor-specific |

`S` saves a profile named `default` rather than rewriting compositor
configuration. Apply it with `wlr-randr-tui --profile default`, or use the
normal named-profile workflow.

## How it works

1. `wlr-randr --json` is parsed into a small data model.
2. The TUI edits that model locally and renders the layout map.
3. Pressing `a` runs one `wlr-randr --output …` command per changed output with
   `--on`/`--off`, `--mode`, `--pos`, `--scale`, `--transform` and
   `--adaptive-sync`.
4. The model is refreshed from `wlr-randr` after a successful apply.

No compositor-specific configuration files are modified.

## Limitations

- Mirroring, HDR, colour management and SDR controls are not exposed by
  `wlr-randr`; the UI says so instead of silently ignoring them.
- Applying a profile only touches connected outputs. Monitors absent from a
  profile are left as they are.
- A compositor may rearrange outputs after hotplug. Reload with `Ctrl+R` or
  re-apply a profile.
- `wlr-randr` has no transactional apply: each output is configured with a
  separate command, so the layout can briefly overlap while applying.

## Development

```sh
nix develop          # Python + Textual + pytest + ruff
pytest
ruff check src tests
ruff format --check src tests
```

The app is split into a pure data/geometry layer (`model.py`, `layout.py`), the
`wlr-randr` wrapper (`wlr.py`), profiles (`profiles.py`), and the Textual UI
(`widgets.py`, `screens.py`, `app.py`).

## License

MIT
