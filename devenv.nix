{ pkgs, inputs, lib, config, ... }:

let
  pythonEnv = pkgs.python312.withPackages (ps: with ps; [
    tree-sitter
    tree-sitter-python
    msgpack
    setuptools
    pyarrow
    pandas
    pydantic
  ]);
  
  beamPackages = pkgs.beam.packages.erlang_27;
in
{
  # Nix Sovereign Architect: Multi-language support with proper modularity
  languages.rust.enable = true;
  languages.rust.channel = "stable";

  languages.python.enable = true;
  languages.python.package = pythonEnv;

  languages.elixir.enable = true;
  languages.elixir.package = beamPackages.elixir_1_18;

  packages = with pkgs; [
    # General Native & Build Tools
    inotify-tools
    watchman
    cmake
    pkg-config
    openssl
    zlib
    gcc13
    llvmPackages_18.libclang.lib
    stdenv.cc.cc.lib
    uv
    beamPackages.rebar3
    psmisc
  ];

  # Managed Processes (Triple-Pod Architecture)
  processes = {
    db.exec = "axon-db-start";
    core.exec = "/home/dstadel/projects/axon/bin/axon-core";
    
    watcher.exec = ''
      export PYTHONPATH="$PYTHONPATH:$PWD/src"
      export ELIXIR_HOME="$PWD/.axon/elixir_home"
      export MIX_HOME="$ELIXIR_HOME/mix"
      export HEX_HOME="$ELIXIR_HOME/hex"
      export PATH="$MIX_HOME/bin:$HEX_HOME/bin:$PATH"
      cd src/watcher && mix ecto.setup && AXON_REPO_SLUG=axon AXON_WATCH_DIR="/home/dstadel/projects/axon" elixir --name watcher@127.0.0.1 --cookie axon_v2_cluster -S mix run --no-halt
    '';

    dashboard.exec = ''
      export ELIXIR_HOME="$PWD/.axon/elixir_home"
      export MIX_HOME="$ELIXIR_HOME/mix"
      export HEX_HOME="$ELIXIR_HOME/hex"
      export PATH="$MIX_HOME/bin:$HEX_HOME/bin:$PATH"
      cd src/dashboard && PHX_PORT=44921 elixir --name dashboard@127.0.0.1 --cookie axon_v2_cluster -S mix phx.server
    '';
  };

  env = {
    # Nix Sovereign Architect Rule 1: Zero Impurity
    CXXFLAGS = "-include cstdint -mavx2 -msse4.2 -mpclmul";
    LIBCLANG_PATH = "${pkgs.llvmPackages_18.libclang.lib}/lib";
    
    HYDRADB_SOURCE = inputs.hydradb-src.outPath;
    
    # Port Isolation for Axon (Series 6000)
    PORT = 6000;
    PHX_PORT = 44921; # Force Dashboard Port
    HYDRA_HTTP_PORT = 6000;
    HYDRA_TCP_PORT = 6040;
    WATCHER_PORT = 6001;

    # devenv-nix-best-practices: Isolation Patterns
    RELEASE_COOKIE = "axon_v1_isolated_cookie";
    CARGO_TARGET_DIR = "/home/dstadel/projects/axon/.axon/cargo-target";
    DATA_DIR = "/home/dstadel/projects/axon/.axon/data";
    ERL_AFLAGS = "-kernel shell_history enabled";
  };

  # Scripts to start the different Pods
  scripts = {
    axon-db-setup.exec = ''
      echo "🛠️ Setting up HydraDB v1.0.0 Stable..."
      mkdir -p $HYDRADB_RUNTIME
      cp -r $HYDRADB_SOURCE/* $HYDRADB_RUNTIME/
      chmod -R +w $HYDRADB_RUNTIME
      cd $HYDRADB_RUNTIME && mix deps.get && mix compile
      echo "✅ HydraDB v1.0.0 Ready in $HYDRADB_RUNTIME"
    '';

    axon-db-start.exec = ''
      if [ ! -d "$HYDRADB_RUNTIME/deps" ]; then axon-db-setup; fi
      echo "🚀 Starting Isolated HydraDB (Pod C) on port 6040..."
      cd $HYDRADB_RUNTIME && export HYDRA_DB_API_KEY=dev_key && export TCP_PORT=6040 && elixir --name hydra_axon@127.0.0.1 -S mix run --no-halt
    '';
  };

  enterShell = ''
    # Dynamic variables requiring $PWD
    export PYTHONPATH="$PYTHONPATH:$PWD/src"
    export HYDRADB_RUNTIME="$PWD/.axon/runtime/hydradb"
    export ELIXIR_HOME="$PWD/.axon/elixir_home"
    export MIX_HOME="$ELIXIR_HOME/mix"
    export HEX_HOME="$ELIXIR_HOME/hex"
    export PATH="$MIX_HOME/bin:$HEX_HOME/bin:$PATH"
    
    # Fix for native dependencies lookup
    export LD_LIBRARY_PATH="${pkgs.stdenv.cc.cc.lib}/lib:$LD_LIBRARY_PATH"

    # Elixir Isolation (Project Level for Service)
    mkdir -p $ELIXIR_HOME

    # Auto Pre-warming (Necessary for Systemd or first setup)
    if [ ! -f "$MIX_HOME/archives/hex-"* ]; then
      echo "📦 Pre-warming Elixir environment (Hex/Rebar)..."
      mix local.hex --force > /dev/null 2>&1
      mix local.rebar --force > /dev/null 2>&1
    fi

    echo "--- AXON v1.0 - DEVENV ARCHITECTURE ---"
    echo "Pod A (Watcher): Elixir $(elixir --version | awk '/Elixir/ {print $2}')"
    echo "Pod B (Parser):  Python $(python --version | awk '/Python/ {print $2}')"
    echo "Pod C (HydraDB): v1.0.0 Stable (Run 'axon-db-start' to launch)"
    echo "---------------------------------------"
  '';
}