{
  description = "jujitsu";

  inputs.nixpkgs-mozilla.url = "github:mozilla/nixpkgs-mozilla";

  outputs = { self, nixpkgs, nixpkgs-mozilla, ... }:
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
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
    in
    {
      overlay = (final: prev: {
        jujitsu = final.callPackage
          (
            { stdenv
            , lib
            , fetchFromGitHub
            , rustPlatform
            , pkgconfig
            , openssl
            , dbus
            , sqlite
            , file
            , gzip
            , makeWrapper
            , Security
            , SystemConfiguration
            , libiconv
            , rust
            }:

            rustPlatform.buildRustPackage rec {
              pname = "jujutsu";
              inherit version;

              src = self;

              cargoLock = {
                lockFile = "${self}/Cargo.lock";
              };
              nativeBuildInputs = [ rust pkgconfig gzip makeWrapper ];
              buildInputs = [ openssl dbus sqlite ]
              ++ lib.optionals stdenv.isDarwin [
                Security
                SystemConfiguration
                libiconv
              ];
            }

          )
          {
            rust = (final.lib.rustLib.fromManifestFile ./toolchain-manifest.toml { inherit (final) stdenv lib fetchurl patchelf; }).rust;
            inherit (final.darwin.apple_sdk.frameworks) Security SystemConfiguration;
          };
      });
    } //
    (foreachSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ nixpkgs-mozilla.overlays.rust self.overlay ];
        };
        updateToolchainManifest = pkgs.writeScriptBin "updateToolchainManifest" ''
          #! /usr/bin/env bash

          set -ex
          
          if [[ -z $1 ]]; then
            channel='"nightly"'
          else
            channel="\"$1\""
          fi

          if [[ -z $2 ]]; then
            date='null'
          else
            date="\"$2\""
          fi

          url=$(nix eval --raw --impure --expr "let flake = (builtins.getFlake (builtins.toString ./.)); in (import flake.inputs.nixpkgs { overlays = [ flake.inputs.nixpkgs-mozilla.overlays.rust ]; }).lib.rustLib.manifest_v2_url { channel = $channel; date = $date; }")
          curl $url > ./toolchain-manifest.toml
        '';
      in
      {
        devShell.${system} = pkgs.mkShell {
          inputsFrom = [ pkgs.jujitsu ];
          packages = [ updateToolchainManifest ];
        };
        packages.${system}.jujitsu = pkgs.jujitsu;
        defaultPackage.${system} = self.packages.${system}.jujitsu;
        checks.${system}.jujitsu = pkgs.jujitsu.overrideAttrs ({ ... }: {
          cargoBuildType = "debug";
          cargoCheckType = "debug";
          preCheck = ''
            export RUST_BACKTRACE=1
          '';
        });
      }));
}
