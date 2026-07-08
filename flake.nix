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

        # One derivation builds every `default-members` crate in release mode, so the
        # output carries all three host binaries: `flights` (the TUI client),
        # `flights-server` (the engine + REST daemon), and `flights-waybar` (the
        # status-bar client — a `default-members` crate per ADR-0008, unlike the wasm
        # `flights-web`, which Trunk builds separately).
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

        # `nix run .#web` — launch the Server and the radar webclient together.
        # Unlike the TUI, the webclient can't ship as a prebuilt binary: Trunk
        # compiles it to wasm from source (ADR-0007). So this bundles the wasm
        # toolchain — `rustToolchain` (which carries the wasm32 std), `trunk`, and a
        # version-matched `wasm-bindgen-cli` — beside the packaged `flights-server`,
        # and builds ./flights-web from the current checkout. The launcher logic
        # lives in ./scripts/flights-web (one copy, shared with the dev shell).
        web = pkgs.writeShellApplication {
          name = "flights-web";
          runtimeInputs = [
            flights # provides flights-server
            rustToolchain # cargo + rustc + the wasm32 std, for Trunk's build
            pkgs.trunk
            pkgs.wasm-bindgen-cli # must match the pinned wasm-bindgen crate
            pkgs.curl # the /meta readiness + CORS probe
            pkgs.coreutils # sleep / tail / tr
          ];
          text = builtins.readFile ./scripts/flights-web;
        };

        # `nix build .#web-dist` — the webclient as a *built static bundle*, not a
        # launcher. Trunk compiles ./flights-web to wasm at build time and emits the
        # deployable directory (index.html + hashed wasm/js + style.css + the B612
        # fonts) into `./result`. Drop that behind any static file server — it is a
        # pure Client (ADR-0005/0007): it renders and reads the Server's REST API
        # cross-origin, so wherever it is served, that origin must appear in the
        # Server's `cors_allow_origin` (the `serve-web` HM option below wires this up
        # automatically). Point the page at a Server with `?server=http://host:port`.
        #
        # This reuses `flights`'s `cargoLock` vendoring so the wasm crates build with
        # no network, then hands the build to Trunk. Trunk's own tool fetch is the
        # one thing that would reach out — `--offline` forbids it and Trunk instead
        # takes `wasm-bindgen` (version-matched to the `=0.2.121` crate pin) and
        # `wasm-opt` (binaryen) straight off the build PATH.
        web-dist = rustPlatform.buildRustPackage {
          pname = "flights-web-dist";
          version = workspace.package.version;
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [
            pkgs.trunk
            pkgs.wasm-bindgen-cli # must match the pinned wasm-bindgen crate
            # wasm-opt, for Trunk's release minification. rustc emits the finished
            # wasm proposals (bulk-memory, sign-ext, …) by default, but binaryen 129
            # won't auto-enable them when optimizing and rejects its own input. A
            # thin wrapper turns them on so `-O` validates; `--version` (Trunk's
            # discovery probe) still reports plain "129", so Trunk accepts it offline.
            (pkgs.writeShellScriptBin "wasm-opt" ''
              exec ${pkgs.binaryen}/bin/wasm-opt \
                --enable-bulk-memory \
                --enable-sign-ext \
                --enable-nontrapping-float-to-int \
                --enable-mutable-globals \
                --enable-reference-types \
                --enable-multivalue \
                "$@"
            '')
          ];

          # Trunk drives the wasm build and the asset pipeline, so replace
          # buildRustPackage's default cargo build/check/install with one Trunk run.
          # The cargoLock setup hook still runs in configurePhase, vendoring deps and
          # pointing cargo (which Trunk shells out to) at the offline registry.
          buildPhase = ''
            runHook preBuild
            # Run from the crate dir (mirroring ./scripts/flights-web): the root
            # Cargo.toml is a *virtual* workspace, so Trunk must see flights-web's own
            # package manifest to resolve the target crate.
            cd flights-web
            trunk build --release --offline --dist "$out" index.html
            runHook postBuild
          '';
          dontCargoBuild = true;
          doCheck = false;
          dontCargoCheck = true;
          installPhase = "true"; # Trunk already wrote the bundle to $out

          meta = {
            description = "flights radar webclient, compiled to a static wasm bundle (ADR-0007)";
          };
        };
      in
      {
        # `nix build` / `nix build .#flights` -> ./result/bin/{flights,flights-server,flights-waybar}
        # `nix build .#radar`                 -> ./result/bin/flights-radar
        # `nix build .#web-dist`              -> ./result (static webclient bundle)
        packages = {
          default = flights;
          flights = flights;
          radar = radar;
          web = web;
          web-dist = web-dist;
        };

        # `nix run` (the TUI alone), `nix run .#radar` (Server + TUI together),
        # `nix run .#web` (Server + webclient together)
        apps = {
          default = flake-utils.lib.mkApp { drv = flights; };
          radar = flake-utils.lib.mkApp {
            drv = radar;
            name = "flights-radar";
          };
          web = flake-utils.lib.mkApp {
            drv = web;
            name = "flights-web";
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

            # The webclient toolchain (ADR-0007). `rustToolchain` already carries
            # the wasm32 std (rust-toolchain.toml's `targets`); these two bundle it
            # into a deployable directory. wasm-bindgen-cli must match the
            # `wasm-bindgen` crate version exactly, so flights-web pins its
            # `wasm-bindgen` dependency to this package's version (currently
            # ${wasm-bindgen-cli.version}).
            trunk
            wasm-bindgen-cli
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
    )
    // {
      # The Server's first **always-on** deployment (ADR-0008). `flights-waybar` is
      # a one-shot module firing at ~1 Hz: it must never start a Server itself (that
      # would swarm pollers against a rate-limited Source and blow the single-poller
      # budget — ADR-0005), so it reads a Server someone else keeps running. This
      # module is that someone: a **systemd user service**, since the Server lives in
      # the user's graphical session beside Waybar and needs no root. `programs.waybar`
      # stays the user's, wired from the documented `custom/flights` snippet in the
      # README rather than auto-managed here.
      #
      # Not wrapped in `eachDefaultSystem`: a Home Manager module is system-agnostic
      # and picks the package for the importing config's own `pkgs.system`.
      homeManagerModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.flights-server;
        in
        {
          options.services.flights-server = {
            enable = lib.mkEnableOption (
              "the flights nearest-flight Server as a systemd user service "
              + "(and the flights/flights-waybar Clients on PATH)"
            );

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.flights;
              defaultText = lib.literalExpression "flights.packages.\${system}.flights";
              description = ''
                The flights package, carrying the engine (`flights-server`) and the
                Clients (`flights`, `flights-waybar`) on PATH — one derivation, no
                wasm (ADR-0008).
              '';
            };

            extraArgs = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [ ];
              example = [
                "--config"
                "%h/.config/flights/config.toml"
              ];
              description = ''
                Extra arguments appended to `flights-server --serve`. By default the
                Server reads `$XDG_CONFIG_HOME/flights/config.toml`; set `[home]`
                there to your location.
              '';
            };

            # Serve the compiled webclient beside the always-on Server. The bundle is
            # a pure static Client (ADR-0005/0007) — `static-web-server` just hands
            # out the files; all state lives in the Server it reads cross-origin. So
            # enabling this also opts the Server into the page's origin over CORS
            # (see `config` below), mirroring what `nix run .#web` does per-run.
            web = {
              enable = lib.mkEnableOption (
                "serving the compiled radar webclient (the .#web-dist bundle) over a "
                + "static-web-server user service, with the Server's CORS wired to match"
              );

              package = lib.mkOption {
                type = lib.types.package;
                default = self.packages.${pkgs.system}.web-dist;
                defaultText = lib.literalExpression "flights.packages.\${system}.web-dist";
                description = ''
                  The built static webclient bundle to serve — Trunk's `dist/` output
                  (wasm + js + css + fonts), not a binary.
                '';
              };

              address = lib.mkOption {
                type = lib.types.str;
                default = "127.0.0.1";
                description = ''
                  Address `static-web-server` binds. Loopback by default; set to
                  `0.0.0.0` to reach the page from the LAN (then set `corsOrigin` to
                  the host the browser actually uses — `address` is a bind address,
                  not a browser origin).
                '';
              };

              port = lib.mkOption {
                type = lib.types.port;
                default = 8080;
                description = "Port `static-web-server` serves the webclient on.";
              };

              corsOrigin = lib.mkOption {
                type = lib.types.str;
                default = "http://127.0.0.1:${toString cfg.web.port}";
                defaultText = lib.literalExpression ''"http://127.0.0.1:''${toString port}"'';
                description = ''
                  The browser origin the Server must allow so the page's cross-origin
                  reads aren't blocked. Appended to the Server as
                  `--cors-allow-origin`. Defaults to the loopback origin `port`
                  produces; override when the page is reached over the LAN or a
                  reverse proxy.
                '';
              };
            };
          };

          config = lib.mkIf cfg.enable {
            # Puts `flights-waybar` on PATH for Waybar's `exec`, plus `flights` (TUI)
            # and `flights-server` for manual use.
            home.packages = [ cfg.package ];

            # Bound to graphical-session.target so it starts/stops with the session
            # in lockstep with Waybar. `Restart=on-failure` keeps the always-on
            # Server up across a transient Source hiccup; the one-poller invariant
            # holds as long as no on-demand launcher (flights-radar / flights-web) is
            # pointed at the same Source concurrently (ADR-0005/0008).
            systemd.user.services.flights-server = {
              Unit = {
                Description = "flights nearest-flight Server (engine + local REST API)";
                PartOf = [ "graphical-session.target" ];
                After = [ "graphical-session.target" ];
              };
              Service = {
                ExecStart = lib.escapeShellArgs (
                  [
                    "${cfg.package}/bin/flights-server"
                    "--serve"
                  ]
                  # Opt the Server into the served page's origin (CORS is off by
                  # default — ADR-0005/0007). `extraArgs` follows, so a user's own
                  # --cors-allow-origin there still stacks.
                  ++ lib.optionals cfg.web.enable [
                    "--cors-allow-origin"
                    cfg.web.corsOrigin
                  ]
                  ++ cfg.extraArgs
                );
                Restart = "on-failure";
                RestartSec = 3;
              };
              Install.WantedBy = [ "graphical-session.target" ];
            };

            # The static webclient, served beside the Server when `web.enable`. Like
            # the Server it lives in the graphical session and needs no root; it only
            # hands out files, so it never touches the Source and can't threaten the
            # single-poller invariant. Ordered `After` the Server purely for tidy
            # startup — the page tolerates a not-yet-ready Server (it shows
            # UNREACHABLE and retries), so this is not a hard dependency.
            systemd.user.services.flights-web = lib.mkIf cfg.web.enable {
              Unit = {
                Description = "flights radar webclient (static bundle over static-web-server)";
                PartOf = [ "graphical-session.target" ];
                After = [
                  "graphical-session.target"
                  "flights-server.service"
                ];
              };
              Service = {
                ExecStart = lib.escapeShellArgs [
                  "${pkgs.static-web-server}/bin/static-web-server"
                  "--root"
                  "${cfg.web.package}"
                  "--host"
                  cfg.web.address
                  "--port"
                  (toString cfg.web.port)
                ];
                Restart = "on-failure";
                RestartSec = 3;
              };
              Install.WantedBy = [ "graphical-session.target" ];
            };
          };
        };
    };
}
