defmodule Axon.Watcher.Schemas.Symbol do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key false
  embedded_schema do
    field(:id, :string)
    field(:name, :string)
    field(:kind, :string)
    field(:start_line, :integer)
    field(:end_line, :integer)
    field(:is_public, :boolean, default: true)
    field(:is_entry_point, :boolean, default: false)
    field(:is_nif, :boolean, default: false)
    field(:is_unsafe, :boolean, default: false)
    field(:tested, :boolean, default: false)
    field(:centrality, :float, default: 0.0)
  end

  def changeset(schema, attrs) do
    schema
    |> cast(attrs, [
      :id,
      :name,
      :kind,
      :start_line,
      :end_line,
      :is_public,
      :is_entry_point,
      :is_nif,
      :is_unsafe,
      :tested,
      :centrality
    ])
    |> validate_required([:id, :name, :kind, :start_line, :end_line])
  end
end

defmodule Axon.Watcher.Schemas.Relationship do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key false
  embedded_schema do
    field(:source, :string)
    field(:target, :string)
    field(:type, :string)
    field(:properties, :map, default: %{})
  end

  def changeset(schema, attrs) do
    schema
    |> cast(attrs, [:source, :target, :type, :properties])
    |> validate_required([:source, :target, :type])
  end
end

defmodule Axon.Watcher.Schemas.ExtractionResult do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key false
  embedded_schema do
    field(:path, :string)
    field(:language, :string)
    embeds_many(:symbols, Axon.Watcher.Schemas.Symbol)
    embeds_many(:relationships, Axon.Watcher.Schemas.Relationship)
  end

  def changeset(schema, attrs) do
    schema
    |> cast(attrs, [:path, :language])
    |> validate_required([:path, :language])
    |> cast_embed(:symbols)
    |> cast_embed(:relationships)
  end
end
