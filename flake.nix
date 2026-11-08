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

      # Prebuilt binary version. Automatically updated by the release
      # workflow (.github/workflows/release.yml) on every version tag.
      binVersion = "0.3.0";
      # Maps Nix system (x86_64-linux, aarch64-linux) to the GitHub
      # release asset name (x86_64-unknown-linux-gnu, …).
      binAssetName = {
        "x86_64-linux" = "x86_64-unknown-linux-gnu";
        "aarch64-linux" = "aarch64-unknown-linux-gnu";
      };
      binSha256 = {
        "x86_64-linux" = "sha256-bF3htuySWT7YGGhi8aUIAdAc6tdoEgbQDjsteGmfKQo=";
        "aarch64-linux" = "sha256-8TqjQfg04kvvNJvbflVilNoEeX0sjtwdvdYZSgC99W4=";
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

          # Prebuilt binary from a GitHub release, pinned to a version.
          # Replace the fake sha256 with the real one from nix-prefetch-url.
          wlr-monitor-bin = let
            system = pkgs.stdenv.hostPlatform.system;
            dynamicLinker = if system == "x86_64-linux" then
              "${pkgs.glibc}/lib/ld-linux-x86-64.so.2"
            else
              "${pkgs.glibc}/lib/ld-linux-aarch64.so.1";
          in
          pkgs.runCommand "wlr-monitor-${binVersion}" { nativeBuildInputs = [ pkgs.patchelf pkgs.makeWrapper pkgs.glibc ]; }
            ''
              mkdir -p $out/bin
              install -m 755 ${pkgs.fetchurl {
                url = "https://github.com/dcalonge/wlr-monitor/releases/download/v${binVersion}/wlr-monitor-${binAssetName.${system}}";
                sha256 = binSha256.${system};
              }} $out/bin/wlr-monitor
              patchelf --set-interpreter ${dynamicLinker} $out/bin/wlr-monitor
              patchelf --set-rpath ${pkgs.lib.makeLibraryPath [ pkgs.glibc ]} $out/bin/wlr-monitor
              wrapProgram $out/bin/wlr-monitor \
                --prefix PATH : ${lib.makeBinPath [ pkgs.wlr-randr ]}
            '';

          # Prebuilt binary from the latest GitHub release.
          # Requires --impure when evaluating the flake (network access).
          wlr-monitor-bin-latest = let
            system = pkgs.stdenv.hostPlatform.system;
            dynamicLinker = if system == "x86_64-linux" then
              "${pkgs.glibc}/lib/ld-linux-x86-64.so.2"
            else
              "${pkgs.glibc}/lib/ld-linux-aarch64.so.1";
            latest = builtins.fromJSON (builtins.fetchurl {
              url = "https://api.github.com/repos/dcalonge/wlr-monitor/releases/latest";
            });
            version = latest.tag_name;
          in
          pkgs.runCommand "wlr-monitor-bin-latest" { nativeBuildInputs = [ pkgs.patchelf pkgs.makeWrapper pkgs.glibc ]; }
            ''
              mkdir -p $out/bin
              install -m 755 ${pkgs.fetchurl {
                url = "https://github.com/dcalonge/wlr-monitor/releases/download/${version}/wlr-monitor-${binAssetName.${system}}";
                sha256 = lib.fakeSha256;
              }} $out/bin/wlr-monitor
              patchelf --set-interpreter ${dynamicLinker} $out/bin/wlr-monitor
              patchelf --set-rpath ${pkgs.lib.makeLibraryPath [ pkgs.glibc ]} $out/bin/wlr-monitor
              wrapProgram $out/bin/wlr-monitor \
                --prefix PATH : ${lib.makeBinPath [ pkgs.wlr-randr ]}
            '';
        in
        {
          default = wlr-monitor;
          wlr-monitor = wlr-monitor;
          wlr-monitor-bin = wlr-monitor-bin;
          wlr-monitor-bin-latest = wlr-monitor-bin-latest;
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
