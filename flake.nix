{
  description = "kb — a fast Markdown knowledge base";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    { self, nixpkgs, ... }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      formatter = forAllSystems (pkgs: pkgs.nixfmt);

      packages = forAllSystems (pkgs: rec {
        kb = pkgs.rustPlatform.buildRustPackage {
          pname = "kb";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;

          # `kb open` shells out to these; they stay runtime lookups on PATH
          # rather than build inputs so the binary works with whatever the user
          # already has installed.
          meta = {
            description = "A fast Markdown knowledge base";
            mainProgram = "kb";
          };
        };
        default = kb;
      });

      # Development lives in the flake, not in dotfiles' home.packages
      # (`use flake` via nix-direnv activates it inside this directory).
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.rustc
            pkgs.cargo
            pkgs.rust-analyzer
            pkgs.clippy
            pkgs.rustfmt
          ];
        };
      });
    };
}
