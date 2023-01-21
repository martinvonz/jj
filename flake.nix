{
  description = "jujutsu";

  inputs = {
    # For installing non-standard rustc versions
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, rust-overlay, ... }:
    let
      lib = nixpkgs.lib;
      systems = [
        "aarch64-linux"
        "aarch64-darwin"
        "i686-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      foreachSystem = f: lib.foldl' (attrs: system: lib.recursiveUpdate attrs (f system)) { } systems;
    in
    {
      overlays.default = (final: prev: {
        jujutsu = final.callPackage
          (
            { stdenv
            , lib
            , fetchFromGitHub
            , rustPlatform
            , pkg-config
            , openssl
            , dbus
            , sqlite
            , file
            , gzip
            , makeWrapper
            , Security
            , SystemConfiguration
            , libiconv
            , installShellFiles
            }:

            rustPlatform.buildRustPackage rec {
              pname = "jujutsu";
              version = "unstable-${self.shortRev or "dirty"}";
              buildNoDefaultFeatures = true;
              buildFeatures = [ "jujutsu-lib/legacy-thrift" ];

              src = self;

              cargoLock = {
                lockFile = "${self}/Cargo.lock";
                outputHashes = {
                  "renderdag-0.1.0" = "sha256-isOd0QBs5StHpj0xRwSPG40juNvnntyHPW7mT4zsPbM";
                };
              };
              nativeBuildInputs = [
                gzip
                installShellFiles
                makeWrapper
                pkg-config
              ];
              buildInputs = [ openssl dbus sqlite ]
              ++ lib.optionals stdenv.isDarwin [
                Security
                SystemConfiguration
                libiconv
              ];
              postInstall = ''
                $out/bin/jj debug mangen > ./jj.1
                installManPage ./jj.1

                $out/bin/jj debug completion --bash > ./completions.bash
                installShellCompletion --bash --name ${pname}.bash ./completions.bash
                $out/bin/jj debug completion --fish > ./completions.fish
                installShellCompletion --fish --name ${pname}.fish ./completions.fish
                $out/bin/jj debug completion --zsh > ./completions.zsh
                installShellCompletion --zsh --name _${pname} ./completions.zsh
              '';
            }
          )
          {
            inherit (final.darwin.apple_sdk.frameworks) Security SystemConfiguration;
          };
      });
    } //
    (foreachSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            self.overlays.default
            rust-overlay.overlays.default
          ];
        };
      in
      {
        packages.${system} = {
          jujutsu = pkgs.jujutsu;
          default = self.packages.${system}.jujutsu;
        };
        apps.${system}.default = {
          type = "app";
          program = "${pkgs.jujutsu}/bin/jj";
        };
        checks.${system}.jujutsu = pkgs.jujutsu.overrideAttrs ({ ... }: {
          cargoBuildType = "debug";
          cargoCheckType = "debug";
          preCheck = ''
            export RUST_BACKTRACE=1
          '';
        });
        formatter.${system} = pkgs.nixpkgs-fmt;
        devShells.${system}.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            # Using the minimal profile with explicit "clippy" extension to avoid
            # two versions of rustfmt
            (rust-bin.stable."1.61.0".minimal.override {
              extensions = [
                "rust-src" # for rust-analyzer
                "clippy"
              ];
            })

            # The CI checks against the latest nightly rustfmt, so we should too.
            (rust-bin.selectLatestNightlyWith (toolchain: toolchain.rustfmt))

            # Required build dependencies
            openssl
            pkg-config # to find openssl

            # Additional tools recommended by contributing.md
            cargo-deny
            cargo-insta
            cargo-nextest
            cargo-watch
          ];
        };
      }));
}
