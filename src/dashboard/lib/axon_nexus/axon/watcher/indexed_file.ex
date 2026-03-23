defmodule Axon.Watcher.IndexedFile do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key {:id, :string, autogenerate: false}
  schema "indexed_files" do
    belongs_to :project, Axon.Watcher.IndexedProject, foreign_key: :project_id, type: :string

    field :path, :string
    field :status, :string # pending, indexed, failed, ignored_by_rule
    field :error_reason, :string
    field :file_hash, :integer # To skip unmodified files

    # Extracted data stats
    field :symbols_count, :integer, default: 0
    field :relations_count, :integer, default: 0
    field :loc, :integer, default: 0 # Lines of Code

    # Telemetry
    field :file_size, :integer, default: 0
    field :ingestion_duration_ms, :integer, default: 0
    field :ram_before_mb, :integer, default: 0
    field :ram_after_mb, :integer, default: 0

    # Health
    field :security_score, :integer, default: 100
    field :coverage_score, :integer, default: 0
    field :is_entry_point, :boolean, default: false

    timestamps(type: :utc_datetime)
    end

    @doc false
    def changeset(indexed_file, attrs) do
    indexed_file
    |> cast(attrs, [
      :id, :project_id, :path, :status, :error_reason, :file_hash,
      :symbols_count, :relations_count, :loc, :security_score, :coverage_score, :is_entry_point,
      :file_size, :ingestion_duration_ms, :ram_before_mb, :ram_after_mb
    ])
    |> validate_required([:id, :project_id, :path, :status])
    |> validate_inclusion(:status, ["pending", "indexed", "failed", "ignored_by_rule"])
  end
end
