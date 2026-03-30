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
    tree-sitter
    emscripten
  ];

  env = {
    # Nix Sovereign Architect Rule 1: Zero Impurity
    CXXFLAGS = "-include cstdint -mavx2 -msse4.2 -mpclmul";
    LIBCLANG_PATH = "${pkgs.llvmPackages_18.libclang.lib}/lib";
    
    ELIXIR_HOME = "/home/dstadel/projects/axon/.axon/elixir_home";
    MIX_HOME = "/home/dstadel/projects/axon/.axon/elixir_home/mix";
    HEX_HOME = "/home/dstadel/projects/axon/.axon/elixir_home/hex";
    
    # Port Isolation for Axon (Series 6000)
    PORT = 6000;
    PHX_PORT = 44127;
    HYDRA_HTTP_PORT = 44129;
    HYDRA_TCP_PORT = 44128;
    HYDRA_ODATA_PORT = 44130;
    HYDRA_HTTP2_PORT = 44131;
    HYDRA_MCP_PORT = 44132;
    WATCHER_PORT = 6001;

    # devenv-nix-best-practices: Isolation Patterns
    RELEASE_COOKIE = "axon_v1_isolated_cookie";
    CARGO_TARGET_DIR = "/home/dstadel/projects/axon/.axon/cargo-target";
    DATA_DIR = "/home/dstadel/projects/axon/.axon/data";
    ERL_AFLAGS = "-kernel shell_history enabled";
    
    PYTHONPATH = "/home/dstadel/projects/axon/src";
    FILESYSTEM_FSINOTIFY_EXECUTABLE_FILE = "${pkgs.inotify-tools}/bin/inotifywait";
  };

  # Managed Processes
  processes = {
    core.exec = "/home/dstadel/projects/axon/bin/axon-core";
    
    nexus.exec = ''
      cd src/dashboard && mix ecto.setup && AXON_REPO_SLUG=axon AXON_WATCH_DIR="/home/dstadel/projects/axon" mix phx.server
    '';
  };

  # Project scripts
  scripts = {
    axon-db-start.exec = ''
      echo "HydraDB is intentionally detached from the current Axon Devenv workflow."
      echo "Re-enable it explicitly when the integration is ready."
      exit 1
    '';
  };

  enterShell = ''
    # Fix for native dependencies lookup
    export LD_LIBRARY_PATH="${pkgs.stdenv.cc.cc.lib}/lib:$LD_LIBRARY_PATH"
    export PATH="$MIX_HOME/bin:$HEX_HOME/bin:$PATH"

    # Elixir Isolation (Project Level for Service)
    mkdir -p $ELIXIR_HOME

    # Auto Pre-warming (Necessary for Systemd or first setup)
    if [ ! -f "$MIX_HOME/archives/hex-"* ]; then
      echo "📦 Pre-warming Elixir environment (Hex/Rebar)..."
      mix local.hex --force > /dev/null 2>&1
      mix local.rebar --force > /dev/null 2>&1
    fi

    echo "--- AXON v1.0 - DEVENV ARCHITECTURE ---"
    echo "Pod A (Control Plane): Elixir $(elixir --version | awk '/Elixir/ {print $2}')"
    echo "Pod B (Data Plane):    Rust $(rustc --version | awk '{print $2}')"
    echo "Support Tooling:       Python $(python --version | awk '/Python/ {print $2}')"
    echo "Pod C (HydraDB):       detached from current Devenv workflow"
    echo "---------------------------------------"
  '';
}
