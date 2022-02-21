{
  description = "jujutsu";

  outputs = { self, nixpkgs, ... }:
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
      overlay = (final: prev: {
        jujutsu = final.callPackage
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
            }:

            rustPlatform.buildRustPackage rec {
              pname = "jujutsu";
              version = "unstable-${self.shortRev or "dirty"}";

              src = self;

              cargoLock = {
                lockFile = "${self}/Cargo.lock";
              };
              nativeBuildInputs = [ pkgconfig gzip makeWrapper ];
              buildInputs = [ openssl dbus sqlite ]
              ++ lib.optionals stdenv.isDarwin [
                Security
                SystemConfiguration
                libiconv
              ];
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
          overlays = [ self.overlay ];
        };
      in
      {
        packages.${system}.jujutsu = pkgs.jujutsu;
        defaultPackage.${system} = self.packages.${system}.jujutsu;
        defaultApp.${system} = {
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
      }));
}
