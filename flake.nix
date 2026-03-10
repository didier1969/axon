{
  description = "Axon v1.0 - The Intelligent Immune System (Triple-Pod Architecture)";
inputs = {
  nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  flake-utils.url = "github:numtide/flake-utils";
  # HydraDB Stable Source
  hydradb-src = {
    url = "git+https://github.com/didier1969/hydraDB.git";
    flake = false;
  };

};

outputs = { self, nixpkgs, flake-utils, hydradb-src, ... }:
  flake-utils.lib.eachDefaultSystem (system:
    let
      pkgs = nixpkgs.legacyPackages.${system};

      # Environnement Python pour le POD B (Parser Slave)
      pythonEnv = pkgs.python312.withPackages (ps: with ps; [
        tree-sitter
        tree-sitter-python
        # Core tools
        msgpack
        setuptools
        pyarrow
        pandas
        pydantic
      ]);

      # Infrastructure pour POD A et POD C
      elixirTools = with pkgs; [
        elixir_1_18
        erlang_27
        inotify-tools
        cmake
        pkg-config
        openssl
        zlib
        gcc13
        llvmPackages_18.libclang.lib
        stdenv.cc.cc.lib
      ];


        nativeTools = with pkgs; [
          rustc
          cargo
          rustfmt
          clippy
        ];

      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = [
            pythonEnv
            pkgs.uv
          ] ++ elixirTools ++ nativeTools;

          shellHook = ''
            # Configuration du pont Python/Elixir
            export PYTHONPATH=$PYTHONPATH:$(pwd)/src
            export HYDRADB_SOURCE="${hydradb-src}"
            export HYDRADB_RUNTIME="$(pwd)/.axon/runtime/hydradb"
            
            # Isolation Elixir (Niveau Projet pour le Service)
            export ELIXIR_HOME="$(pwd)/.axon/elixir_home"
            mkdir -p $ELIXIR_HOME
            export MIX_HOME="$ELIXIR_HOME/mix"
            export HEX_HOME="$ELIXIR_HOME/hex"
            export PATH="$MIX_HOME/bin:$HEX_HOME/bin:$PATH"

            # Préchauffage automatique (Indispensable pour Systemd)
            if [ ! -f "$MIX_HOME/archives/hex-"* ]; then
              echo "📦 Pre-warming Elixir environment (Hex/Rebar)..."
              mix local.hex --force > /dev/null 2>&1
              mix local.rebar --force > /dev/null 2>&1
            fi
            
            # Isolation des Ports pour Axon (Série 6000)
            export PORT=6000
            export HYDRA_HTTP_PORT=6000
            export HYDRA_TCP_PORT=6040
            export WATCHER_PORT=6001
            
            # Configuration Compilation
            export LIBCLANG_PATH="${pkgs.llvmPackages_18.libclang.lib}/lib"
            export LD_LIBRARY_PATH="${pkgs.stdenv.cc.cc.lib}/lib:$LD_LIBRARY_PATH"
            export CXXFLAGS="-include cstdint -mavx2 -msse4.2 -mpclmul"

            # Script de setup automatique pour HydraDB Stable
            axon-db-setup() {
              echo "🛠️ Setting up HydraDB v1.0.0 Stable..."
              mkdir -p $HYDRADB_RUNTIME
              cp -r $HYDRADB_SOURCE/* $HYDRADB_RUNTIME/
              chmod -R +w $HYDRADB_RUNTIME
              cd $HYDRADB_RUNTIME && mix deps.get && mix compile
              echo "✅ HydraDB v1.0.0 Ready in $HYDRADB_RUNTIME"
            }

            axon-db-start() {
              if [ ! -d "$HYDRADB_RUNTIME/deps" ]; then axon-db-setup; fi
              echo "🚀 Starting Isolated HydraDB (Pod C) on port 6040..."
              cd $HYDRADB_RUNTIME && export HYDRA_DB_API_KEY=dev_key && export TCP_PORT=6040 && elixir --name hydra_axon@127.0.0.1 -S mix run --no-halt
            }
            
            echo "--- AXON v1.0 - UNIFIED STABLE ENVIRONMENT ---"
            echo "Pod A (Watcher): Elixir $(elixir --version | grep 'Elixir' | awk '{print $2}')"
            echo "Pod B (Parser):  Python $(python --version | awk '{print $2}')"
            echo "Pod C (HydraDB): v1.0.0 Stable (Run 'axon-db-start' to launch)"
            echo "-----------------------------------------------"
          '';
        };
      }
    );
}
