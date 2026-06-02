defmodule AxonDashboard.DashboardState do
  @moduledoc """
  REQ-AXO-901806 — typed envelope for the brain's `dashboard_state_v1`
  PubSub event. Maps the raw JSON payload (string keys, produced by
  `Jason.decode/1`) into an Elixir struct with atom keys so LiveView
  callsites get :
  - compile-time validation (`%DashboardState{totals: %Totals{}}`)
  - autocomplete + pattern-match safety
  - explicit conversion boundary (single `from_map/1` call site)

  No business logic lives here — this is a thin DTO between the
  BridgeClient parsing layer and the LiveViews. The companion modules
  `Totals`, `PipelineConfig`, `Lifecycle`, `Filesystem`, and
  `PerProjectEntry` mirror the brain envelope sub-blocks.
  """

  alias AxonDashboard.DashboardState.{
    Embedder,
    Filesystem,
    Lifecycle,
    PerProjectEntry,
    PipelineConfig,
    Runtime,
    RuntimeConfig,
    Telemetry,
    Totals
  }

  @type t :: %__MODULE__{
          ts_ms: non_neg_integer() | nil,
          runtime: Runtime.t() | nil,
          embedder: Embedder.t() | nil,
          telemetry: Telemetry.t() | nil,
          filesystem: Filesystem.t() | nil,
          lifecycle: Lifecycle.t() | nil,
          totals: Totals.t() | nil,
          per_project: [PerProjectEntry.t()],
          runtime_config: RuntimeConfig.t() | nil
        }

  defstruct ts_ms: nil,
            runtime: nil,
            embedder: nil,
            telemetry: nil,
            filesystem: nil,
            lifecycle: nil,
            totals: nil,
            per_project: [],
            runtime_config: nil

  @doc """
  Convert a raw `Jason.decode/1` output (string-keyed map) to a typed
  `%DashboardState{}`. Missing keys are tolerated and default to `nil`
  or empty list — the brain may emit a degraded envelope during
  startup before PG cache is warm.
  """
  @spec from_map(map() | nil) :: t()
  def from_map(nil), do: %__MODULE__{}

  def from_map(%{} = m) do
    %__MODULE__{
      ts_ms: num(m, "ts_ms"),
      runtime: Runtime.from_map(Map.get(m, "runtime")),
      embedder: Embedder.from_map(Map.get(m, "embedder")),
      telemetry: Telemetry.from_map(Map.get(m, "telemetry")),
      filesystem: Filesystem.from_map(Map.get(m, "filesystem")),
      lifecycle: Lifecycle.from_map(Map.get(m, "lifecycle")),
      totals: Totals.from_map(Map.get(m, "totals")),
      per_project:
        case Map.get(m, "per_project") do
          list when is_list(list) -> Enum.map(list, &PerProjectEntry.from_map/1)
          _ -> []
        end,
      runtime_config: RuntimeConfig.from_map(Map.get(m, "runtime_config"))
    }
  end

  @doc "Wall-clock age of the envelope in milliseconds (best-effort)."
  @spec observed_age_ms(t() | nil) :: non_neg_integer() | nil
  def observed_age_ms(nil), do: nil
  def observed_age_ms(%__MODULE__{ts_ms: nil}), do: nil

  def observed_age_ms(%__MODULE__{ts_ms: ts_ms}) when is_integer(ts_ms) do
    max(0, System.system_time(:millisecond) - ts_ms)
  end

  @doc false
  def num(m, k), do: m |> Map.get(k) |> coerce_int()

  defp coerce_int(n) when is_integer(n), do: n
  defp coerce_int(n) when is_float(n), do: round(n)
  defp coerce_int(_), do: nil

  # ── Sub-envelope structs ────────────────────────────────────────

  defmodule Runtime do
    @moduledoc "Identity + degraded reason produced once per tick."
    @type t :: %__MODULE__{
            build_id: String.t() | nil,
            install_generation: String.t() | nil,
            runtime_mode: String.t() | nil,
            instance_kind: String.t() | nil,
            degraded_reason: String.t() | nil,
            runtime_idle: boolean(),
            pipeline_status: String.t() | nil,
            blocked_reason: String.t() | nil
          }
    defstruct build_id: nil,
              install_generation: nil,
              runtime_mode: nil,
              instance_kind: nil,
              degraded_reason: nil,
              runtime_idle: false,
              pipeline_status: nil,
              blocked_reason: nil

    def from_map(nil), do: nil

    def from_map(%{} = m) do
      %__MODULE__{
        build_id: Map.get(m, "build_id"),
        install_generation: Map.get(m, "install_generation"),
        runtime_mode: Map.get(m, "runtime_mode"),
        instance_kind: Map.get(m, "instance_kind"),
        degraded_reason: Map.get(m, "degraded_reason"),
        runtime_idle: Map.get(m, "runtime_idle", false) == true,
        pipeline_status: Map.get(m, "pipeline_status"),
        blocked_reason: Map.get(m, "blocked_reason")
      }
    end
  end

  defmodule Embedder do
    @moduledoc "Embedder provider state."
    @type t :: %__MODULE__{
            requested: String.t() | nil,
            effective: String.t() | nil,
            init_error: String.t() | nil,
            last_lane: String.t() | nil,
            compute: String.t() | nil,
            compute_source: String.t() | nil
          }
    defstruct requested: nil,
              effective: nil,
              init_error: nil,
              last_lane: nil,
              # DEC-AXO-901626 — observable Pipeline B compute verdict.
              compute: nil,
              compute_source: nil

    def from_map(nil), do: nil

    def from_map(%{} = m) do
      %__MODULE__{
        requested: Map.get(m, "requested"),
        effective: Map.get(m, "effective"),
        init_error: Map.get(m, "init_error"),
        last_lane: Map.get(m, "last_lane"),
        compute: Map.get(m, "compute"),
        compute_source: Map.get(m, "compute_source")
      }
    end
  end

  defmodule Telemetry do
    @moduledoc "Live runtime metrics (1 Hz)."
    @type t :: %__MODULE__{
            chunk_embeddings_per_second: float(),
            vector_chunks_embedded_cumulative: non_neg_integer(),
            graph_workers_active_current: non_neg_integer(),
            graph_workers_started_total: non_neg_integer(),
            ingress_buffered_entries: non_neg_integer(),
            ingress_hot_entries: non_neg_integer(),
            ready_queue_chunks_current: non_neg_integer(),
            ready_queue_chunks_small: non_neg_integer(),
            ready_queue_chunks_medium: non_neg_integer(),
            ready_queue_chunks_large: non_neg_integer(),
            homogeneous_batches_total: non_neg_integer(),
            mixed_fallback_batches_total: non_neg_integer(),
            service_pressure: String.t(),
            scheduler: String.t()
          }
    defstruct chunk_embeddings_per_second: 0.0,
              vector_chunks_embedded_cumulative: 0,
              graph_workers_active_current: 0,
              graph_workers_started_total: 0,
              ingress_buffered_entries: 0,
              ingress_hot_entries: 0,
              ready_queue_chunks_current: 0,
              ready_queue_chunks_small: 0,
              ready_queue_chunks_medium: 0,
              ready_queue_chunks_large: 0,
              homogeneous_batches_total: 0,
              mixed_fallback_batches_total: 0,
              service_pressure: "unknown",
              scheduler: "unknown"

    def from_map(nil), do: nil

    def from_map(%{} = m) do
      %__MODULE__{
        chunk_embeddings_per_second: float(m, "chunk_embeddings_per_second"),
        vector_chunks_embedded_cumulative: int(m, "vector_chunks_embedded_cumulative"),
        graph_workers_active_current: int(m, "graph_workers_active_current"),
        graph_workers_started_total: int(m, "graph_workers_started_total"),
        ingress_buffered_entries: int(m, "ingress_buffered_entries"),
        ingress_hot_entries: int(m, "ingress_hot_entries"),
        ready_queue_chunks_current: int(m, "ready_queue_chunks_current"),
        ready_queue_chunks_small: int(m, "ready_queue_chunks_small"),
        ready_queue_chunks_medium: int(m, "ready_queue_chunks_medium"),
        ready_queue_chunks_large: int(m, "ready_queue_chunks_large"),
        homogeneous_batches_total: int(m, "homogeneous_batches_total"),
        mixed_fallback_batches_total: int(m, "mixed_fallback_batches_total"),
        service_pressure: Map.get(m, "service_pressure", "unknown"),
        scheduler: Map.get(m, "scheduler", "unknown")
      }
    end

    defp int(m, k) do
      case Map.get(m, k) do
        n when is_integer(n) -> n
        n when is_float(n) -> round(n)
        _ -> 0
      end
    end

    defp float(m, k) do
      case Map.get(m, k) do
        n when is_float(n) -> n
        n when is_integer(n) -> n * 1.0
        _ -> 0.0
      end
    end
  end

  defmodule Filesystem do
    @moduledoc "Cached watcher walk counts (60 s TTL brain-side)."
    @type t :: %__MODULE__{disk_files: integer(), eligible_files: integer()}
    defstruct disk_files: -1, eligible_files: -1

    def from_map(nil), do: nil

    def from_map(%{} = m) do
      %__MODULE__{
        disk_files: Map.get(m, "disk_files", -1),
        eligible_files: Map.get(m, "eligible_files", -1)
      }
    end
  end

  defmodule Lifecycle do
    @moduledoc "Embedder lifecycle (PG-backed heartbeat or brain-local fallback)."
    @type t :: %__MODULE__{
            phase: String.t() | nil,
            source: String.t() | nil,
            heartbeat_age_ms: non_neg_integer() | nil,
            wake_count: non_neg_integer(),
            sleep_count: non_neg_integer(),
            last_used_ms: non_neg_integer() | nil
          }
    defstruct phase: nil,
              source: nil,
              heartbeat_age_ms: nil,
              wake_count: 0,
              sleep_count: 0,
              last_used_ms: nil

    def from_map(nil), do: nil

    def from_map(%{} = m) do
      %__MODULE__{
        phase: Map.get(m, "phase"),
        source: Map.get(m, "source"),
        heartbeat_age_ms: Map.get(m, "heartbeat_age_ms"),
        wake_count: Map.get(m, "wake_count", 0),
        sleep_count: Map.get(m, "sleep_count", 0),
        last_used_ms: Map.get(m, "last_used_ms")
      }
    end
  end

  defmodule Totals do
    @moduledoc "PG aggregate totals across all projects."
    @type t :: %__MODULE__{
            files: non_neg_integer(),
            files_indexed: non_neg_integer(),
            files_inflight: non_neg_integer(),
            symbols: non_neg_integer(),
            edges: non_neg_integer(),
            chunks: non_neg_integer(),
            embedded: non_neg_integer(),
            pending: non_neg_integer(),
            orphan_embeddings: non_neg_integer(),
            projects: non_neg_integer(),
            coverage_pct: float()
          }
    defstruct files: 0,
              files_indexed: 0,
              files_inflight: 0,
              symbols: 0,
              edges: 0,
              chunks: 0,
              embedded: 0,
              pending: 0,
              orphan_embeddings: 0,
              projects: 0,
              coverage_pct: 0.0

    def from_map(nil), do: nil

    def from_map(%{} = m) do
      %__MODULE__{
        files: i(m, "files"),
        files_indexed: i(m, "files_indexed"),
        files_inflight: i(m, "files_inflight"),
        symbols: i(m, "symbols"),
        edges: i(m, "edges"),
        chunks: i(m, "chunks"),
        embedded: i(m, "embedded"),
        pending: i(m, "pending"),
        orphan_embeddings: i(m, "orphan_embeddings"),
        projects: i(m, "projects"),
        coverage_pct: f(m, "coverage_pct")
      }
    end

    defp i(m, k) do
      case Map.get(m, k) do
        n when is_integer(n) -> n
        n when is_float(n) -> round(n)
        _ -> 0
      end
    end

    defp f(m, k) do
      case Map.get(m, k) do
        n when is_float(n) -> n
        n when is_integer(n) -> n * 1.0
        _ -> 0.0
      end
    end
  end

  defmodule PerProjectEntry do
    @moduledoc "Per-project breakdown row (used with Phoenix.LiveView.stream/3)."
    @type t :: %__MODULE__{
            project_code: String.t(),
            chunks: non_neg_integer(),
            embedded: non_neg_integer(),
            symbols: non_neg_integer(),
            edges: non_neg_integer(),
            coverage_pct: float()
          }
    @derive {Jason.Encoder, only: [:project_code, :chunks, :embedded, :symbols, :edges, :coverage_pct]}
    defstruct project_code: "?",
              chunks: 0,
              embedded: 0,
              symbols: 0,
              edges: 0,
              coverage_pct: 0.0

    def from_map(nil), do: nil

    def from_map(%{} = m) do
      %__MODULE__{
        project_code: Map.get(m, "project_code", "?"),
        chunks: i(m, "chunks"),
        embedded: i(m, "embedded"),
        symbols: i(m, "symbols"),
        edges: i(m, "edges"),
        coverage_pct: f(m, "coverage_pct")
      }
    end

    defp i(m, k) do
      case Map.get(m, k) do
        n when is_integer(n) -> n
        n when is_float(n) -> round(n)
        _ -> 0
      end
    end

    defp f(m, k) do
      case Map.get(m, k) do
        n when is_float(n) -> n
        n when is_integer(n) -> n * 1.0
        _ -> 0.0
      end
    end
  end

  defmodule PipelineConfig do
    @moduledoc "Sub-block of RuntimeConfig — workers + batch sizes per pipeline."
    @type t :: %__MODULE__{
            a1_workers: non_neg_integer(),
            a2_workers: non_neg_integer(),
            a3_workers: non_neg_integer(),
            a3_batch_size: non_neg_integer(),
            a3_batch_timeout_ms: non_neg_integer(),
            b1_workers: non_neg_integer(),
            b2_workers: non_neg_integer(),
            b3_workers: non_neg_integer(),
            b2_batch_size: non_neg_integer(),
            b2_batch_timeout_ms: non_neg_integer(),
            b3_batch_size: non_neg_integer(),
            b3_batch_timeout_ms: non_neg_integer(),
            a3_to_b1_buffer_cap: non_neg_integer(),
            coldstart_batch_size: non_neg_integer()
          }
    defstruct a1_workers: 0,
              a2_workers: 0,
              a3_workers: 0,
              a3_batch_size: 0,
              a3_batch_timeout_ms: 0,
              b1_workers: 0,
              b2_workers: 0,
              b3_workers: 0,
              b2_batch_size: 0,
              b2_batch_timeout_ms: 0,
              b3_batch_size: 0,
              b3_batch_timeout_ms: 0,
              a3_to_b1_buffer_cap: 0,
              coldstart_batch_size: 0

    def from_map(nil), do: %__MODULE__{}
    def from_map(%{} = m) do
      pa = Map.get(m, "pipeline_a", %{})
      pb = Map.get(m, "pipeline_b", %{})
      %__MODULE__{
        a1_workers: int(pa, "a1_workers"),
        a2_workers: int(pa, "a2_workers"),
        a3_workers: int(pa, "a3_workers"),
        a3_batch_size: int(pa, "a3_batch_size"),
        a3_batch_timeout_ms: int(pa, "a3_batch_timeout_ms"),
        b1_workers: int(pb, "b1_workers"),
        b2_workers: int(pb, "b2_workers"),
        b3_workers: int(pb, "b3_workers"),
        b2_batch_size: int(pb, "b2_batch_size"),
        b2_batch_timeout_ms: int(pb, "b2_batch_timeout_ms"),
        b3_batch_size: int(pb, "b3_batch_size"),
        b3_batch_timeout_ms: int(pb, "b3_batch_timeout_ms"),
        a3_to_b1_buffer_cap: int(pb, "a3_to_b1_buffer_cap"),
        coldstart_batch_size: int(pb, "coldstart_batch_size")
      }
    end

    defp int(m, k) do
      case Map.get(m, k) do
        n when is_integer(n) -> n
        n when is_float(n) -> round(n)
        _ -> 0
      end
    end
  end

  defmodule RuntimeConfig do
    @moduledoc """
    Boot-time semi-static configs from PG `runtime_config_snapshot`.
    See `crate::runtime_config` (axon-core) for the writer side.
    """
    @type t :: %__MODULE__{
            pipeline: PipelineConfig.t(),
            notify_channel: String.t() | nil,
            coldstart_poll_interval_secs: non_neg_integer(),
            ingress_drain_batch: non_neg_integer()
          }
    defstruct pipeline: %PipelineConfig{},
              notify_channel: nil,
              coldstart_poll_interval_secs: 0,
              ingress_drain_batch: 0

    def from_map(nil), do: %__MODULE__{}

    def from_map(%{} = m) do
      %__MODULE__{
        pipeline: PipelineConfig.from_map(m),
        notify_channel: Map.get(m, "notify_channel"),
        coldstart_poll_interval_secs: int(m, "coldstart_poll_interval_secs"),
        ingress_drain_batch: int(m, "ingress_drain_batch")
      }
    end

    defp int(m, k) do
      case Map.get(m, k) do
        n when is_integer(n) -> n
        n when is_float(n) -> round(n)
        _ -> 0
      end
    end
  end
end
