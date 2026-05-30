{
  description = "flights — a Rust project";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        # Single source of truth for the toolchain: ./rust-toolchain.toml.
        # The dev shell and the package build use the exact same rustc/cargo.
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };

        # The root manifest is now a virtual workspace; read shared metadata from
        # [workspace.package] rather than a [package] that no longer exists.
        workspace = (pkgs.lib.importTOML ./Cargo.toml).workspace;

        # One derivation builds every workspace member in release mode, so the
        # output carries both binaries: `flights` (the TUI client) and
        # `flights-server` (the engine + REST daemon).
        flights = rustPlatform.buildRustPackage {
          pname = "flights";
          version = workspace.package.version;
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;

          meta = {
            description = "Nearest-flight radar: a thick server and a thin TUI client over a local REST API";
            mainProgram = "flights";
          };
        };
      in
      {
        # `nix build` / `nix build .#flights` -> ./result/bin/{flights,flights-server}
        packages = {
          default = flights;
          flights = flights;
        };

        # `nix run`
        apps.default = flake-utils.lib.mkApp { drv = flights; };

        # `nix develop` (and direnv) -> dev environment with the full toolchain
        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
          ]
          ++ (with pkgs; [
            cargo-watch
            cargo-edit
          ]);

          # Let rust-analyzer find the standard library sources.
          env.RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
        };

        # `nix fmt`
        formatter = pkgs.nixfmt;
      }
    );
}
