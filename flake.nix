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

        # Slim core Rust toolchain. wasm32 rust-std is kept here because it's
        # small (single-digit MB) and lets rust-analyzer / `cargo check` exercise
        # the frontend's WASM target without flipping into the `web` shell.
        rustCore = fenix.packages.${system}.combine [
          fenix.packages.${system}.latest.cargo
          fenix.packages.${system}.latest.rustc
          fenix.packages.${system}.latest.rustfmt
          fenix.packages.${system}.latest.clippy
          fenix.packages.${system}.latest.rust-src
          fenix.packages.${system}.targets.wasm32-unknown-unknown.latest.rust-std
        ];

        # Mobile-only Rust cross-compilation targets. Pulled into the `mobile`
        # shell only — daily cargo work doesn't pay for them.
        # fenix packages are read-only Nix store paths, so dx can't call rustup
        # to install them at runtime — they must be declared here instead.
        rustMobileTargets = [
          fenix.packages.${system}.targets.aarch64-linux-android.latest.rust-std
        ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
          fenix.packages.${system}.targets.aarch64-apple-ios.latest.rust-std
          fenix.packages.${system}.targets.aarch64-apple-ios-sim.latest.rust-std
        ];

        # Slim core + mobile targets, combined into a single fenix toolchain so
        # `dx` and `cargo` see one rustc with every needed --target.
        rustMobile = fenix.packages.${system}.combine ([
          fenix.packages.${system}.latest.cargo
          fenix.packages.${system}.latest.rustc
          fenix.packages.${system}.latest.rustfmt
          fenix.packages.${system}.latest.clippy
          fenix.packages.${system}.latest.rust-src
          fenix.packages.${system}.targets.wasm32-unknown-unknown.latest.rust-std
        ] ++ rustMobileTargets);

        # `dioxus-cli` from nixpkgs-unstable bundles its own `wasm-bindgen-cli`
        # (appended to PATH via `--suffix`), but the `dioxus` git pin in
        # Cargo.toml (v0.7.9 monorepo tag) pulls in `wasm-bindgen 0.2.122`
        # transitively, and nixpkgs-unstable only ships 0.2.121. `dx` requires
        # the CLI version to match the locked crate, so we supply 0.2.122 and
        # put it earlier in PATH.
        #
        # We install the upstream *prebuilt* CLI binary from the GitHub release
        # rather than building it from source. Building from source (via
        # `fetchCrate` + `fetchCargoVendor`) downloads the crate and every one
        # of its dependencies from the crates.io app endpoint
        # (https://crates.io/api/v1/crates/.../download), which enforces
        # User-Agent policy + rate limits and intermittently 403s the Nix
        # fetcher on cold-cache CI runs — that failure aborts the whole
        # `nix develop` shell before any cargo command runs. GitHub release
        # assets are a plain CDN with no such gating and touch crates.io zero
        # times. Linux uses the static musl build so no dynamic-linker patching
        # is needed inside the Nix sandbox.
        wasm-bindgen-cli-0_2_122 =
          let
            version = "0.2.122";
            plat = {
              x86_64-linux = {
                triple = "x86_64-unknown-linux-musl";
                hash = "sha256-Eio/4uqcbj6JtQ5C3Ro0bkmfP2WlSq4LTq62WBOdHg4=";
              };
              aarch64-linux = {
                triple = "aarch64-unknown-linux-musl";
                hash = "sha256-sd/kcq8O4SzaCHoLKJhd87UXQV2a5GfLz4USMbzoVts=";
              };
              x86_64-darwin = {
                triple = "x86_64-apple-darwin";
                hash = "sha256-y1AKayScIEdLvdhtTY1ZP6Zcnl29Vquxrn7kjAg5ChE=";
              };
              aarch64-darwin = {
                triple = "aarch64-apple-darwin";
                hash = "sha256-Nr6tyGxcAsURoVLo/CxjhQZGiH6X3JSuBFU2QPg56Ag=";
              };
            }.${system} or (throw "wasm-bindgen-cli ${version}: unsupported system ${system}");
          in
          pkgs.stdenvNoCC.mkDerivation {
            pname = "wasm-bindgen-cli";
            inherit version;
            src = pkgs.fetchurl {
              url = "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${version}/wasm-bindgen-${version}-${plat.triple}.tar.gz";
              inherit (plat) hash;
            };
            # The tarball is already-built binaries — only unpack + install.
            dontConfigure = true;
            dontBuild = true;
            installPhase = ''
              runHook preInstall
              mkdir -p $out/bin
              cp wasm-bindgen wasm2es6js wasm-bindgen-test-runner $out/bin/
              chmod +x $out/bin/*
              runHook postInstall
            '';
          };

        # Package buckets per shell. The slim default carries only what
        # everyday cargo/rust-analyzer work needs; opt-in shells layer their
        # extras on top. zellij + process-compose live in `default` so
        # `just serve` / `just serve-pc` work from a fresh checkout without
        # an extra bootstrap step — they're ~10 MB each and harmless.
        slimPackages = [
          pkgs-unstable.git
          pkgs.sqlite
          pkgs.pkg-config
          pkgs.openssl
          rustCore
          pkgs.zellij
          # process-compose pinned to unstable for a fresher build than the
          # v1.64.1 that nixpkgs-stable (25.05) ships. The original driver
          # (`is_interactive: true`) was reverted from process-compose.yaml
          # because it pinned `dx` panes to a PTY that detached from the TUI
          # log pane, but we keep the newer build for general quality-of-life
          # fixes and to leave room for future config keys.
          pkgs-unstable.process-compose
          pkgs.just
        ];

        # Web-build extras: dioxus-cli + matched wasm-bindgen + node + ffmpeg.
        # Node is here (not in `e2e`) because some web-only workflows shell
        # out to `npm`/`npx` (e.g. tooling under `ui_tests/playwright/tools/`).
        # ffmpeg is the runtime backend for the F2.3 audiobook HLS pipeline
        # (`db::hls` shells out via OMNIBUS_FFMPEG_PATH); kept out of slim
        # `default` since daily cargo work doesn't need it.
        webExtras = [
          wasm-bindgen-cli-0_2_122
          pkgs-unstable.dioxus-cli
          pkgs-unstable.nodejs_22
          pkgs.ffmpeg-headless
        ];

        # E2E extras layer on top of webExtras: Playwright's Nix-provided
        # Chromium bundle. The PLAYWRIGHT_BROWSERS_PATH export below pins it.
        e2eExtras = [
          pkgs-unstable.playwright-driver.browsers
        ];

        # Mobile extras: JDK for the Android NDK toolchain. Mobile rust
        # targets are bundled into rustMobile (used in place of rustCore).
        mobileExtras = [
          pkgs.jdk21
        ];

        # CI / occasional-local audit tooling. Not needed for daily work.
        auditExtras = [
          pkgs-unstable.cargo-audit
          pkgs-unstable.cargo-deny
        ];

        # Common shellHook fragment shared by every shell: parks
        # CARGO_TARGET_DIR outside the repo so flake evals don't snapshot
        # ~12 GB of build artifacts into /nix/store, exports the CSRF
        # origin allowlist used by `dx serve --fullstack`, and sources a
        # per-repo `.env` last so it can override any default above.
        commonShellHook = ''
          # Keep target/ out of the repo so flake evaluations don't snapshot
          # ~12GB of build artifacts into /nix/store on every direnv reload.
          # Resolve the repo root so `nix develop` from a subdir lands in the
          # same target dir; basename keeps it per-worktree so parallel
          # worktrees don't race.
          # Skip if the caller already pinned CARGO_TARGET_DIR (CI sets it
          # to ./target so workflow paths and rust-cache stay valid).
          if [ -z "''${CARGO_TARGET_DIR:-}" ]; then
            _cargo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
            export CARGO_TARGET_DIR="$HOME/.cache/cargo-target/$(basename "$_cargo_root")"
          fi

          # Per-workspace stable PORT — each known `jj workspace` gets its
          # own base port so three agents in three workspaces don't compete
          # for the same port. scripts/dev-server-up.sh port-walks within
          # $PORT..$PORT+9 as a fallback for foreign-process / sibling
          # collisions. Override by exporting PORT before `nix develop`.
          if [ -z "''${PORT:-}" ]; then
            _ws_name="$(basename "$(git rev-parse --show-toplevel 2>/dev/null || pwd)")"
            case "$_ws_name" in
              omnibus)        export PORT=3000 ;;
              omnibus-xray)   export PORT=3010 ;;
              omnibus-yankee) export PORT=3020 ;;
              omnibus-zulu)   export PORT=3030 ;;
              *)              export PORT=3000 ;;
            esac
          fi

          # `dx serve --fullstack` runs an HTTP proxy on $PORT that
          # rewrites Host: to the upstream backend's loopback address,
          # without setting X-Forwarded-Host. The CSRF origin_check
          # middleware then sees Origin=http://localhost:3000 vs
          # Host=127.0.0.1:<random>, calls it a mismatch, and 403s every
          # cookie-authed POST. Declare an allowlist so origin_check can
          # match the browser's Origin directly. Override or extend in
          # production deployments.
          export OMNIBUS_PUBLIC_ORIGIN="http://localhost:''${PORT}"

          # Source per-repo .env last so it can override any default set
          # above. Look in the worktree root (resolved via git so
          # `nix develop` from a subdir still picks it up); fall back to
          # the current dir. `.env` is gitignored and is meant for
          # secret-bearing or per-developer values only — non-secret
          # defaults live in this shellHook. `.env.example` is the
          # checked-in template.
          _env_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
          if [ -f "$_env_root/.env" ]; then
            set -a
            # shellcheck disable=SC1090
            source "$_env_root/.env"
            set +a
          fi
        '';

        # `e2e` shell pins Playwright's Chromium to the Nix store. The npm
        # @playwright/test version must match this bundle's version.
        playwrightHook = ''
          export PLAYWRIGHT_BROWSERS_PATH="${pkgs-unstable.playwright-driver.browsers}"
          export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
        '';

        # `mobile` shell auto-detects Xcode + Android SDK/NDK on the host.
        # These detections only make sense when actually building mobile
        # targets, so they live outside the common hook.
        mobileHook = ''
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

        mkShell = { name, packages, extraHook ? "" }:
          pkgs.mkShell {
            inherit packages;
            DATABASE_URL = "sqlite://omnibus.db?mode=rwc";
            shellHook = ''
              echo "Nix dev shell ready (${name})."
            '' + commonShellHook + extraHook;
          };
      in {
        devShells = {
          # Slim default — covers daily cargo/clippy/fmt/test and the
          # `rust.yml/check` CI job. Anything WASM-, mobile-, or
          # Playwright-shaped lives in an opt-in shell below.
          default = mkShell {
            name = "default";
            packages = slimPackages;
          };

          # WASM dev: dioxus-cli, matched wasm-bindgen, node. Used by
          # `just dev-up`, ad-hoc `dx serve --platform web`, and the
          # server pane in the multiplexer.
          web = mkShell {
            name = "web";
            packages = slimPackages ++ webExtras;
          };

          # End-to-end browser tests: web extras + Playwright's Chromium
          # bundle, with PLAYWRIGHT_BROWSERS_PATH pinned to the Nix store.
          # Used by `e2e.yml` and the playwright pane in the multiplexer.
          e2e = mkShell {
            name = "e2e";
            packages = slimPackages ++ webExtras ++ e2eExtras;
            extraHook = playwrightHook;
          };

          # Mobile builds: Android + iOS rust-std targets, JDK 21, and
          # the Xcode/Android SDK auto-detection hook. Used by the
          # mobile panes in the multiplexer and `cargo build -p omnibus-mobile`.
          mobile = pkgs.mkShell {
            packages = [
              pkgs-unstable.git
              pkgs.sqlite
              pkgs.pkg-config
              pkgs.openssl
              rustMobile
              pkgs.zellij
              pkgs-unstable.process-compose
              pkgs.just
            ] ++ mobileExtras;
            DATABASE_URL = "sqlite://omnibus.db?mode=rwc";
            shellHook = ''
              echo "Nix dev shell ready (mobile)."
            '' + commonShellHook + mobileHook;
          };

          # Security tooling: cargo-audit + cargo-deny. Used by
          # `rust.yml/security` and the occasional local audit.
          audit = mkShell {
            name = "audit";
            packages = slimPackages ++ auditExtras;
          };
        };
      });
}
