{
  description = "Axon v1.0 - The Intelligent Immune System (Triple-Pod Architecture)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        
        # Environnement Python pour le POD B (Parser Slave)
        pythonEnv = pkgs.python312.withPackages (ps: with ps; [
          tree-sitter
          msgpack
          setuptools
          pyarrow
          pandas
          pydantic
        ]);

        # Infrastructure complète pour le POD A (Watcher) et le POD C (HydraDB)
        elixirTools = with pkgs; [
          elixir
          erlang_27
          inotify-tools
          # Dépendances natives impératives pour HydraDB
          cmake
          pkg-config
          openssl
          zlib
          # Forçage d'une toolchain stable pour éviter GCC 15 / RocksDB errors
          gcc13
          llvmPackages_18.clang
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
            
            # Forçage du compilateur pour les NIFs Rust/C++
            export CC=clang
            export CXX=clang++
            export CXXFLAGS="-include cstdint -mavx2 -msse4.2 -mpclmul"
            
            # Garantir que Mix trouve les bons outils Nix
            export MIX_ENV=dev
            export LIBCLANG_PATH="${pkgs.llvmPackages_18.libclang.lib}/lib"
            export LD_LIBRARY_PATH="${pkgs.stdenv.cc.cc.lib}/lib:$LD_LIBRARY_PATH"
            
            echo "--- AXON v1.0 - UNIFIED TRIPLE-POD ENVIRONMENT ---"
            echo "Pod A (Watcher): Elixir $(elixir --version | grep 'Elixir' | awk '{print $2}')"
            echo "Pod B (Parser):  Python $(python --version | awk '{print $2}')"
            echo "Pod C (HydraDB): Stable Toolchain (Clang/GCC13, Erlang 27)"
            echo "---------------------------------------------------"
          '';
        };
      }
    );
}
