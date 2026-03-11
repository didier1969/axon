defmodule AxonDashboardWeb.StatusLive do
  use AxonDashboardWeb, :live_view

  def mount(_params, _session, socket) do
    if connected?(socket) do
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
    end

    {:ok, assign(socket, events: [], total_symbols: 0, status: :waiting)}
  end

  def render(assigns) do
    ~H"""
    <div class="p-8">
      <h1 class="text-3xl font-bold mb-6">Axon v2 Dashboard</h1>
      
      <div class="grid grid-cols-1 md:grid-cols-3 gap-6 mb-8">
        <div class="stat bg-base-200 rounded-box p-4">
          <div class="stat-title text-sm opacity-60">Status</div>
          <div class={"stat-value text-2xl #{if @status == :processing, do: "text-primary", else: ""}"}>
            <%= @status |> Atom.to_string() |> String.capitalize() %>
          </div>
        </div>
        
        <div class="stat bg-base-200 rounded-box p-4">
          <div class="stat-title text-sm opacity-60">Total Symbols</div>
          <div class="stat-value text-2xl text-secondary"><%= @total_symbols %></div>
        </div>
        
        <div class="stat bg-base-200 rounded-box p-4">
          <div class="stat-title text-sm opacity-60">Files Indexed</div>
          <div class="stat-value text-2xl"><%= length(@events) %></div>
        </div>
      </div>

      <div class="bg-base-300 p-4 rounded-box">
        <h2 class="text-xl font-semibold mb-4 text-secondary-focus">Recent Events</h2>
        <div class="overflow-x-auto h-96">
          <table class="table table-compact w-full">
            <thead>
              <tr class="opacity-50 border-b border-base-100">
                <th class="py-2">File</th>
                <th class="py-2">Symbols</th>
              </tr>
            </thead>
            <tbody>
              <%= for event <- Enum.take(@events, 20) do %>
                <tr class="border-b border-base-200/50 hover:bg-base-100/30 transition-colors">
                  <td class="py-2 text-xs font-mono"><%= event["path"] %></td>
                  <td class="py-2 font-bold"><%= event["symbol_count"] %></td>
                </tr>
              <% end %>
            </tbody>
          </table>
        </div>
      </div>
    </div>
    """
  end

  def handle_info({:bridge_event, ["FileIndexed", data]}, socket) do
    # Mise à jour des stats en direct
    new_events = [data | socket.assigns.events]
    new_total = socket.assigns.total_symbols + data["symbol_count"]
    
    {:noreply, assign(socket, events: new_events, total_symbols: new_total, status: :processing)}
  end

  def handle_info({:bridge_event, ["ScanComplete", _data]}, socket) do
    {:noreply, assign(socket, status: :complete)}
  end

  def handle_info({:bridge_event, _event}, socket) do
    {:noreply, socket}
  end
end
