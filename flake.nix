{
  description = "kb — a fast Markdown knowledge base";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    { self, nixpkgs, ... }:
    let
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
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
          version = cargoToml.workspace.package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;

          # The git module's tests build throwaway repositories, so the sandbox
          # needs git. Runtime lookups (git, fzf, glow) stay on PATH so the
          # binary uses whatever the user already has.
          nativeCheckInputs = [ pkgs.git ];

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
