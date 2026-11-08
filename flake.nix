{
  description = "A TUI monitor-layout editor for wlroots compositors using wlr-randr";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    { self, nixpkgs, crane }:
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
          inherit (pkgs) lib;
          manifest = (lib.importTOML ./Cargo.toml).package;
          craneLib = crane.mkLib pkgs;
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              ./tests/fixtures
            ];
          };
          commonArgs = {
            inherit src;
            pname = manifest.name;
            version = manifest.version;
            strictDeps = true;
            nativeBuildInputs = [ pkgs.makeBinaryWrapper ];
          };
          # Compiled dependencies, cached across source-only changes.
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          wlr-monitor = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              # Tests run with `cargo test`; building them again here doubles
              # the release build time.
              doCheck = false;
              postFixup = ''
                wrapProgram $out/bin/wlr-monitor \
                  --prefix PATH : ${lib.makeBinPath [ pkgs.wlr-randr ]}
              '';
              meta = {
                description = "A TUI monitor-layout editor for wlroots compositors";
                homepage = "https://github.com/dcalonge/wlr-monitor";
                license = lib.licenses.mit;
                mainProgram = "wlr-monitor";
                platforms = lib.platforms.linux;
              };
            }
          );
        in
        {
          default = wlr-monitor;
          wlr-monitor = wlr-monitor;
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.wlr-randr
          ];
          env.RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });

      # Home Manager module: installs the package, creates the writable
      # include files and hooks them into the mango / sway configs.
      homeModules.default = import ./nix/hm-module.nix self;
      homeManagerModules.default = self.homeModules.default;

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
