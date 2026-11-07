"""A Textual theme that follows the terminal's own color palette.

impala and bluetui only use the 16 ANSI colors (plus the terminal's default
foreground/background), which is why they match foot, kitty, alacritty, …
themes without any configuration. This theme does the same: every color is
an ANSI color, so Textual emits palette indices instead of RGB values.
"""

from __future__ import annotations

from textual.theme import Theme

TERMINAL_THEME = Theme(
    name="terminal",
    primary="ansi_blue",
    secondary="ansi_cyan",
    accent="ansi_yellow",
    warning="ansi_yellow",
    error="ansi_red",
    success="ansi_green",
    foreground="ansi_default",
    background="ansi_default",
    surface="ansi_default",
    panel="ansi_default",
    boost="ansi_default",
    dark=True,
    ansi=True,
    variables={
        "ansi-background": "ansi_default",
        "ansi-foreground": "ansi_default",
        "border": "ansi_green",
        "border-blurred": "ansi_default",
        "block-cursor-foreground": "ansi_white",
        "block-cursor-background": "ansi_bright_black",
        "block-cursor-text-style": "bold",
        "block-cursor-blurred-foreground": "ansi_default",
        "block-cursor-blurred-background": "ansi_bright_black",
        "block-cursor-blurred-text-style": "none",
        "block-hover-background": "ansi_default",
        "input-cursor-background": "ansi_white",
        "input-cursor-foreground": "ansi_black",
        "input-cursor-text-style": "none",
        "input-selection-background": "ansi_bright_black",
        "input-selection-foreground": "ansi_white",
        "screen-selection-background": "ansi_bright_black",
        "screen-selection-foreground": "ansi_white",
        "scrollbar": "ansi_bright_black",
        "scrollbar-hover": "ansi_white",
        "scrollbar-active": "ansi_white",
        "scrollbar-background": "ansi_default",
        "scrollbar-background-hover": "ansi_default",
        "scrollbar-background-active": "ansi_default",
        "scrollbar-corner-color": "ansi_default",
    },
)
