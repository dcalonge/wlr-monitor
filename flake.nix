{
  description = "A TUI monitor-layout editor for wlroots compositors using wlr-randr";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          python = pkgs.python3;
          wlr-monitor = python.pkgs.buildPythonApplication {
            pname = "wlr-monitor";
            version = "0.1.0";
            format = "pyproject";
            src = ./.;

            nativeBuildInputs = [
              python.pkgs.setuptools
              python.pkgs.wheel
              pkgs.makeWrapper
            ];
            propagatedBuildInputs = [ python.pkgs.textual ];
            nativeCheckInputs = [ python.pkgs.pytest ];
            doCheck = true;
            checkPhase = ''
              runHook preCheck
              python -m pytest -q
              runHook postCheck
            '';

            postFixup = ''
              wrapProgram $out/bin/wlr-monitor \
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.wlr-randr ]}
            '';

            meta = {
              description = "A TUI monitor-layout editor for wlroots compositors";
              homepage = "https://github.com/dcalonge/wlr-monitor";
              license = pkgs.lib.licenses.mit;
              mainProgram = "wlr-monitor";
              platforms = pkgs.lib.platforms.linux;
            };
          };
        in
        {
          default = wlr-monitor;
          wlr-monitor = wlr-monitor;
        }
      );

      devShells = forAllSystems (
        pkgs:
        let
          python = pkgs.python3.withPackages (ps: [
            ps.pytest
            ps.ruff
            ps.textual
          ]);
        in
        {
          default = pkgs.mkShell {
            packages = [
              python
              pkgs.wlr-randr
            ];
            shellHook = ''
              export PYTHONPATH="$PWD/src''${PYTHONPATH:+:$PYTHONPATH}"
            '';
          };
        }
      );

      # Home Manager module: installs the package, creates the writable
      # include files and hooks them into the mango / sway configs.
      homeModules.default = import ./nix/hm-module.nix self;
      homeManagerModules.default = self.homeModules.default;

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
