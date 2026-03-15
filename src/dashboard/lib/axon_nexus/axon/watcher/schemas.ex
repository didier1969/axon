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
    field(:start_byte, :integer, default: 0)
    field(:end_byte, :integer, default: 0)
    field(:content, :string, default: "")
    field(:is_exported, :boolean, default: false)
    field(:is_entry_point, :boolean, default: false)
    field(:signature, :string, default: "")
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
      :start_byte,
      :end_byte,
      :content,
      :is_exported,
      :is_entry_point,
      :signature,
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
