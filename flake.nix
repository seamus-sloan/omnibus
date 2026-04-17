{
  description = "Omnibus full-stack Rust app dev environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs-unstable.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };
  };

  outputs = { self, nixpkgs, nixpkgs-unstable, flake-utils, fenix }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        pkgs-unstable = import nixpkgs-unstable { inherit system; };

        # Rust toolchain with mobile cross-compilation targets pre-installed.
        # fenix packages are read-only Nix store paths, so dx can't call rustup
        # to install them at runtime — they must be declared here instead.
        rust = fenix.packages.${system}.combine ([
          fenix.packages.${system}.latest.cargo
          fenix.packages.${system}.latest.rustc
          fenix.packages.${system}.latest.rustfmt
          fenix.packages.${system}.latest.clippy
          fenix.packages.${system}.latest.rust-src
          fenix.packages.${system}.targets.aarch64-linux-android.latest.rust-std
        ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
          fenix.packages.${system}.targets.aarch64-apple-ios.latest.rust-std
          fenix.packages.${system}.targets.aarch64-apple-ios-sim.latest.rust-std
        ]);
      in {
        devShells.default = pkgs.mkShell {
          packages = [
            pkgs-unstable.git
            pkgs.sqlite
            pkgs.pkg-config
            pkgs.openssl
            rust
            pkgs.jdk21
            pkgs-unstable.dioxus-cli
            pkgs-unstable.nodejs_22
          ];

          DATABASE_URL = "sqlite://omnibus.db?mode=rwc";

          shellHook = ''
            echo "Nix dev shell ready."
            echo "Run: cargo test -p omnibus"
            echo "Run: cargo run -p omnibus"

            # Auto-detect Android SDK + NDK on macOS.
            if [ -z "$ANDROID_HOME" ]; then
              for sdk_base in \
                "$HOME/Library/Android/sdk" \
                "$HOME/Android/Sdk"; do
                if [ -d "$sdk_base" ]; then
                  export ANDROID_HOME="$sdk_base"
                  echo "ANDROID_HOME=$ANDROID_HOME"
                  break
                fi
              done
            fi
            if [ -z "$ANDROID_NDK_HOME" ] && [ -n "$ANDROID_HOME" ] && [ -d "$ANDROID_HOME/ndk" ]; then
              _ndk=$(ls -d "$ANDROID_HOME/ndk/"* 2>/dev/null | sort -V | tail -1)
              [ -n "$_ndk" ] && export ANDROID_NDK_HOME="$_ndk" && echo "ANDROID_NDK_HOME=$ANDROID_NDK_HOME"
            fi
          '';
        };
      });
}
