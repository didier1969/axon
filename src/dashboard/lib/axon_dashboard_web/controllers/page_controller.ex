defmodule AxonDashboardWeb.PageController do
  use AxonDashboardWeb, :controller

  def home(conn, _params) do
    render(conn, :home)
  end
end
