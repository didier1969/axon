# Copyright (c) Didier Stadelmann. All rights reserved.
defmodule AxonDashboard.MixProject do
  use Mix.Project

  def project do
    [
      app: :axon_dashboard,
      version: "0.1.0",
      elixir: "~> 1.15",
      elixirc_paths: elixirc_paths(Mix.env()),
      start_permanent: Mix.env() == :prod,
      aliases: aliases(),
      deps: deps(),
      test_coverage: [
        summary: [threshold: 85],
        ignore_modules: [
          AxonDashboardWeb.CoreComponents,
          AxonDashboardWeb.Layouts,
          AxonDashboardWeb.PageHTML,
          AxonDashboardWeb.PageController,
          AxonDashboard.Application,
          AxonDashboardWeb.Telemetry
        ]
      ],
      compilers: [:phoenix_live_view] ++ Mix.compilers(),
      listeners: [Phoenix.CodeReloader]
    ]
  end

  # Configuration for the OTP application.
  #
  # Type `mix help compile.app` for more information.
  def application do
    [
      mod: {AxonDashboard.Application, []},
      extra_applications: [:logger, :runtime_tools, :os_mon]
    ]
  end

  def cli do
    [
      preferred_envs: [precommit: :test]
    ]
  end

  # Specifies which paths to compile per environment.
  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_), do: ["lib"]

  # Specifies your project dependencies.
  #
  # Type `mix help deps` for examples and options.
  defp deps do
    [
      {:phoenix, "~> 1.8.5"},
      {:phoenix_html, "~> 4.1"},
      {:phoenix_live_view, "~> 1.1.0"},
      {:lazy_html, ">= 0.1.0", only: :test},
      {:esbuild, "~> 0.10", runtime: Mix.env() == :dev},
      {:tailwind, "~> 0.3", runtime: Mix.env() == :dev},
      {:heroicons,
       github: "tailwindlabs/heroicons",
       tag: "v2.2.0",
       sparse: "optimized",
       app: false,
       compile: false,
       depth: 1},
      {:telemetry_metrics, "~> 1.0"},
      {:telemetry_poller, "~> 1.0"},
      {:jason, "~> 1.2"},
      {:msgpax, "~> 2.3"},
      {:dns_cluster, "~> 0.2.0"},
      {:bandit, "~> 1.5"},
      {:rustler, "~> 0.36.0", runtime: false},
      {:file_system, "~> 1.0"},
      # REQ-AXO-901801 (MIL-AXO-028 cat A) — ecto_sqlite3 removed. The
      # dashboard owns no canonical state — PG is the source of truth
      # (PIL-AXO-001 data-ownership convention). The dep was a leftover
      # from an early scaffolding experiment that left axon_nexus.db /
      # .db-shm / .db-wal turds at the project root with no supervised
      # Repo, no Ecto schema, and no migration path. Removing the dep
      # closes 4 transitive dependencies (db_connection, decimal, ecto,
      # ecto_sql) the dashboard never used.
      {:liveview_witness, path: "../liveview_witness"},
      # REQ-AXO-901649 — Wallaby drives Chrome via WebDriver for E2E feature
      # tests (test/axon_dashboard_web/features/). `runtime: false` keeps it
      # out of prod / dev compile graphs ; `only: :test` keeps the dep tree
      # lean. ChromeDriver + Chromium themselves are provisioned by
      # devenv.nix so every machine gets ABI-matched binaries.
      {:wallaby, "~> 0.30", runtime: false, only: :test}
    ]
  end

  # Aliases are shortcuts or tasks specific to the current project.
  # For example, to install project dependencies and perform other setup tasks, run:
  #
  #     $ mix setup
  #
  # See the documentation for `Mix` for more info on aliases.
  defp aliases do
    [
      setup: ["deps.get", "ecto.setup", "assets.setup", "assets.build"],
      "ecto.setup": ["ecto.create", "ecto.migrate"],
      "ecto.reset": ["ecto.drop", "ecto.setup"],
      "assets.setup": ["tailwind.install --if-missing", "esbuild.install --if-missing"],
      "assets.build": ["compile", "tailwind axon_dashboard", "esbuild axon_dashboard"],
      "assets.deploy": [
        "tailwind axon_dashboard --minify",
        "esbuild axon_dashboard --minify",
        "phx.digest"
      ],
      precommit: ["compile --warnings-as-errors", "deps.unlock --unused", "format", "test"]
    ]
  end
end
