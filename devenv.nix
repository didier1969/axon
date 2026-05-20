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

  # MIL-AXO-015 P1: PostgreSQL 17 + pgvector + pgmq
  # Replaces DuckDB as the canonical Axon storage layer (DEC-AXO-075).
  # CPT-AXO-039 per-project schema namespace, CPT-AXO-041 pgvector HNSW.
  # Single dev instance for Axon team; live/dev process separation handled
  # at runtime via 2 different DATABASE_URLs (different DBs on this single
  # instance, or 2 separate PG instances in production - client choice per
  # CPT-AXO-042 distribution model).
  # AGE retired MIL-AXO-017 / REQ-AXO-90005 — Apache AGE extension removed
  # (was the legacy Cypher overlay, replaced by public.Edge + WITH RECURSIVE
  # SQL functions in 04_graph_functions.sql, REQ-AXO-296).
  services.postgres = {
    enable = true;
    package = pkgs.postgresql_17;
    # REQ-AXO-901624 — pgmq fournit la queue-PG-native qui découple le
    # calcul `content_tsv` du chemin critique A3 (sub-drum identifié
    # session 48). `exts.pgmq` est le pkg nixpkgs canonique.
    extensions = exts: [ exts.pgvector exts.pgmq ];
    initialDatabases = [
      { name = "axon_dev"; }
      { name = "axon_live"; }
    ];
    listen_addresses = "127.0.0.1";
    port = 44144;
    settings = {
      # Axon's hot path benefits from generous shared_buffers; client tunes
      # for their own scale via standard PG ops procedures (CPT-AXO-038).
      shared_buffers = "512MB";
      # pgvector index build benefits from larger maintenance memory.
      maintenance_work_mem = "256MB";
    };
  };

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
    # Postgres CLI for hand inspection / migration scripts
    postgresql_17

    # REQ-AXO-901630 — DO NOT add `pkgs.onnxruntime` here. The nixpkgs
    # default onnxruntime ships without TensorRT/CUDA provider libs and,
    # when present in `packages`, leaks its `lib/` into devenv shell's
    # composite LD_LIBRARY_PATH. The indexer would then dlopen that
    # rather than the TensorRT-enabled artifact resolved by
    # `scripts/lib/axon-ort-runtime.sh` from
    # `.axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json`
    # and silently fall back to NoOpEmbedder (junk vectors). The artifact
    # is built once via `scripts/build_ort_tensorrt_artifact.sh` and
    # consumed at runtime through `ORT_STRATEGY=system` +
    # `ORT_DYLIB_PATH`. No shell-level ORT dependency required.
  ];

  env = {
    # Nix Sovereign Architect Rule 1: Zero Impurity
    CXXFLAGS = "-include cstdint -mavx2 -msse4.2 -mpclmul";
    LIBCLANG_PATH = "${pkgs.llvmPackages_18.libclang.lib}/lib";
    
    ELIXIR_HOME = config.env.DEVENV_ROOT + "/.axon/elixir_home";
    MIX_HOME = config.env.DEVENV_ROOT + "/.axon/elixir_home/mix";
    HEX_HOME = config.env.DEVENV_ROOT + "/.axon/elixir_home/hex";
    
    # Port Isolation for Axon (Series 6000)
    PORT = 6000;
    PHX_PORT = 44127;
    HYDRA_HTTP_PORT = 44129;
    HYDRA_TCP_PORT = 44128;
    HYDRA_ODATA_PORT = 44130;
    HYDRA_HTTP2_PORT = 44131;
    HYDRA_MCP_PORT = 44132;
    WATCHER_PORT = 6001;

    # MIL-AXO-015 P1: PostgreSQL connection strings for Axon runtime.
    # Live and dev share the dev devenv-managed PG (single instance, two
    # DBs). Production deployments override these via client-supplied
    # DATABASE_URLs (CPT-AXO-042).
    # REQ-AXO-271 slice 6 follow-up (2026-05-10): URLs explicitly carry
    # the `axon` user — PostgreSQL rejects user-less URLs because the OS
    # user `dstadel` is not a PG role. The live brain already runs with
    # this exact form (env captured from a live process).
    AXON_LIVE_DATABASE_URL = "postgres://axon@127.0.0.1:44144/axon_live";
    AXON_DEV_DATABASE_URL = "postgres://axon@127.0.0.1:44144/axon_dev";
    # PGHOST used by psql CLI and sqlx-cli for hand operations.
    # PGPORT is auto-exported by the devenv postgres module from
    # services.postgres.port (int). Declaring it again here as a string
    # triggers `option `env.PGPORT' has conflicting option types`.
    PGHOST = "localhost";

    # devenv-nix-best-practices: Isolation Patterns
    RELEASE_COOKIE = "axon_v1_isolated_cookie";
    CARGO_TARGET_DIR = config.env.DEVENV_ROOT + "/.axon/cargo-target";
    DATA_DIR = config.env.DEVENV_ROOT + "/.axon/data";
    ERL_AFLAGS = "-kernel shell_history enabled";
    ELIXIR_ERL_OPTIONS = "+fnu";
    
    PYTHONPATH = config.env.DEVENV_ROOT + "/src";
    FILESYSTEM_FSINOTIFY_EXECUTABLE_FILE = "${pkgs.inotify-tools}/bin/inotifywait";
  };

  # Managed Processes
  processes = {
    core.exec = config.env.DEVENV_ROOT + "/bin/axon-core";
    
    nexus.exec = ''
      cd src/dashboard && mix ecto.setup && AXON_REPO_SLUG=axon AXON_WATCH_DIR="${config.env.DEVENV_ROOT}" mix phx.server
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
    if [ "''${AXON_SKIP_ELIXIR_PREWARM:-0}" != "1" ] && [ ! -f "$MIX_HOME/archives/hex-"* ]; then
      echo "📦 Pre-warming Elixir environment (Hex/Rebar)..." >&2
      mix local.hex --force > /dev/null 2>&1
      mix local.rebar --force > /dev/null 2>&1
    fi

    echo "--- AXON v1.0 - DEVENV ARCHITECTURE (MIL-AXO-015) ---" >&2
    echo "Plane A (Visualization): Elixir $(elixir --version | awk '/Elixir/ {print $2}')" >&2
    echo "Plane B (Runtime + Postgres): Rust $(rustc --version | awk '{print $2}')" >&2
    echo "Support Tooling:         Python $(python --version | awk '/Python/ {print $2}')" >&2
    echo "Storage:                 PostgreSQL 17 + pgvector + pgmq @ 127.0.0.1:44144" >&2
    echo "HydraDB:                 detached legacy workflow" >&2
    echo "---------------------------------------" >&2

    # Daily SOLL backup: fire-and-forget, idempotent (1×/UTC-day max).
    # Disabled with AXON_SKIP_SOLL_BACKUP=1.
    if [ "''${AXON_SKIP_SOLL_BACKUP:-0}" != "1" ] && [ -x "$DEVENV_ROOT/scripts/backup_soll_daily.sh" ]; then
      mkdir -p "$DEVENV_ROOT/.devenv"
      nohup bash "$DEVENV_ROOT/scripts/backup_soll_daily.sh" \
        >> "$DEVENV_ROOT/.devenv/backup_soll.log" 2>&1 </dev/null &
      disown 2>/dev/null || true
    fi
  '';
}
