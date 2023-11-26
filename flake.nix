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

      rust-version = pkgs.rust-bin.stable."1.71.0".default;

      ourRustPlatform = pkgs.makeRustPlatform {
        rustc = rust-version;
        cargo = rust-version;
      };

      # these are needed in both devShell and buildInputs
      darwinDeps = with pkgs; lib.optionals stdenv.isDarwin [
        darwin.apple_sdk.frameworks.Security
        darwin.apple_sdk.frameworks.SystemConfiguration
        libiconv
      ];

      # NOTE (aseipp): on Linux, go ahead and use mold by default to improve
      # link times a bit; mostly useful for debug build speed, but will help
      # over time if we ever get more dependencies, too
      useMoldLinker = pkgs.stdenv.isLinux;

      # these are needed in both devShell and buildInputs
      linuxNativeDeps = with pkgs; lib.optionals stdenv.isLinux [
        mold-wrapped
      ];

    in
    {
      packages = {
        jujutsu = ourRustPlatform.buildRustPackage {
          pname = "jujutsu";
          version = "unstable-${self.shortRev or "dirty"}";

          buildFeatures = [ "packaging" ];
          cargoBuildFlags = ["--bin" "jj"]; # don't build and install the fake editors
          useNextest = true;
          src = filterSrc ./. [
            ".*\\.nix$"
            "^.jj/"
            "^flake\\.lock$"
            "^target/"
          ];

          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = with pkgs; [
            gzip
            installShellFiles
            makeWrapper
            pkg-config
            gnupg # for signing tests
          ] ++ linuxNativeDeps;
          buildInputs = with pkgs; [
            openssl zstd libgit2 libssh2
          ] ++ darwinDeps;

          ZSTD_SYS_USE_PKG_CONFIG = "1";
          LIBSSH2_SYS_USE_PKG_CONFIG = "1";
          RUSTFLAGS = pkgs.lib.optionalString useMoldLinker "-C link-arg=-fuse-ld=mold";
          NIX_JJ_GIT_HASH = self.rev or "";
          CARGO_INCREMENTAL = "0";

          preCheck = "export RUST_BACKTRACE=1";
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
      formatter = pkgs.nixpkgs-fmt;
      devShells.default = pkgs.mkShell {
        buildInputs = with pkgs; [
          # The CI checks against the latest nightly rustfmt, so we should too.
          # NOTE (aseipp): include this FIRST before the rust-version override
          # below; otherwise, it will be overridden by the rust-version and
          # we'll get stable rustfmt instead.
          (rust-bin.selectLatestNightlyWith (toolchain: toolchain.rustfmt))

          # Using the minimal profile with explicit "clippy" extension to avoid
          # two versions of rustfmt
          (rust-version.override {
            extensions = [
              "rust-src" # for rust-analyzer
              "clippy"
            ];
          })

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

          # In case you need to run `cargo run --bin gen-protos`
          protobuf

          # To run the signing tests
          gnupg

          # For building the documentation website
          poetry
        ] ++ darwinDeps ++ linuxNativeDeps;

        shellHook = ''
          export RUST_BACKTRACE=1
          export ZSTD_SYS_USE_PKG_CONFIG=1
          export LIBSSH2_SYS_USE_PKG_CONFIG=1
        '' + pkgs.lib.optionalString useMoldLinker ''
          export RUSTFLAGS="-C link-arg=-fuse-ld=mold $RUSTFLAGS"
        '';
      };
    }));
}
