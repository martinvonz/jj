{
  description = "Jujutsu VCS, a Git-compatible DVCS that is both simple and powerful";

  inputs = {
    # For listing and iterating nix systems
    flake-utils.url = "github:numtide/flake-utils";

    # For installing non-standard rustc versions
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    rust-overlay.inputs.flake-utils.follows = "flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }: {
    overlays.default = (final: prev: {
      jujutsu = self.packages.${final.system}.jujutsu;
    });
  } //
  (flake-utils.lib.eachDefaultSystem (system:
    let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [
          rust-overlay.overlays.default
        ];
      };
      filterSrc = src: regexes:
        pkgs.lib.cleanSourceWith {
          inherit src;
          filter = path: type:
            let
              relPath = pkgs.lib.removePrefix (toString src + "/") (toString path);
            in
            pkgs.lib.all (re: builtins.match re relPath == null) regexes;
        };
    in
    {
      packages = {
        jujutsu = pkgs.rustPlatform.buildRustPackage rec {
          pname = "jujutsu";
          version = "unstable-${self.shortRev or "dirty"}";
          buildNoDefaultFeatures = true;
          buildFeatures = [ "watchman" ];
          cargoBuildFlags = [ "--bin" "jj" ]; # don't build and install the fake editors
          useNextest = true;
          src = filterSrc ./. [
            ".*\\.nix$"
            "^.jj/"
            "^flake\\.lock$"
            "^target/"
          ];
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
          nativeBuildInputs = with pkgs; [
            gzip
            installShellFiles
            makeWrapper
            pkg-config
          ];
          buildInputs = with pkgs; [ openssl zstd libgit2 libssh2 ]
            ++ lib.optionals stdenv.isDarwin [
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
            libiconv
            ];
          ZSTD_SYS_USE_PKG_CONFIG = "1";
          LIBSSH2_SYS_USE_PKG_CONFIG = "1";
          NIX_JJ_GIT_HASH = self.rev or "";
          CARGO_INCREMENTAL = "0";
          postInstall = ''
            $out/bin/jj util mangen > ./jj.1
            installManPage ./jj.1

            installShellCompletion --cmd jj \
              --bash <($out/bin/jj util completion --bash) \
              --fish <($out/bin/jj util completion --fish) \
              --zsh <($out/bin/jj util completion --zsh)
          '';
        };
        default = self.packages.${system}.jujutsu;
      };
      apps.default = {
        type = "app";
        program = "${self.packages.${system}.jujutsu}/bin/jj";
      };
      checks.jujutsu = self.packages.${system}.jujutsu.overrideAttrs ({ ... }: {
        cargoBuildType = "debug";
        cargoCheckType = "debug";
        preCheck = ''
          export RUST_BACKTRACE=1
        '';
      });
      formatter = pkgs.nixpkgs-fmt;
      devShells.default = pkgs.mkShell {
        buildInputs = with pkgs; [
          # Using the minimal profile with explicit "clippy" extension to avoid
          # two versions of rustfmt
          (rust-bin.stable."1.64.0".minimal.override {
            extensions = [
              "rust-src" # for rust-analyzer
              "clippy"
            ];
          })

          # The CI checks against the latest nightly rustfmt, so we should too.
          (rust-bin.selectLatestNightlyWith (toolchain: toolchain.rustfmt))

          # Foreign dependencies
          openssl zstd libgit2 libssh2
          pkg-config

          # Make sure rust-analyzer is present
          rust-analyzer

          # Additional tools recommended by contributing.md
          cargo-deny
          cargo-insta
          cargo-nextest
          cargo-watch
        ];

        shellHook = ''
          export RUST_BACKTRACE=1
          export ZSTD_SYS_USE_PKG_CONFIG=1
          export LIBSSH2_SYS_USE_PKG_CONFIG=1
        '';
      };
    }));
}
