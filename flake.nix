{
  description = "Sciscraper - a scientific Markdown editor with RAG assistance";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachSystem [
      "x86_64-linux"
      "aarch64-linux"
      "x86_64-darwin"
      "aarch64-darwin"
    ]
      (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          lib = pkgs.lib;
          cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

          linuxBuildInputs = with pkgs; [
            gtk3
            libxkbcommon
            wayland
            vulkan-loader
            libGL
            libx11
            libxcursor
            libxi
            libxrandr
            libxcb
          ];

          darwinFrameworks = with pkgs.darwin.apple_sdk.frameworks; [
            AppKit
            Cocoa
            CoreFoundation
            CoreGraphics
            CoreServices
            Foundation
            Metal
            QuartzCore
          ];

          sciscraper = pkgs.rustPlatform.buildRustPackage {
            pname = "sciscraper";
            version = cargoToml.package.version;

            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [ pkgs.pkg-config ]
              ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.wrapGAppsHook3 ];

            buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux linuxBuildInputs
              ++ lib.optionals pkgs.stdenv.hostPlatform.isDarwin darwinFrameworks;

            doCheck = false;

            meta = with lib; {
              description = "A scientific Markdown editor with integrated reference management and RAG assistance";
              homepage = "https://github.com/OlaProeis/Ferrite";
              license = licenses.mit;
              mainProgram = "sciscraper";
              platforms = platforms.linux ++ platforms.darwin;
            };
          };
        in
        {
          packages = {
            default = sciscraper;
            sciscraper = sciscraper;
          };

          apps.default = {
            type = "app";
            program = "${sciscraper}/bin/sciscraper";
          };

          devShells.default = pkgs.mkShell {
            packages = [
              pkgs.rustc
              pkgs.cargo
              pkgs.rustfmt
              pkgs.clippy
              pkgs.rust-analyzer
              pkgs.pkg-config
            ]
            ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux linuxBuildInputs
            ++ lib.optionals pkgs.stdenv.hostPlatform.isDarwin darwinFrameworks;

            shellHook = ''
              echo "Sciscraper Nix dev shell ready."
              echo "Run cargo commands normally, e.g. cargo build --release"
            '';
          };

          checks.default = sciscraper;
        }
      );
}
