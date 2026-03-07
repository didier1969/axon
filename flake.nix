{
  description = "Axon v1.0 - The Intelligent Immune System (Triple-Pod Architecture)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # Futur point d'ancrage pour HydraDB
    # hydradb.url = "github:didier1969/hydradb";
  };

  outputs = { self, nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        
        # Environnement Python pour le POD B (Parser Slave)
        pythonEnv = pkgs.python311.withPackages (ps: with ps; [
          tree-sitter
          msgpack
          setuptools
          # On garde pyarrow pour la future conformité HydraDB
          pyarrow
        ]);

        # Outils nécessaires pour le POD A (Elixir Watcher)
        # inotify-tools est requis pour le watcher natif sur Linux
        watcherTools = with pkgs; [
          elixir
          erlang
          inotify-tools
        ];

        # Outils pour les NIFs et la performance native
        nativeTools = with pkgs; [
          rustc
          cargo
          pkg-config
        ];

      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = [
            pythonEnv
            pkgs.uv
          ] ++ watcherTools ++ nativeTools;

          shellHook = ''
            # Configuration du pont Python/Elixir
            export PYTHONPATH=$PYTHONPATH:$(pwd)/src
            
            # Garantir que Mix trouve les bons outils Nix
            export MIX_ENV=dev
            
            echo "--- AXON v1.0 - Triple-Pod Environment ---"
            echo "Pod A (Watcher): Elixir $(elixir --version | grep 'Elixir' | awk '{print $2}')"
            echo "Pod B (Parser):  Python $(python --version | awk '{print $2}')"
            echo "Pod C (HydraDB): Infrastructure Ready (via Nix inputs)"
            echo "-------------------------------------------"
          '';
        };
      }
    );
}
