defmodule Axon.Watcher.Repo do
  use Ecto.Repo,
    otp_app: :axon_watcher,
    adapter: Ecto.Adapters.SQLite3
end
