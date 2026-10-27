{
  description = "cycling: VANA, C606 firmware and host tools";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      forSystems =
        f:
        nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      packages = forSystems (pkgs: {
        esp-rust = pkgs.callPackage ./nix/esp-rust.nix { };
      });

      devShells = forSystems (pkgs: {
        default = pkgs.callPackage ./nix/shell.nix {
          inherit (self.packages.${pkgs.stdenv.hostPlatform.system}) esp-rust;
        };
      });

      # Evaluates and builds the whole environment; `cycling verify` runs the suites.
      checks = forSystems (pkgs: {
        shell = self.devShells.${pkgs.stdenv.hostPlatform.system}.default;
      });

      formatter = forSystems (pkgs: pkgs.nixfmt);
    };
}
