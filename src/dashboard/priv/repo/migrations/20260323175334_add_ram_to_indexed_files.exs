defmodule AxonDashboard.Repo.Migrations.AddRamToIndexedFiles do
  use Ecto.Migration

  def change do
    alter table(:indexed_files) do
      add :ram_before_mb, :integer, default: 0
      add :ram_after_mb, :integer, default: 0
    end
  end
end
