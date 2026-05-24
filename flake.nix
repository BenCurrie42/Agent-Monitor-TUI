{
  description = "AgentMonitorTUI — passive lazydocker-style TUI for monitoring Claude Code sessions";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        darwinDeps = pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "agent-monitor-tui";
          version = "0.0.6";

          src = self;

          cargoLock.lockFile = ./Cargo.lock;

          buildInputs = darwinDeps;

          meta = with pkgs.lib; {
            description = "Passive lazydocker-style TUI for monitoring Claude Code sessions";
            homepage = "https://github.com/BenCurrie42/Agent-Monitor-TUI";
            license = licenses.mit;
            mainProgram = "agentmonitor";
            platforms = platforms.unix;
          };
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustc
            cargo
            rust-analyzer
            clippy
            rustfmt
          ] ++ darwinDeps;

          # Prevents build.rs xcrun from being needed in the dev shell
          shellHook = ''
            echo "agent-monitor dev shell — run: cargo build --release"
          '';
        };
      });
}
