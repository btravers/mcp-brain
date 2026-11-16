{
  description = "mcp-brain development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEachSystem =
        f:
        nixpkgs.lib.genAttrs systems (
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
      devShells = forEachSystem (pkgs: {
        default = pkgs.mkShell {
          # The toolchain pinned in rust-toolchain.toml, so Nix and rustup agree.
          packages = [ (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml) ];
        };
      });
    };
}
