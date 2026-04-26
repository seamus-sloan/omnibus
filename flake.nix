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
          fenix.packages.${system}.targets.wasm32-unknown-unknown.latest.rust-std
        ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
          fenix.packages.${system}.targets.aarch64-apple-ios.latest.rust-std
          fenix.packages.${system}.targets.aarch64-apple-ios-sim.latest.rust-std
        ]);

        # `dioxus-cli` from nixpkgs-unstable bundles `wasm-bindgen-cli 0.2.114`
        # (appended to PATH via `--suffix`), but `dioxus-fullstack 0.7.5` pulls
        # in `wasm-bindgen 0.2.118` transitively (js-sys 0.3.95 hard-pins it).
        # Build the matching CLI ourselves and put it earlier in PATH so dx
        # picks it up first.
        wasm-bindgen-cli-0_2_118 = pkgs-unstable.wasm-bindgen-cli.overrideAttrs (old: rec {
          version = "0.2.118";
          src = pkgs.fetchCrate {
            pname = "wasm-bindgen-cli";
            inherit version;
            hash = "sha256-ve783oYH0TGv8Z8lIPdGjItzeLDQLOT5uv/jbFOlZpI=";
          };
          cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
            inherit src;
            name = "wasm-bindgen-cli-${version}-vendor";
            hash = "sha256-EYDfuBlH3zmTxACBL+sjicRna84CvoesKSQVcYiG9P0=";
          };
        });
      in {
        devShells.default = pkgs.mkShell {
          packages = [
            pkgs-unstable.git
            pkgs.sqlite
            pkgs.pkg-config
            pkgs.openssl
            rust
            pkgs.jdk21
            wasm-bindgen-cli-0_2_118
            pkgs-unstable.dioxus-cli
            pkgs-unstable.nodejs_22
            pkgs-unstable.playwright-driver.browsers
            pkgs.zellij
            pkgs.process-compose
            pkgs.just
          ];

          DATABASE_URL = "sqlite://omnibus.db?mode=rwc";

          shellHook = ''
            echo "Nix dev shell ready."
            echo "Run: cargo test -p omnibus"
            echo "Run: cargo run -p omnibus"

            # Keep target/ out of the repo so flake evaluations don't snapshot
            # ~12GB of build artifacts into /nix/store on every direnv reload.
            # Resolve the repo root so `nix develop` from a subdir lands in the
            # same target dir; basename keeps it per-worktree so parallel jj
            # workspaces don't race.
            _cargo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
            export CARGO_TARGET_DIR="$HOME/.cache/cargo-target/$(basename "$_cargo_root")"

            # Pin Playwright's Chromium to the Nix store so no per-user
            # download lands in ~/Library/Caches/ms-playwright/. The npm
            # @playwright/test version must match this bundle's version.
            export PLAYWRIGHT_BROWSERS_PATH="${pkgs-unstable.playwright-driver.browsers}"
            export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1

            # `dx serve --fullstack` runs an HTTP proxy on $PORT that
            # rewrites Host: to the upstream backend's loopback address,
            # without setting X-Forwarded-Host. The CSRF origin_check
            # middleware then sees Origin=http://localhost:3000 vs
            # Host=127.0.0.1:<random>, calls it a mismatch, and 403s every
            # cookie-authed POST. Declare an allowlist so origin_check can
            # match the browser's Origin directly. Override or extend in
            # production deployments.
            export OMNIBUS_PUBLIC_ORIGIN="http://localhost:''${PORT:-3000}"

            # Nix injects xcbuild's fake xcrun and its own cc wrapper, both of which
            # break iOS builds. Fix: prepend /usr/bin so the real Xcode xcrun and
            # Apple clang shadow Nix's stubs. Set DEVELOPER_DIR so the real xcrun
            # can locate all platform SDKs (including iphonesimulator). Set SDKROOT
            # to the Xcode macOS SDK so Apple clang and xcrun agree on the sysroot.
            # Rust (fenix) uses absolute store paths and is unaffected by PATH order.
            if [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
              export PATH="/usr/bin:$PATH"
              export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
              export SDKROOT="$DEVELOPER_DIR/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"
              echo "DEVELOPER_DIR=$DEVELOPER_DIR"
            fi

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
