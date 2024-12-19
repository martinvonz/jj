{
  description = "Jujutsu VCS, a Git-compatible DVCS that is both simple and powerful";

  inputs = {
    # For listing and iterating nix systems
    flake-utils.url = "github:numtide/flake-utils";

    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    # For installing non-standard rustc versions
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
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

      ourRustVersion = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default);

      ourRustPlatform = pkgs.makeRustPlatform {
        rustc = ourRustVersion;
        cargo = ourRustVersion;
      };

      # these are needed in both devShell and buildInputs
      darwinDeps = with pkgs; lib.optionals stdenv.isDarwin [
        darwin.apple_sdk.frameworks.Security
        darwin.apple_sdk.frameworks.SystemConfiguration
        libiconv
      ];

      # these are needed in both devShell and buildInputs
      linuxNativeDeps = with pkgs; lib.optionals stdenv.isLinux [
        mold-wrapped
      ];

      # on macOS and Linux, use faster parallel linkers that are much more
      # efficient than the defaults. these noticeably improve link time even for
      # medium sized rust projects like jj
      rustLinkerFlags =
        if pkgs.stdenv.isLinux then
          [ "-fuse-ld=mold" "-Wl,--compress-debug-sections=zstd" ]
        else if pkgs.stdenv.isDarwin then
          # on darwin, /usr/bin/ld actually looks at the environment variable
          # $DEVELOPER_DIR, which is set by the nix stdenv, and if set,
          # automatically uses it to route the `ld` invocation to the binary
          # within. in the devShell though, that isn't what we want; it's
          # functional, but Xcode's linker as of ~v15 (not yet open source)
          # is ultra-fast and very shiny; it is enabled via -ld_new, and on by
          # default as of v16+
          [ "--ld-path=$(unset DEVELOPER_DIR; /usr/bin/xcrun --find ld)" "-ld_new" ]
        else
          [ ];

      rustLinkFlagsString = pkgs.lib.concatStringsSep " " (pkgs.lib.concatMap (x:
        [ "-C" "link-arg=${x}" ]
      ) rustLinkerFlags);
    in
    {
      packages = {
        jujutsu = ourRustPlatform.buildRustPackage {
          pname = "jujutsu";
          version = "unstable-${self.shortRev or "dirty"}";

          buildFeatures = [ "packaging" ];
          cargoBuildFlags = [ "--bin" "jj" ]; # don't build and install the fake editors
          useNextest = true;
          src = filterSrc ./. [
            ".*\\.nix$"
            "^.jj/"
            "^flake\\.lock$"
            "^target/"
          ];

          cargoLock.lockFile = ./Cargo.lock;
          cargoLock.outputHashes = {
            "git2-0.19.0" = "sha256-fV8dFChGeDhb20bMyqefpAD5/+raQQ2sMdkEtlA1jaE=";
          };
          nativeBuildInputs = with pkgs; [
            gzip
            installShellFiles
            makeWrapper
            pkg-config

            # for libz-ng-sys (zlib-ng)
            # TODO: switch to the packaged zlib-ng and drop this dependency
            cmake

            # for signing tests
            gnupg
            openssh
          ] ++ linuxNativeDeps;
          buildInputs = with pkgs; [
            openssl zstd libgit2 openssh
          ] ++ darwinDeps;

          ZSTD_SYS_USE_PKG_CONFIG = "1";
          RUSTFLAGS = pkgs.lib.optionalString pkgs.stdenv.isLinux "-C link-arg=-fuse-ld=mold";
          NIX_JJ_GIT_HASH = self.rev or "";
          CARGO_INCREMENTAL = "0";

          preCheck = ''
            export RUST_BACKTRACE=1
          '';

          postInstall = ''
            $out/bin/jj util mangen > ./jj.1
            installManPage ./jj.1

            installShellCompletion --cmd jj \
              --bash <($out/bin/jj util completion bash) \
              --fish <($out/bin/jj util completion fish) \
              --zsh <($out/bin/jj util completion zsh)
          '';

          meta = {
            description = "Git-compatible DVCS that is both simple and powerful";
            homepage = "https://github.com/jj-vcs/jj";
            license = pkgs.lib.licenses.asl20;
            mainProgram = "jj";
          };
        };
        default = self.packages.${system}.jujutsu;
      };

      formatter = pkgs.nixpkgs-fmt;

      checks.jujutsu = self.packages.${system}.jujutsu.overrideAttrs ({ ... }: {
        # FIXME (aseipp): when running `nix flake check`, this will override the
        # main package, and nerf the build and installation phases. this is
        # because for some inexplicable reason, the cargo cache gets invalidated
        # in between buildPhase and checkPhase, causing every nix CI build to be
        # 2x as long.
        #
        # upstream issue: https://github.com/NixOS/nixpkgs/issues/291222
        buildPhase = "true";
        installPhase = "touch $out";
        # NOTE (aseipp): buildRustPackage also, by default, runs `cargo check`
        # in `--release` mode, which is far slower; the existing CI builds all
        # use the default `test` profile, so we should too.
        cargoCheckType = "test";
      });

      devShells.default = pkgs.mkShell {
        packages = with pkgs; [
          # NOTE (aseipp): explicitly add rust-src to the rustc compiler only in
          # devShell. this in turn causes a dependency on the rust compiler src,
          # which bloats the closure size by several GiB. but doing this here
          # and not by default avoids the default flake install from including
          # that dependency, so it's worth it
          #
          # relevant PR: https://github.com/rust-lang/rust/pull/129687
          (ourRustVersion.override {
            extensions = [ "rust-src" "rust-analyzer" ];
          })

          # Foreign dependencies
          openssl zstd libgit2
          pkg-config

          # Additional tools recommended by contributing.md
          cargo-deny
          cargo-insta
          cargo-nextest
          cargo-watch

          # Miscellaneous tools
          watchman

          # In case you need to run `cargo run --bin gen-protos`
          protobuf

          # for libz-ng-sys (zlib-ng)
          # TODO: switch to the packaged zlib-ng and drop this dependency
          cmake

          # To run the signing tests
          gnupg
          openssh

          # For building the documentation website
          uv
        ] ++ darwinDeps ++ linuxNativeDeps;

        shellHook = ''
          export RUST_BACKTRACE=1
          export ZSTD_SYS_USE_PKG_CONFIG=1

          export RUSTFLAGS="-Zthreads=0 ${rustLinkFlagsString}"
        '';
      };
    }));
}
