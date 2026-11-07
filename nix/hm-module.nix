self:
{
  config,
  lib,
  options,
  pkgs,
  ...
}:
let
  cfg = config.programs.wlr-randr-tui;
  # wlr-randr-tui reads $XDG_CONFIG_HOME, which Home Manager only exports
  # when xdg.enable is set; otherwise it falls back to ~/.config.
  configHome =
    if config.xdg.enable then config.xdg.configHome else "${config.home.homeDirectory}/.config";

  # mango hard-codes ~/.config/mango (it ignores XDG_CONFIG_HOME).
  mangoInclude = "${config.home.homeDirectory}/.config/mango/wlr-randr-tui.conf";
  swayInclude = "${configHome}/sway/wlr-randr-tui.conf";
  niriInclude = "${configHome}/niri/wlr-randr-tui.kdl";

  includeFiles =
    lib.optional cfg.mango.enable mangoInclude
    ++ lib.optional cfg.sway.enable swayInclude
    ++ lib.optional cfg.niri.enable niriInclude;

  # mango's Home Manager module comes from the mango flake; only touch it
  # when it has been imported, so this module also works without it.
  hasMangoModule = options ? wayland.windowManager.mango.extraConfig;
in
{
  options.programs.wlr-randr-tui = {
    enable = lib.mkEnableOption "wlr-randr-tui, a TUI monitor-layout editor";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "wlr-randr-tui.packages.\${system}.default";
      description = "The wlr-randr-tui package to install.";
    };

    mango.enable = lib.mkEnableOption ''
      the mango integration: creates ~/.config/mango/wlr-randr-tui.conf and
      appends `source_optional=` for it to
      `wayland.windowManager.mango.extraConfig` (requires mango's Home Manager
      module)
    '';

    sway.enable = lib.mkEnableOption ''
      the sway integration: creates ~/.config/sway/wlr-randr-tui.conf and
      appends `include` for it to `wayland.windowManager.sway.extraConfig`
    '';

    niri.enable = lib.mkEnableOption ''
      the niri integration: creates ~/.config/niri/wlr-randr-tui.kdl so an
      `include "wlr-randr-tui.kdl"` line in the niri config never fails
    '';
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        home.packages = [ cfg.package ];

        assertions = [
          {
            assertion = !cfg.mango.enable || hasMangoModule;
            message = "programs.wlr-randr-tui.mango.enable needs mango's Home Manager module (wayland.windowManager.mango).";
          }
        ];

        # The generated files must stay writable, so they are created here
        # instead of being linked from the Nix store. Existing files (with a
        # layout applied from the TUI) are left untouched.
        home.activation.wlrRandrTuiIncludes = lib.hm.dag.entryAfter [ "writeBoundary" ] (
          lib.concatMapStrings (file: ''
            if [ ! -e ${lib.escapeShellArg file} ]; then
              run mkdir -p ${lib.escapeShellArg (builtins.dirOf file)}
              run touch ${lib.escapeShellArg file}
            fi
          '') includeFiles
        );
      }

      (lib.mkIf cfg.sway.enable {
        wayland.windowManager.sway.extraConfig = lib.mkAfter ''
          include ${swayInclude}
        '';
      })

      (lib.optionalAttrs hasMangoModule {
        wayland.windowManager.mango.extraConfig = lib.mkIf cfg.mango.enable (
          lib.mkAfter ''

            source_optional=${mangoInclude}
          ''
        );
      })
    ]
  );
}
