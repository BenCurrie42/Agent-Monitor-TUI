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

        # RELEASE SYNC: bump version + hashes after each `cargo release` / GitHub release.
        # Hashes are sha256 SRI (sha256-<base64>) from the GitHub release asset digests.
        # Run: echo "<hex>" | xxd -r -p | base64  to convert GitHub's hex digest.
        version = "0.1.0";

        binaries = {
          "aarch64-darwin" = {
            url = "https://github.com/BenCurrie42/Agent-Monitor-TUI/releases/download/v${version}/agentmonitor-aarch64-apple-darwin";
            hash = "sha256-Irj+l90lereAk9/9GC6xPsJ5IRuRhi7+GhgqcKonzLI=";
          };
          "x86_64-darwin" = {
            url = "https://github.com/BenCurrie42/Agent-Monitor-TUI/releases/download/v${version}/agentmonitor-x86_64-apple-darwin";
            hash = "sha256-taTqNhhWyDgFHqwXS90SxA2tcEkGNkOL01gzA3T8RFY=";
          };
          "x86_64-linux" = {
            url = "https://github.com/BenCurrie42/Agent-Monitor-TUI/releases/download/v${version}/agentmonitor-x86_64-unknown-linux-musl";
            hash = "sha256-+k4e1RWsgFtAF8rbWK4CoL2sbsWlZkVbGclWOHSO5cM=";
          };
        };

        binary = binaries.${system} or (throw "Unsupported system: ${system}. Add a release asset entry in flake.nix.");
      in
      {
        packages.default = pkgs.stdenv.mkDerivation {
          pname = "agent-monitor-tui";
          inherit version;

          src = pkgs.fetchurl {
            inherit (binary) url hash;
          };

          dontUnpack = true;

          installPhase = ''
            mkdir -p $out/bin
            cp $src $out/bin/agentmonitor
            chmod +x $out/bin/agentmonitor
          '';

          meta = with pkgs.lib; {
            description = "Passive lazydocker-style TUI for monitoring Claude Code sessions";
            homepage = "https://github.com/BenCurrie42/Agent-Monitor-TUI";
            license = licenses.mit;
            mainProgram = "agentmonitor";
            platforms = [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" ];
          };
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustc
            cargo
            rust-analyzer
            clippy
            rustfmt
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];

          shellHook = ''
            echo "agent-monitor dev shell — run: cargo build --release"
          '';
        };
      });
}
