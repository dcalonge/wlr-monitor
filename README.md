# wlr-randr-tui

A terminal monitor-layout editor for **wlroots compositors** (mango, sway,
niri, River, Labwc, Wayfire, …) built on
[`wlr-randr`](https://gitlab.freedesktop.org/emersion/wlr-randr).

It is a TUI in the spirit of [hyprmon](https://github.com/erans/hyprmon). The
layout itself goes through the standard `wlr-output-management-unstable-v1`
protocol, so it works anywhere `wlr-randr` works. On **mango**, **sway** and
**niri** it also uses the compositor's own IPC (`mmsg`, `swaymsg`,
`niri msg`) for the settings the protocol cannot express (HDR, color depth,
color profiles, mango's VRR) and writes the applied layout to a config file
you include from your compositor config, so it survives restarts.

The interface follows [impala](https://github.com/pythops/impala) and
[bluetui](https://github.com/pythops/bluetui): stacked sections that keep
working in narrow or floating windows, and only the terminal's ANSI colors,
so it matches your foot / kitty / alacritty theme without configuration.

```text
 ┏━ Layout ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
 ┃    ┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓┌────────────────────────┐    ┃
 ┃    ┃        HDMI-A-1 [ON]         ┃│       eDP-1 [ON]       │    ┃
 ┃    ┃       1920x1080@100Hz        ┃│     1920x1080@60Hz     │    ┃
 ┃    ┃                              ┃│        VRR x1.2        │    ┃
 ┃    ┃                              ┃└────────────────────────┘    ┃
 ┃    ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛                              ┃
 ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ grid 32px · snap Edges · Ready. ━┛
 ┌─ Outputs ──────────────────────────────────────────────────────────┐
 │          Name           Mode      State Scale   Position  VRR      │
 │                                                                    │
 │ ▸      HDMI-A-1    1920x1080@100    On    1       0,0     Off      │
 │         eDP-1       1920x1080@60    On   1.2     1920,0    On      │
 └────────────────────────────────────────────────────────────────────┘
       ←↓↑→ Move | ⇥ Next | f Mode | r Scale | t Rotation | v VRR
  c Advanced | Space On/off | a Apply | o Profiles | ? Help | q Quit
```

## Requirements

- Linux with a wlroots-based Wayland compositor.
- `wlr-randr` on `PATH`.
- Python 3.11+ and Textual 8.2.5+ (installed automatically by pip/pipx/uv).
- `mmsg` / `swaymsg` / `niri` for the compositor integrations; they come with
  their compositors.
- A terminal at least ~30×12. Mouse support is optional but enables dragging,
  right-click toggling and wheel scaling.

## Install

### pipx / uv

```sh
pipx install .
# or
uv tool install .
```

### Nix

The repository has a flake with the package (wrapped with `wlr-randr`) and a
development shell:

```sh
nix run .
nix develop
```

### NixOS / Home Manager

The flake ships a Home Manager module. It installs the package, creates the
writable include files and adds the include line to the compositor config:

```nix
{
  inputs.wlr-randr-tui.url = "github:dcalonge/wlr-randr-tui";

  # In your Home Manager configuration:
  imports = [ inputs.wlr-randr-tui.homeModules.default ];

  programs.wlr-randr-tui = {
    enable = true;
    mango.enable = true; # needs mango's HM module (wayland.windowManager.mango)
    # sway.enable = true; # wayland.windowManager.sway
    # niri.enable = true; # creates the file; add the include line yourself
  };
}
```

Without the module, add the package to `home.packages` and the include line
from [Compositor integration](#compositor-integration) to your config, e.g.
via `wayland.windowManager.mango.extraConfig`.

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
wlr-randr-tui --init-include        # write the compositor include, print the line to add
wlr-randr-tui --print-config work   # print the compositor config for a profile
wlr-randr-tui --cfg ~/.config/wlr-randr-tui
```

`--cfg` accepts either a directory (then `profiles.json` is read inside it) or
a JSON file directly. `--compositor auto|wlroots|mango|sway|niri` overrides
detection, `--include PATH` changes the generated file and `--no-include`
disables it (and ignores the state remembered from earlier applies).
`-v` / `--version` prints the version.

Changes are **not applied** until you press `a`. This makes it safe to
experiment with a layout before touching the live session. `Ctrl+R` reloads
the outputs and discards pending edits.

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
| `t` | Pick rotation |
| `v` | Toggle adaptive sync (VRR) |
| `c` / `d` | Advanced settings: rotation, VRR, on/off, HDR, color depth, color profile (`i` picks an ICC file; absolute path, `~` is expanded) |
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

The last enabled monitor cannot be disabled, and nothing (a profile, a revert,
the advanced dialog) is applied if it would turn every monitor off.

Mouse support: left-click a monitor to select it and drag it to move it;
right-click to toggle it on/off; scroll over a monitor to change its scale by
`0.05`. Dragging requires a terminal with mouse support (foot, Kitty, WezTerm,
Alacritty, etc.).

## Compositor integration

The compositor is detected from `MANGO_INSTANCE_SIGNATURE`, `SWAYSOCK`,
`NIRI_SOCKET` or `XDG_CURRENT_DESKTOP`. Every successful apply (`a`, `Z`, a
profile from the browser or `--profile`) also writes the full layout to:

| Compositor | Generated file | Line to add to your config |
| --- | --- | --- |
| mango | `~/.config/mango/wlr-randr-tui.conf` | `source_optional=~/.config/mango/wlr-randr-tui.conf` |
| sway | `~/.config/sway/wlr-randr-tui.conf` | `include ~/.config/sway/wlr-randr-tui.conf` |
| niri | `~/.config/niri/wlr-randr-tui.kdl` | `include "wlr-randr-tui.kdl"` |

`wlr-randr-tui --init-include` writes the file from the current layout and
prints the exact line, and the TUI shows the line at start-up until it is
present. On niri, which refuses a config whose include does not exist, the
TUI also creates the file at start-up if it is missing. mango always uses
`~/.config/mango`, whatever `XDG_CONFIG_HOME` says.

Remove your own monitor/output rules for the same monitors: mango and niri
use a single matching rule (the *first* one at start-up), so leftovers would
win. Settings wlr-randr-tui does not manage can be moved into the generated
file instead and are kept on every rewrite: extra `monitor_rule` options in
mango (`hdr_max_lum`, `hdr_force`, `primary`, …) and extra nodes in a niri
`output` block (`layout {}`, `hot-corners`, `focus-at-startup`,
`variable-refresh-rate on-demand=true`, …). sway merges `output` commands, so
keep `bg`, `max_render_time`, … in your own config.

Outputs are matched by EDID make/model/serial when known and unique (stable
across dock / MST connector renames), otherwise by connector name; identical
monitors that report the same serial are always matched by connector. Monitors
that are not connected keep their last rule, so applying something while
undocked does not forget the dock layout; delete the generated file to start
over (the remembered state goes with it). `--print-config` shows exactly what
an apply would write, remembered monitors included.

The generated file is a plain, writable file, so it keeps working when the
main config is a read-only Nix store symlink. wlr-randr-tui refuses to write
through a symlink. Settings the compositor cannot report back (color profile,
bit depth) are kept in `~/.local/state/wlr-randr-tui/<compositor>-layout.json`
(a custom `--include` path gets its own state file). If `swaymsg` or `niri` is
missing, wlr-randr-tui applies with `wlr-randr` instead and skips HDR and
color settings.

| | mango | sway | niri | other wlroots |
| --- | --- | --- | --- | --- |
| Live apply | `monitor_rule` + `mmsg dispatch reload_config` | `swaymsg output …` | `niri msg output …` | `wlr-randr` |
| VRR | `vrr:` (see below) | `adaptive_sync` | `variable-refresh-rate` | `--adaptive-sync` |
| HDR | `hdr:` / `togglehdr` | `hdr` (sway 1.11+) | – | – |
| Color depth | – (global `hdr_depth`) | `render_bit_depth` 6/8/10 | `max-bpc` (niri after 25.11) | – |
| Color profile | `icc:` | `color_profile` gamma22/srgb (1.11+) or ICC | – | – |

Version-dependent options are only offered when the compositor's IPC shows
support for them (sway's `get_outputs` reports `hdr`, niri's `outputs`
reports `max_bpc`).

**mango and VRR.** mango keeps adaptive sync in sync with the monitor rule's
`vrr` value and switches it off again on the next focus change, which is why
VRR set through `wlr-randr` alone does not stick. Once the include is sourced,
wlr-randr-tui applies everything by writing the monitor rules and running
`mmsg dispatch reload_config`, so VRR (and HDR/ICC) work and persist. A config
reload is a full reload: it re-runs your `exec=` lines, resets per-tag
master factor / count to the configured defaults and re-creates keyboard and
cursor settings. Without the include it falls back to `wlr-randr` plus
`mmsg dispatch togglehdr` and warns about VRR.

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
          "hdr": true,
          "hardware_id": "Microstep/MSI MP275/PC3M394200857",
          "description": "Microstep MSI MP275 PC3M394200857 (HDMI-A-1)"
        }
      }
    }
  }
}
```

`hdr`, `bit_depth` and `color_profile` are only stored when they are set, so
older profiles keep working and never reset those settings. Settings the
current compositor cannot apply are ignored when a profile is applied.

Profiles match outputs by connector name first and by EDID hardware id as a
fallback, so they keep working when the kernel renames a connector after a
replug. The fallback is only used when the id is unique on both sides, so
identical monitors never get each other's settings. Outputs in the profile that are not connected are skipped; connected
outputs missing from the profile are left alone. A profile with invalid
content (for example an unknown rotation) is skipped and reported in the
profile browser and by `--list-profiles`; it is kept in the file until you
fix, overwrite or delete it.

Applying a profile writes it to the compositor include (see above), so once
the include line is in your config it is also what the compositor uses after
a restart — no start-up command needed. On other wlroots compositors, run
`wlr-randr-tui --profile work` from your startup file or bind it to a key:

```text
bindsym $mod+Shift+p exec wlr-randr-tui --profile work
```

Before modifying an existing profile file, the store writes a timestamped
backup next to it (`profiles.json.bak.<epoch>`). The file and its directory are
created with user-only permissions.

## Theme

wlr-randr-tui uses a Textual theme built only from the terminal's 16 ANSI
colors and its default foreground/background, like impala and bluetui. Change
your terminal theme (for example foot's `[colors]` section) and the TUI follows
it. `NO_COLOR` is respected.

## Limitations

- HDR, color depth and color profiles are only available on mango, sway and
  niri (and only what each one supports, see the table above).
- Applying a profile only touches connected outputs. Monitors absent from a
  profile are left as they are.
- `wlr-randr` and `niri msg` have no transactional apply: each output (or
  setting) is a separate command, so the layout can briefly overlap while
  applying. Outputs are enabled before others are disabled.
- Include detection follows one level of `source` / `include` lines. Deeper
  nesting still works, but the start-up hint may show anyway.

## Development

```sh
nix develop          # Python + Textual + pytest + ruff
pytest
ruff check src tests
ruff format --check src tests
```

The app is split into a pure data/geometry layer (`model.py`, `layout.py`),
the `wlr-randr` wrapper (`wlr.py`), compositor integrations (`compositor.py`),
profiles (`profiles.py`), and the Textual UI (`theme.py`, `widgets.py`,
`screens.py`, `app.py`).

## License

MIT
