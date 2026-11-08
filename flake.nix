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

      # GitHub release asset name per Nix system.
      binAsset = {
        "x86_64-linux" = "x86_64-unknown-linux-gnu";
        "aarch64-linux" = "aarch64-unknown-linux-gnu";
      };
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          inherit (pkgs) lib;
          manifest = (lib.importTOML ./Cargo.toml).package;
          wlr-monitor = pkgs.rustPlatform.buildRustPackage {
            pname = manifest.name;
            inherit (manifest) version;
            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./src
                ./tests/fixtures
              ];
            };
            cargoLock.lockFile = ./Cargo.lock;

            # A compiled wrapper: no shell start-up on every launch.
            nativeBuildInputs = [ pkgs.makeBinaryWrapper ];
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
          };

          # Prebuilt binary from the latest GitHub Release.
          #
          # sha256 is fake — pass --impure to skip the check (required since
          # "latest" changes with every release).
          #
          # On NixOS the binary is dynamically linked against system glibc,
          # so it may need `pkgs.makeRunCapable` (or just use the default
          # source build above).
          wlr-monitor-bin = pkgs.fetchurl {
            url = "https://github.com/dcalonge/wlr-monitor/releases/latest/download/wlr-monitor-${binAsset.${pkgs.stdenv.hostPlatform.system}}";
            sha256 = lib.fakeSha256;
          };
        in
        {
          default = wlr-monitor;
          wlr-monitor = wlr-monitor;
          wlr-monitor-bin = wlr-monitor-bin;
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
