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
      # `nix build` / flake input: the same pinned toolchain as the dev shell.
      packages = forEachSystem (
        pkgs:
        let
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };
        in
        {
          default = rustPlatform.buildRustPackage {
            pname = "mcp-brain";
            version = (pkgs.lib.importTOML ./Cargo.toml).package.version;
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            # CI runs the test suite in the dev shell.
            doCheck = false;
            meta.mainProgram = "mcp-brain";
          };
        }
      );

      devShells = forEachSystem (pkgs: {
        default = pkgs.mkShell {
          packages = [
            # The toolchain pinned in rust-toolchain.toml, so Nix and rustup agree.
            (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
            # CI lints install.sh with it.
            pkgs.shellcheck
          ];
          RUST_BACKTRACE = "1";
        };
      });
    };
}
