defmodule Axon.Watcher.Repo.Migrations.AddIndexingHistory do
  use Ecto.Migration

  def change do
    create table(:indexed_projects, primary_key: false) do
      add :id, :string, primary_key: true
      add :name, :string, null: false
      add :path, :string, null: false
      
      # Status and Telemetry
      add :status, :string, default: "active" # active, ignored
      add :total_files, :integer, default: 0
      add :indexed_files, :integer, default: 0
      add :failed_files, :integer, default: 0
      add :ignored_files, :integer, default: 0
      
      # Health Metrics (from Data Plane)
      add :security_score, :integer, default: 100
      add :coverage_score, :integer, default: 0
      add :total_symbols, :integer, default: 0
      add :total_relations, :integer, default: 0

      timestamps(type: :utc_datetime)
    end

    create table(:indexed_files, primary_key: false) do
      add :id, :string, primary_key: true
      add :project_id, references(:indexed_projects, type: :string, on_delete: :delete_all)
      
      add :path, :string, null: false
      add :status, :string, null: false # pending, indexed, failed, ignored_by_rule
      add :error_reason, :string
      add :file_hash, :integer # To skip unmodified files
      
      # Extracted data stats
      add :symbols_count, :integer, default: 0
      add :relations_count, :integer, default: 0
      add :loc, :integer, default: 0 # Lines of Code
      
      # Health
      add :security_score, :integer, default: 100
      add :coverage_score, :integer, default: 0
      add :is_entry_point, :boolean, default: false

      timestamps(type: :utc_datetime)
    end

    create index(:indexed_files, [:project_id])
    create index(:indexed_files, [:status])
    create index(:indexed_files, [:path])
  end
end
