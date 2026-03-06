{
  description = "Axon Intelligence Unit - Parsing Toolchain for HydraDB Integration";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        pythonEnv = pkgs.python311.withPackages (ps: with ps; [
          pyarrow
          pandas
          tree-sitter
          setuptools
        ]);
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            pythonEnv
            uv
            elixir
            erlang
            rustc
            cargo
          ];

          shellHook = ''
            export PYTHONPATH=$PYTHONPATH:$(pwd)/src
            echo "Axon x HydraDB - Compliance Environment Ready"
          '';
        };
      }
    );
}
