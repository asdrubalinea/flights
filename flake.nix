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

        # `nix run .#radar` — launch the Server and the radar TUI together. The TUI
        # is a thin Client (ADR-0005), so this is a launcher, not a merged binary:
        # it starts `flights-server` in the background and runs `flights` in front,
        # reusing an already-running Server rather than spawning a second poller.
        # The launcher logic lives in ./scripts/flights-radar (one copy, shared with
        # the dev shell); here we just put the packaged binaries + curl on its PATH.
        radar = pkgs.writeShellApplication {
          name = "flights-radar";
          runtimeInputs = [
            flights # provides both `flights` and `flights-server`
            pkgs.curl # the /meta readiness probe
            pkgs.coreutils # sleep / tail / dirname
          ];
          text = builtins.readFile ./scripts/flights-radar;
        };
      in
      {
        # `nix build` / `nix build .#flights` -> ./result/bin/{flights,flights-server}
        # `nix build .#radar`                 -> ./result/bin/flights-radar
        packages = {
          default = flights;
          flights = flights;
          radar = radar;
        };

        # `nix run` (the TUI alone), `nix run .#radar` (Server + TUI together)
        apps = {
          default = flake-utils.lib.mkApp { drv = flights; };
          radar = flake-utils.lib.mkApp {
            drv = radar;
            name = "flights-radar";
          };
        };

        # `nix develop` (and direnv) -> dev environment with the full toolchain
        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
          ]
          ++ (with pkgs; [
            cargo-watch
            cargo-edit
            curl # flights-radar's readiness probe (also handy for poking the REST API)
          ]);

          # Let rust-analyzer find the standard library sources.
          env.RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

          # Put the repo's launcher on PATH so `flights-radar` works in the dev shell
          # (it cargo-builds both crates, then runs Server + TUI together). direnv /
          # nix develop enter at the repo root, so $PWD resolves scripts/ correctly.
          shellHook = ''
            export PATH="$PWD/scripts:$PATH"
          '';
        };

        # `nix fmt`
        formatter = pkgs.nixfmt;
      }
    );
}
