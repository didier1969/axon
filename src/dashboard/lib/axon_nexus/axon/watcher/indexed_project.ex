defmodule Axon.Watcher.IndexedProject do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key {:id, :string, autogenerate: false}
  schema "indexed_projects" do
    field :name, :string
    field :path, :string

    # Status and Telemetry
    field :status, :string, default: "active" # active, ignored
    field :total_files, :integer, default: 0
    field :indexed_files, :integer, default: 0
    field :failed_files, :integer, default: 0
    field :ignored_files, :integer, default: 0

    # Health Metrics (from Data Plane)
    field :security_score, :integer, default: 100
    field :coverage_score, :integer, default: 0
    field :total_symbols, :integer, default: 0
    field :total_relations, :integer, default: 0

    has_many :files, Axon.Watcher.IndexedFile, foreign_key: :project_id

    timestamps(type: :utc_datetime)
  end

  @doc false
  def changeset(indexed_project, attrs) do
    indexed_project
    |> cast(attrs, [
      :id, :name, :path, :status,
      :total_files, :indexed_files, :failed_files, :ignored_files,
      :security_score, :coverage_score, :total_symbols, :total_relations
    ])
    |> validate_required([:id, :name, :path])
    |> validate_inclusion(:status, ["active", "ignored"])
  end
end
