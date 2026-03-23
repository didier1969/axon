defmodule AxonDashboard.Repo.Migrations.AddTelemetryToIndexedFiles do
  use Ecto.Migration

  def change do
    alter table(:indexed_files) do
      add :file_size, :integer, default: 0
      add :ingestion_duration_ms, :integer, default: 0
    end
  end
end
