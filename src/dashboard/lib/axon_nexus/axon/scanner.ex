defmodule Axon.Scanner do
  use Rustler, otp_app: :axon_dashboard, crate: "axon_scanner"

  def scan(_path), do: :erlang.nif_error(:nif_not_loaded)
  def start_streaming(_path, _pid), do: :erlang.nif_error(:nif_not_loaded)
end
