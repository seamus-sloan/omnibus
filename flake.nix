{
  description = "Omnibus full-stack Rust app dev environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            git
            cargo
            rustc
            rustfmt
            clippy
            sqlite
            pkg-config
            openssl
            nodejs_22
          ];

          DATABASE_URL = "sqlite://omnibus.db?mode=rwc";

          shellHook = ''
            echo "Nix dev shell ready."
            echo "Run: cargo test"
            echo "Run: cargo run"
          '';
        };
      });
}
